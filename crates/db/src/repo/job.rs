//! Job queries.
//!
//! The list query is the performance-critical path (NFR-P-02): filters compose in SQL,
//! pagination is keyset-based, and the projection is only what a table row renders.

use jobseeker_core::domain::enums::{JobStatus, Seniority, WorkMode};
use jobseeker_core::ids::{CompanyId, JobId, ListingId};
use jobseeker_core::Result;
use sqlx::Row;

use crate::repo::{clamp_limit, Cursor, Page};
use crate::{db_err, Db};

/// The projection a job list row needs. Deliberately narrow: the full record is one more
/// request away, and list queries must not drag description text through the pipe.
#[derive(Debug, Clone, serde::Serialize)]
pub struct JobListRow {
    pub id: String,
    pub title: String,
    pub company_name: String,
    pub company_slug: String,
    pub status: String,
    pub work_mode: String,
    pub seniority: String,
    pub salary_min_cents: Option<i64>,
    pub salary_max_cents: Option<i64>,
    pub salary_currency: Option<String>,
    pub salary_period: String,
    pub salary_is_estimate: bool,
    pub posted_at: Option<String>,
    pub closes_at: Option<String>,
    pub primary_location: Option<String>,
    pub user_rating: Option<i64>,
    pub is_archived: bool,
    pub extraction_partial: bool,
    pub match_overall: Option<f64>,
    /// Required bars with the year-count haircut removed. Diagnostic; not in overall.
    #[serde(default)]
    pub skills_coverage: Option<f64>,
    /// Tenure bars only. Diagnostic; not in overall.
    #[serde(default)]
    pub years_fit: Option<f64>,
    pub updated_at: String,
}

/// Filters for the job list. Every field is optional; `None` means "do not constrain".
#[derive(Debug, Clone, Default)]
pub struct JobFilter {
    /// FTS5 query string. When present, results are ranked by bm25 rather than by `sort`.
    pub query: Option<String>,
    pub status: Option<JobStatus>,
    pub work_mode: Option<WorkMode>,
    pub seniority: Option<Seniority>,
    pub company_id: Option<CompanyId>,
    /// Minimum *annualized* maximum salary, so an hourly contract is comparable.
    pub min_salary_cents: Option<i64>,
    pub country: Option<String>,
    pub include_archived: bool,
    pub exclude_clearance: bool,
    pub posted_after: Option<String>,
    pub closes_before: Option<String>,
    pub sort: JobSort,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum JobSort {
    #[default]
    PostedAt,
    UpdatedAt,
    Salary,
    Relevance,
}

impl JobSort {
    fn column(self) -> &'static str {
        match self {
            JobSort::PostedAt => "j.posted_at",
            JobSort::UpdatedAt => "j.updated_at",
            JobSort::Salary => "j.salary_max_cents",
            // Relevance is handled by the FTS branch; fall back to recency without a query.
            JobSort::Relevance => "j.posted_at",
        }
    }
}

/// Paged, filtered job list.
///
/// Keyset pagination on `(sort_column, id)`: `OFFSET` on a growing table degrades linearly,
/// and the id tiebreak keeps the order total so no row is skipped or repeated.
pub async fn list(db: &Db, filter: &JobFilter) -> Result<Page<JobListRow>> {
    let limit = clamp_limit(filter.limit);
    let sort_col = filter.sort.column();
    let use_fts = filter
        .query
        .as_deref()
        .is_some_and(|q| !q.trim().is_empty());

    let mut sql = String::from(
        "SELECT j.id, j.title, c.name AS company_name, c.slug AS company_slug, j.status,
                j.work_mode, j.seniority, j.salary_min_cents, j.salary_max_cents,
                j.salary_currency, j.salary_period, j.salary_is_estimate,
                j.posted_at, j.closes_at, j.user_rating, j.is_archived,
                j.extraction_partial, j.updated_at,
                (SELECT m.overall FROM match_score m
                  WHERE m.job_id = j.id
                  ORDER BY m.computed_at DESC LIMIT 1) AS match_overall,
                (SELECT COALESCE(NULLIF(TRIM(COALESCE(l.city,'') || CASE WHEN l.city IS NOT NULL
                                          AND l.region IS NOT NULL THEN ', ' ELSE '' END
                                          || COALESCE(l.region,'')), ''), l.raw)
                   FROM job_location l
                  WHERE l.job_id = j.id
                  ORDER BY l.is_primary DESC, l.ordinal ASC LIMIT 1) AS primary_location
           FROM job j
           JOIN company c ON c.id = j.company_id",
    );
    if use_fts {
        sql.push_str(" JOIN job_fts f ON f.rowid = j.fts_rowid");
    }
    sql.push_str(" WHERE j.deleted_at IS NULL");

    // Bind parameters are pushed in the same order the placeholders are appended.
    let mut binds: Vec<Bind> = Vec::new();

    if use_fts {
        sql.push_str(" AND job_fts MATCH ?");
        binds.push(Bind::Text(filter.query.clone().unwrap_or_default()));
    }
    if !filter.include_archived {
        sql.push_str(" AND j.is_archived = 0");
    }
    if let Some(status) = filter.status {
        sql.push_str(" AND j.status = ?");
        binds.push(Bind::Text(status.as_str().to_string()));
    }
    if let Some(mode) = filter.work_mode {
        sql.push_str(" AND j.work_mode = ?");
        binds.push(Bind::Text(mode.as_str().to_string()));
    }
    if let Some(seniority) = filter.seniority {
        sql.push_str(" AND j.seniority = ?");
        binds.push(Bind::Text(seniority.as_str().to_string()));
    }
    if let Some(company) = &filter.company_id {
        sql.push_str(" AND j.company_id = ?");
        binds.push(Bind::Text(company.as_str().to_string()));
    }
    if let Some(min) = filter.min_salary_cents {
        // Annualize in SQL so a $60/hr contract is comparable with a $150k salary. Jobs with
        // no stated band are excluded from a salary filter rather than assumed to fail it —
        // roughly half of postings have no band, and silently hiding them would be wrong.
        sql.push_str(
            " AND j.salary_max_cents IS NOT NULL AND (j.salary_max_cents * CASE j.salary_period
                    WHEN 'hour' THEN 2080 WHEN 'day' THEN 260 WHEN 'week' THEN 52
                    WHEN 'month' THEN 12 ELSE 1 END) >= ?",
        );
        binds.push(Bind::Int(min));
    }
    if let Some(country) = &filter.country {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM job_location l WHERE l.job_id = j.id AND l.country = ?)",
        );
        binds.push(Bind::Text(country.clone()));
    }
    if filter.exclude_clearance {
        sql.push_str(" AND j.requires_clearance IS NULL");
    }
    if let Some(after) = &filter.posted_after {
        sql.push_str(" AND j.posted_at >= ?");
        binds.push(Bind::Text(after.clone()));
    }
    if let Some(before) = &filter.closes_before {
        sql.push_str(" AND j.closes_at IS NOT NULL AND j.closes_at <= ?");
        binds.push(Bind::Text(before.clone()));
    }

    if !use_fts {
        if let Some(cursor) = &filter.cursor {
            let cursor = Cursor::decode(cursor)?;
            // Descending keyset: strictly older, or same key with a smaller id.
            sql.push_str(&format!(
                " AND (COALESCE({sort_col}, '') < ? \
                   OR (COALESCE({sort_col}, '') = ? AND j.id < ?))"
            ));
            binds.push(Bind::Text(cursor.sort_key.clone()));
            binds.push(Bind::Text(cursor.sort_key.clone()));
            binds.push(Bind::Text(cursor.id.clone()));
        }
        sql.push_str(&format!(
            " ORDER BY COALESCE({sort_col}, '') DESC, j.id DESC LIMIT ?"
        ));
    } else {
        // Title and company are weighted up; requirement text matters more than body prose.
        sql.push_str(" ORDER BY bm25(job_fts, 10.0, 6.0, 1.0, 3.0) LIMIT ?");
    }
    binds.push(Bind::Int(limit + 1)); // one extra row tells us whether a next page exists

    // Every fragment appended above is a literal; the only interpolation is `sort_col`, which
    // comes from a `JobSort` enum, and all user data travels through `binds`.
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
    for bind in &binds {
        q = match bind {
            Bind::Text(s) => q.bind(s),
            Bind::Int(i) => q.bind(i),
        };
    }
    let rows = q.fetch_all(db.reader()).await.map_err(db_err)?;

    let has_more = rows.len() as i64 > limit;
    let mut items = Vec::with_capacity(rows.len().min(limit as usize));
    for row in rows.iter().take(limit as usize) {
        items.push(JobListRow {
            id: row.try_get("id").map_err(db_err)?,
            title: row.try_get("title").map_err(db_err)?,
            company_name: row.try_get("company_name").map_err(db_err)?,
            company_slug: row.try_get("company_slug").map_err(db_err)?,
            status: row.try_get("status").map_err(db_err)?,
            work_mode: row.try_get("work_mode").map_err(db_err)?,
            seniority: row.try_get("seniority").map_err(db_err)?,
            salary_min_cents: row.try_get("salary_min_cents").map_err(db_err)?,
            salary_max_cents: row.try_get("salary_max_cents").map_err(db_err)?,
            salary_currency: row.try_get("salary_currency").map_err(db_err)?,
            salary_period: row.try_get("salary_period").map_err(db_err)?,
            salary_is_estimate: row
                .try_get::<i64, _>("salary_is_estimate")
                .map_err(db_err)?
                != 0,
            posted_at: row.try_get("posted_at").map_err(db_err)?,
            closes_at: row.try_get("closes_at").map_err(db_err)?,
            primary_location: row.try_get("primary_location").map_err(db_err)?,
            user_rating: row.try_get("user_rating").map_err(db_err)?,
            is_archived: row.try_get::<i64, _>("is_archived").map_err(db_err)? != 0,
            extraction_partial: row
                .try_get::<i64, _>("extraction_partial")
                .map_err(db_err)?
                != 0,
            match_overall: row.try_get("match_overall").map_err(db_err)?,
            skills_coverage: None,
            years_fit: None,
            updated_at: row.try_get("updated_at").map_err(db_err)?,
        });
    }
    attach_list_breakdowns(db, &mut items).await?;

    // FTS results are relevance-ranked, and bm25 scores are not a stable keyset; that page
    // is deliberately single-page until relevance cursors are implemented.
    let next_cursor = if has_more && !use_fts {
        items.last().map(|last| {
            let sort_key = match filter.sort {
                JobSort::PostedAt | JobSort::Relevance => {
                    last.posted_at.clone().unwrap_or_default()
                }
                JobSort::UpdatedAt => last.updated_at.clone(),
                JobSort::Salary => last
                    .salary_max_cents
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            };
            Cursor {
                sort_key,
                id: last.id.clone(),
            }
            .encode()
        })
    } else {
        None
    };

    Ok(Page::new(items, next_cursor))
}

async fn attach_list_breakdowns(db: &Db, items: &mut [JobListRow]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let ids: Vec<String> = items.iter().map(|row| row.id.clone()).collect();
    let map = crate::repo::score::breakdowns_for_jobs(db, &ids).await?;
    for row in items {
        if let Some((skills, years)) = map.get(&row.id) {
            row.skills_coverage = *skills;
            row.years_fit = *years;
        }
    }
    Ok(())
}

enum Bind {
    Text(String),
    Int(i64),
}

/// Identity + hash of every live job, for `reconcile --check`.
#[derive(Debug, Clone)]
pub struct ReconcileRow {
    pub id: String,
    pub title: String,
    pub content_hash: String,
    pub file_path: Option<String>,
}

pub async fn list_ids(db: &Db) -> Result<Vec<jobseeker_core::ids::JobId>> {
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT id FROM job WHERE deleted_at IS NULL ORDER BY id")
            .fetch_all(db.reader())
            .await
            .map_err(db_err)?;
    rows.into_iter().map(|s| s.parse()).collect()
}

pub async fn list_reconcile_rows(db: &Db) -> Result<Vec<ReconcileRow>> {
    let rows = sqlx::query(
        "SELECT id, title, content_hash, file_path FROM job WHERE deleted_at IS NULL ORDER BY id",
    )
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(ReconcileRow {
            id: row.try_get("id").map_err(db_err)?,
            title: row.try_get("title").map_err(db_err)?,
            content_hash: row.try_get("content_hash").map_err(db_err)?,
            file_path: row.try_get("file_path").map_err(db_err)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityCounts {
    pub jobs: i64,
    pub companies: i64,
    pub requirements: i64,
}

pub async fn entity_counts(db: &Db) -> Result<EntityCounts> {
    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job WHERE deleted_at IS NULL")
        .fetch_one(db.reader())
        .await
        .map_err(db_err)?;
    let companies: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM company WHERE deleted_at IS NULL")
            .fetch_one(db.reader())
            .await
            .map_err(db_err)?;
    let requirements: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM requirement")
        .fetch_one(db.reader())
        .await
        .map_err(db_err)?;
    Ok(EntityCounts {
        jobs,
        companies,
        requirements,
    })
}

/// Count of jobs matching a status, for the dashboard facets.
pub async fn count_by_status(db: &Db) -> Result<Vec<(String, i64)>> {
    sqlx::query_as(
        "SELECT status, COUNT(*) FROM job
          WHERE deleted_at IS NULL AND is_archived = 0
          GROUP BY status",
    )
    .fetch_all(db.reader())
    .await
    .map_err(db_err)
}

/// Open jobs closing within `days` — the nag list.
pub async fn closing_soon(db: &Db, days: i64, limit: i64) -> Result<Vec<JobListRow>> {
    let cutoff = jobseeker_core::time::to_rfc3339(
        &(jobseeker_core::time::now() + chrono::Duration::days(days)),
    );
    list(
        db,
        &JobFilter {
            status: Some(JobStatus::Open),
            closes_before: Some(cutoff),
            limit: Some(limit),
            ..Default::default()
        },
    )
    .await
    .map(|page| page.items)
}

/// Allocate this job's FTS surrogate rowid and (re)index it.
///
/// Contentless FTS5 rows cannot be updated in place — a change is delete-then-insert, and
/// the delete must repeat the *old* column values, which triggers cannot see for the
/// requirements aggregate. So indexing lives here, in the same transaction as the write.
pub async fn reindex_fts(db: &Db, job_id: &JobId) -> Result<()> {
    let mut tx = db.writer().begin().await.map_err(db_err)?;

    let existing: Option<i64> = sqlx::query_scalar("SELECT fts_rowid FROM job WHERE id = ?1")
        .bind(job_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?
        .flatten();

    let rowid = match existing {
        Some(rowid) => {
            // Contentless delete requires the previous values; we re-read them from the row
            // we are about to replace.
            let old: Option<(String, String, String, String)> = sqlx::query_as(
                "SELECT title, company_name, description_text, requirements_text
                   FROM job_fts WHERE rowid = ?1",
            )
            .bind(rowid)
            .fetch_optional(&mut *tx)
            .await
            .ok()
            .flatten();
            if let Some((title, company, description, requirements)) = old {
                sqlx::query(
                    "INSERT INTO job_fts (job_fts, rowid, title, company_name, description_text,
                                          requirements_text)
                     VALUES ('delete', ?1, ?2, ?3, ?4, ?5)",
                )
                .bind(rowid)
                .bind(title)
                .bind(company)
                .bind(description)
                .bind(requirements)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            }
            rowid
        }
        None => {
            let next: i64 = sqlx::query_scalar(
                "UPDATE fts_rowid_seq SET value = value + 1 WHERE name = 'job' RETURNING value",
            )
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;
            sqlx::query("UPDATE job SET fts_rowid = ?1 WHERE id = ?2")
                .bind(next)
                .bind(job_id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            next
        }
    };

    let row = sqlx::query(
        "SELECT j.title, c.name AS company_name, j.description_text,
                COALESCE((SELECT GROUP_CONCAT(r.text, ' ') FROM requirement r
                           WHERE r.job_id = j.id), '') AS requirements_text
           FROM job j JOIN company c ON c.id = j.company_id
          WHERE j.id = ?1",
    )
    .bind(job_id.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;

    let Some(row) = row else {
        tx.commit().await.map_err(db_err)?;
        return Ok(());
    };

    sqlx::query(
        "INSERT INTO job_fts (rowid, title, company_name, description_text, requirements_text)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(rowid)
    .bind(row.try_get::<String, _>("title").map_err(db_err)?)
    .bind(row.try_get::<String, _>("company_name").map_err(db_err)?)
    .bind(
        row.try_get::<String, _>("description_text")
            .map_err(db_err)?,
    )
    .bind(
        row.try_get::<String, _>("requirements_text")
            .map_err(db_err)?,
    )
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;

    tx.commit().await.map_err(db_err)?;
    Ok(())
}

/// Where one stored field came from, so the UI can show a confidence dot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FieldProvenanceRow {
    pub field: String,
    pub provenance: String,
    pub confidence: f64,
}

/// One requirement as the job-detail page renders it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RequirementRow {
    pub id: String,
    pub text: String,
    pub normalized_text: String,
    pub kind: String,
    pub necessity: String,
    pub min_years: Option<f64>,
    pub is_blocker: bool,
}

/// Full job record for the detail view and the CLI `show` command.
#[derive(Debug, Clone, serde::Serialize)]
pub struct JobDetail {
    pub id: String,
    pub company_id: String,
    pub company_name: String,
    pub company_slug: String,
    pub title: String,
    pub status: String,
    pub work_mode: String,
    pub seniority: String,
    pub employment_type: String,
    pub salary_min_cents: Option<i64>,
    pub salary_max_cents: Option<i64>,
    pub salary_currency: Option<String>,
    pub salary_period: String,
    pub salary_is_estimate: bool,
    pub salary_raw: Option<String>,
    pub posted_at: Option<String>,
    pub closes_at: Option<String>,
    pub apply_url: Option<String>,
    pub description_md: String,
    pub file_path: Option<String>,
    pub content_hash: String,
    pub extraction_partial: bool,
    pub extraction_model: Option<String>,
    pub locations: Vec<String>,
    pub requirements: Vec<RequirementRow>,
    pub requires_clearance: Option<String>,
    pub user_rating: Option<i64>,
    pub user_notes_md: Option<String>,
    pub is_archived: bool,
    #[serde(default)]
    pub listings: Vec<ListingBrief>,
    pub provenance: Vec<FieldProvenanceRow>,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListingBrief {
    pub id: String,
    pub url: String,
    pub source: String,
    pub is_canonical: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MergeReport {
    pub from_id: String,
    pub into_id: String,
    pub listings_moved: u64,
    pub requirements_added: u64,
}

pub async fn get(db: &Db, id: &JobId) -> Result<Option<JobDetail>> {
    let row = sqlx::query(
        "SELECT j.id, j.company_id, c.name AS company_name, c.slug AS company_slug,
                j.title, j.status, j.work_mode, j.seniority, j.employment_type,
                j.salary_min_cents, j.salary_max_cents, j.salary_currency, j.salary_period,
                j.salary_is_estimate, j.salary_raw, j.posted_at, j.closes_at, j.apply_url,
                j.description_md, j.file_path, j.content_hash, j.extraction_partial,
                j.extraction_model, j.requires_clearance, j.user_rating, j.user_notes_md,
                j.is_archived, j.updated_at
           FROM job j
           JOIN company c ON c.id = j.company_id
          WHERE j.id = ?1 AND j.deleted_at IS NULL",
    )
    .bind(id.as_str())
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    let Some(row) = row else { return Ok(None) };

    let locations: Vec<String> = sqlx::query_scalar(
        "SELECT raw FROM job_location WHERE job_id = ?1 ORDER BY is_primary DESC, ordinal ASC",
    )
    .bind(id.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;

    let req_rows = sqlx::query(
        "SELECT id, text, normalized_text, kind, necessity, min_years, is_blocker
           FROM requirement WHERE job_id = ?1 ORDER BY ordinal ASC",
    )
    .bind(id.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    let mut requirements = Vec::with_capacity(req_rows.len());
    for r in req_rows {
        requirements.push(RequirementRow {
            id: r.try_get("id").map_err(db_err)?,
            text: r.try_get("text").map_err(db_err)?,
            normalized_text: r.try_get("normalized_text").map_err(db_err)?,
            kind: r.try_get("kind").map_err(db_err)?,
            necessity: r.try_get("necessity").map_err(db_err)?,
            min_years: r.try_get("min_years").map_err(db_err)?,
            is_blocker: r.try_get::<i64, _>("is_blocker").map_err(db_err)? != 0,
        });
    }

    Ok(Some(JobDetail {
        id: row.try_get("id").map_err(db_err)?,
        company_id: row.try_get("company_id").map_err(db_err)?,
        company_name: row.try_get("company_name").map_err(db_err)?,
        company_slug: row.try_get("company_slug").map_err(db_err)?,
        title: row.try_get("title").map_err(db_err)?,
        status: row.try_get("status").map_err(db_err)?,
        work_mode: row.try_get("work_mode").map_err(db_err)?,
        seniority: row.try_get("seniority").map_err(db_err)?,
        employment_type: row.try_get("employment_type").map_err(db_err)?,
        salary_min_cents: row.try_get("salary_min_cents").map_err(db_err)?,
        salary_max_cents: row.try_get("salary_max_cents").map_err(db_err)?,
        salary_currency: row.try_get("salary_currency").map_err(db_err)?,
        salary_period: row.try_get("salary_period").map_err(db_err)?,
        salary_is_estimate: row
            .try_get::<i64, _>("salary_is_estimate")
            .map_err(db_err)?
            != 0,
        salary_raw: row.try_get("salary_raw").map_err(db_err)?,
        posted_at: row.try_get("posted_at").map_err(db_err)?,
        closes_at: row.try_get("closes_at").map_err(db_err)?,
        apply_url: row.try_get("apply_url").map_err(db_err)?,
        description_md: row.try_get("description_md").map_err(db_err)?,
        file_path: row.try_get("file_path").map_err(db_err)?,
        content_hash: row.try_get("content_hash").map_err(db_err)?,
        extraction_partial: row
            .try_get::<i64, _>("extraction_partial")
            .map_err(db_err)?
            != 0,
        extraction_model: row.try_get("extraction_model").map_err(db_err)?,
        locations,
        requirements,
        listings: load_listings(db, id).await?,
        requires_clearance: row.try_get("requires_clearance").map_err(db_err)?,
        user_rating: row.try_get("user_rating").map_err(db_err)?,
        user_notes_md: row.try_get("user_notes_md").map_err(db_err)?,
        is_archived: row.try_get::<i64, _>("is_archived").map_err(db_err)? != 0,
        provenance: load_provenance(db, id).await?,
        updated_at: row.try_get("updated_at").map_err(db_err)?,
    }))
}

async fn load_listings(db: &Db, id: &JobId) -> Result<Vec<ListingBrief>> {
    let rows = sqlx::query(
        "SELECT l.id, l.url, l.is_canonical, s.kind
           FROM job_source_listing l
           JOIN source s ON s.id = l.source_id
          WHERE l.job_id = ?1
          ORDER BY l.is_canonical DESC, s.fidelity DESC, l.first_seen_at ASC",
    )
    .bind(id.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(ListingBrief {
            id: r.try_get("id").map_err(db_err)?,
            url: r.try_get("url").map_err(db_err)?,
            source: r.try_get("kind").map_err(db_err)?,
            is_canonical: r.try_get::<i64, _>("is_canonical").map_err(db_err)? != 0,
        });
    }
    Ok(out)
}

/// Attach `from`'s listings onto `into`, union requirements by normalized text,
/// then soft-delete `from`. The keeper (`into`) keeps its title and description.
/// User-initiated — never silent. Same company required.
pub async fn merge(db: &Db, from: &JobId, into: &JobId) -> Result<MergeReport> {
    if from == into {
        return Err(jobseeker_core::Error::BadRequest(
            "cannot merge a job into itself".into(),
        ));
    }
    let from_row = get(db, from)
        .await?
        .ok_or(jobseeker_core::Error::NotFound("job"))?;
    let into_row = get(db, into)
        .await?
        .ok_or(jobseeker_core::Error::NotFound("job"))?;
    if from_row.company_id != into_row.company_id {
        return Err(jobseeker_core::Error::BadRequest(
            "merge requires the same company".into(),
        ));
    }

    let listing_ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM job_source_listing WHERE job_id = ?1",
    )
    .bind(from.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;

    let mut listings_moved = 0u64;
    for lid in &listing_ids {
        let listing_id: ListingId = lid.parse()?;
        crate::repo::listing::attach_to_job(db, &listing_id, into).await?;
        listings_moved += 1;
    }

    let existing_norm: Vec<String> = sqlx::query_scalar(
        "SELECT normalized_text FROM requirement WHERE job_id = ?1",
    )
    .bind(into.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    let existing: std::collections::HashSet<String> = existing_norm.into_iter().collect();
    let donor_reqs = sqlx::query(
        "SELECT text, normalized_text, kind, necessity, min_years, is_blocker, confidence, provenance
           FROM requirement WHERE job_id = ?1 ORDER BY ordinal ASC",
    )
    .bind(from.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    let mut next_ord: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM requirement WHERE job_id = ?1",
    )
    .bind(into.as_str())
    .fetch_one(db.reader())
    .await
    .map_err(db_err)?;
    let ts = jobseeker_core::time::to_rfc3339(&jobseeker_core::time::now());
    let mut requirements_added = 0u64;
    for r in donor_reqs {
        let norm: String = r.try_get("normalized_text").map_err(db_err)?;
        if existing.contains(&norm) {
            continue;
        }
        let rid = uuid::Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO requirement (id, job_id, text, normalized_text, kind, necessity,
                                      min_years, is_blocker, ordinal, confidence, provenance,
                                      created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
        )
        .bind(&rid)
        .bind(into.as_str())
        .bind(r.try_get::<String, _>("text").map_err(db_err)?)
        .bind(&norm)
        .bind(r.try_get::<String, _>("kind").map_err(db_err)?)
        .bind(r.try_get::<String, _>("necessity").map_err(db_err)?)
        .bind(r.try_get::<Option<f64>, _>("min_years").map_err(db_err)?)
        .bind(r.try_get::<i64, _>("is_blocker").map_err(db_err)?)
        .bind(next_ord)
        .bind(r.try_get::<f64, _>("confidence").unwrap_or(0.5))
        .bind(r.try_get::<String, _>("provenance").unwrap_or_else(|_| "rules".into()))
        .bind(&ts)
        .execute(db.writer())
        .await
        .map_err(db_err)?;
        next_ord += 1;
        requirements_added += 1;
    }

    let into_locs: Vec<String> =
        sqlx::query_scalar("SELECT raw FROM job_location WHERE job_id = ?1")
            .bind(into.as_str())
            .fetch_all(db.reader())
            .await
            .map_err(db_err)?;
    let into_loc_set: std::collections::HashSet<String> = into_locs.into_iter().collect();
    let donor_locs = sqlx::query(
        "SELECT raw, city, region, country, postal_code, is_primary, is_remote_scope,
                timezone_requirement
           FROM job_location WHERE job_id = ?1 ORDER BY ordinal ASC",
    )
    .bind(from.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    let mut loc_ord: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(ordinal), -1) + 1 FROM job_location WHERE job_id = ?1",
    )
    .bind(into.as_str())
    .fetch_one(db.reader())
    .await
    .map_err(db_err)?;
    for r in donor_locs {
        let raw: String = r.try_get("raw").map_err(db_err)?;
        if into_loc_set.contains(&raw) {
            continue;
        }
        let lid = uuid::Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO job_location (id, job_id, raw, city, region, country, postal_code,
                                       is_primary, is_remote_scope, timezone_requirement, ordinal)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?10)",
        )
        .bind(&lid)
        .bind(into.as_str())
        .bind(&raw)
        .bind(r.try_get::<Option<String>, _>("city").map_err(db_err)?)
        .bind(r.try_get::<Option<String>, _>("region").map_err(db_err)?)
        .bind(r.try_get::<Option<String>, _>("country").map_err(db_err)?)
        .bind(r.try_get::<Option<String>, _>("postal_code").map_err(db_err)?)
        .bind(r.try_get::<i64, _>("is_remote_scope").map_err(db_err)?)
        .bind(r.try_get::<Option<String>, _>("timezone_requirement").map_err(db_err)?)
        .bind(loc_ord)
        .execute(db.writer())
        .await
        .map_err(db_err)?;
        loc_ord += 1;
    }

    if into_row.apply_url.is_none() && from_row.apply_url.is_some() {
        sqlx::query("UPDATE job SET apply_url = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(from_row.apply_url.as_deref())
            .bind(&ts)
            .bind(into.as_str())
            .execute(db.writer())
            .await
            .map_err(db_err)?;
    }
    if into_row.salary_raw.is_none() && from_row.salary_raw.is_some() {
        sqlx::query(
            "UPDATE job SET salary_raw = ?1, salary_min_cents = ?2, salary_max_cents = ?3,
                            salary_currency = ?4, salary_period = ?5, salary_is_estimate = ?6,
                            updated_at = ?7
              WHERE id = ?8",
        )
        .bind(&from_row.salary_raw)
        .bind(from_row.salary_min_cents)
        .bind(from_row.salary_max_cents)
        .bind(&from_row.salary_currency)
        .bind(&from_row.salary_period)
        .bind(i64::from(from_row.salary_is_estimate))
        .bind(&ts)
        .bind(into.as_str())
        .execute(db.writer())
        .await
        .map_err(db_err)?;
    } else if let (Some(a), Some(b)) = (&into_row.salary_raw, &from_row.salary_raw) {
        if a != b {
            let cid = uuid::Uuid::now_v7().to_string();
            sqlx::query(
                "INSERT INTO extraction_conflict
                    (id, job_id, field, value_a, provenance_a, value_b, provenance_b,
                     resolution, created_at)
                 VALUES (?1, ?2, 'salary', ?3, 'keeper', ?4, 'merged', 'unresolved', ?5)",
            )
            .bind(&cid)
            .bind(into.as_str())
            .bind(a)
            .bind(b)
            .bind(&ts)
            .execute(db.writer())
            .await
            .map_err(db_err)?;
        }
    }

    sqlx::query("UPDATE job SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2")
        .bind(&ts)
        .bind(from.as_str())
        .execute(db.writer())
        .await
        .map_err(db_err)?;
    sqlx::query(
        "INSERT OR IGNORE INTO tombstone (entity_kind, entity_id, deleted_at, reason)
         VALUES ('job', ?1, ?2, ?3)",
    )
    .bind(from.as_str())
    .bind(&ts)
    .bind(format!("merged_into:{into}"))
    .execute(db.writer())
    .await
    .map_err(db_err)?;

    reindex_fts(db, into).await?;
    Ok(MergeReport {
        from_id: from.as_str().to_string(),
        into_id: into.as_str().to_string(),
        listings_moved,
        requirements_added,
    })
}

async fn load_provenance(db: &Db, id: &JobId) -> Result<Vec<FieldProvenanceRow>> {
    let rows = sqlx::query(
        "SELECT field, provenance, confidence FROM field_provenance
          WHERE entity_kind = 'job' AND entity_id = ?1
          ORDER BY field ASC",
    )
    .bind(id.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(FieldProvenanceRow {
            field: r.try_get("field").map_err(db_err)?,
            provenance: r.try_get("provenance").map_err(db_err)?,
            confidence: r.try_get("confidence").map_err(db_err)?,
        });
    }
    Ok(out)
}

/// Job + requirements in domain types, for the matcher.
pub async fn scoring_inputs(
    db: &Db,
    id: &JobId,
) -> Result<
    Option<(
        jobseeker_core::domain::job::Job,
        Vec<jobseeker_core::domain::requirement::Requirement>,
        Vec<jobseeker_core::domain::location::JobLocation>,
    )>,
> {
    let detail = match get(db, id).await? {
        Some(d) => d,
        None => return Ok(None),
    };
    let salary = jobseeker_core::domain::salary::Salary {
        min_cents: detail.salary_min_cents,
        max_cents: detail.salary_max_cents,
        currency: detail
            .salary_currency
            .clone()
            .unwrap_or_else(|| "USD".into()),
        period: detail
            .salary_period
            .parse()
            .unwrap_or(jobseeker_core::domain::salary::SalaryPeriod::Unknown),
        is_estimate: detail.salary_is_estimate,
        raw: detail.salary_raw.clone(),
    };

    let loc_rows = sqlx::query(
        "SELECT raw, city, region, country, is_primary, is_remote_scope, ordinal
           FROM job_location WHERE job_id = ?1 ORDER BY ordinal ASC",
    )
    .bind(id.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    let mut locations = Vec::new();
    for r in loc_rows {
        locations.push(jobseeker_core::domain::location::JobLocation {
            job_id: id.clone(),
            raw: r.try_get("raw").map_err(db_err)?,
            city: r.try_get("city").map_err(db_err)?,
            region: r.try_get("region").map_err(db_err)?,
            country: r.try_get("country").map_err(db_err)?,
            postal_code: None,
            lat: None,
            lon: None,
            is_primary: r.try_get::<i64, _>("is_primary").map_err(db_err)? != 0,
            is_remote_scope: r.try_get::<i64, _>("is_remote_scope").map_err(db_err)? != 0,
            timezone_requirement: None,
            ordinal: r.try_get("ordinal").map_err(db_err)?,
        });
    }

    let now = jobseeker_core::time::now();
    let req_rows = sqlx::query(
        "SELECT id, ordinal, text, normalized_text, kind, necessity, min_years, max_years,
                is_blocker, quantity_raw, confidence, provenance
           FROM requirement WHERE job_id = ?1 ORDER BY ordinal ASC",
    )
    .bind(id.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    let mut requirements = Vec::new();
    for r in req_rows {
        let kind: String = r.try_get("kind").map_err(db_err)?;
        let necessity: String = r.try_get("necessity").map_err(db_err)?;
        let provenance: String = r.try_get("provenance").map_err(db_err)?;
        let conf: f64 = r.try_get("confidence").map_err(db_err)?;
        requirements.push(jobseeker_core::domain::requirement::Requirement {
            id: r.try_get::<String, _>("id").map_err(db_err)?.parse()?,
            job_id: id.clone(),
            ordinal: r.try_get("ordinal").map_err(db_err)?,
            text: r.try_get("text").map_err(db_err)?,
            normalized_text: r.try_get("normalized_text").map_err(db_err)?,
            kind: kind.parse()?,
            necessity: necessity.parse()?,
            skill_id: None,
            min_years: r
                .try_get::<Option<f64>, _>("min_years")
                .map_err(db_err)?
                .map(|y| y as f32),
            max_years: r
                .try_get::<Option<f64>, _>("max_years")
                .map_err(db_err)?
                .map(|y| y as f32),
            level: None,
            education_level: None,
            field_of_study: None,
            is_blocker: r.try_get::<i64, _>("is_blocker").map_err(db_err)? != 0,
            quantity_raw: r.try_get("quantity_raw").map_err(db_err)?,
            source_span: None,
            confidence: jobseeker_core::provenance::Confidence::new(conf as f32),
            provenance: provenance.parse()?,
            created_at: now,
            updated_at: now,
        });
    }

    // A thin Job is enough for the snapshot fields the scorer reads.
    let job = jobseeker_core::domain::job::Job {
        id: id.clone(),
        company_id: detail.company_id.parse()?,
        slug: String::new(),
        title: detail.title,
        title_normalized: String::new(),
        seniority: detail.seniority.parse()?,
        employment_type: detail.employment_type.parse()?,
        work_mode: detail.work_mode.parse()?,
        work_mode_detail: None,
        department: None,
        team: None,
        description_md: detail.description_md,
        description_text: String::new(),
        summary: None,
        responsibilities_md: None,
        benefits_md: None,
        salary,
        equity_offered: false,
        comp_notes: None,
        posted_at: None,
        closes_at: None,
        first_seen_at: now,
        last_seen_at: now,
        closed_at: None,
        status: detail.status.parse()?,
        apply_url: detail.apply_url,
        apply_kind: jobseeker_core::domain::enums::ApplyKind::Unknown,
        canonical_listing_id: None,
        requires_clearance: detail.requires_clearance,
        visa_sponsorship: jobseeker_core::domain::enums::Tristate::Unspecified,
        travel_pct: None,
        education_min: jobseeker_core::domain::enums::EducationLevel::Unknown,
        years_experience_min: None,
        years_experience_max: None,
        content_hash: detail.content_hash,
        extraction_model: None,
        extracted_at: None,
        extraction_confidence: None,
        extraction_partial: detail.extraction_partial,
        file_path: detail.file_path,
        user_rating: None,
        user_notes_md: None,
        is_archived: false,
        created_at: now,
        updated_at: now,
    };
    Ok(Some((job, requirements, locations)))
}

#[derive(Debug, Clone, Default)]
pub struct JobPatch {
    pub title: Option<String>,
    pub user_rating: Option<i32>,
    pub user_notes_md: Option<String>,
    pub is_archived: Option<bool>,
    pub status: Option<String>,
}

/// User edits. Fields written here become provenance `manual`.
pub async fn patch(db: &Db, id: &JobId, patch: &JobPatch) -> Result<u64> {
    let ts = jobseeker_core::time::to_rfc3339(&jobseeker_core::time::now());
    if let Some(title) = &patch.title {
        let slug = jobseeker_core::slug::slugify(title);
        let title_normalized = jobseeker_core::slug::normalize_job_title(title);
        sqlx::query(
            "UPDATE job SET title = ?2, slug = ?3, title_normalized = ?4, updated_at = ?5
              WHERE id = ?1 AND deleted_at IS NULL",
        )
        .bind(id.as_str())
        .bind(title)
        .bind(&slug)
        .bind(&title_normalized)
        .bind(&ts)
        .execute(db.writer())
        .await
        .map_err(db_err)?;
    }
    let result = sqlx::query(
        r#"UPDATE job SET
            user_rating = COALESCE(?2, user_rating),
            user_notes_md = COALESCE(?3, user_notes_md),
            is_archived = COALESCE(?4, is_archived),
            status = COALESCE(?5, status),
            updated_at = ?6
          WHERE id = ?1 AND deleted_at IS NULL"#,
    )
    .bind(id.as_str())
    .bind(patch.user_rating.map(i64::from))
    .bind(patch.user_notes_md.as_deref())
    .bind(patch.is_archived.map(i64::from))
    .bind(patch.status.as_deref())
    .bind(&ts)
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    let touched_any = result.rows_affected() > 0 || patch.title.is_some();
    if touched_any {
        for field in [
            "title",
            "user_rating",
            "user_notes_md",
            "is_archived",
            "status",
        ] {
            let touched = match field {
                "title" => patch.title.is_some(),
                "user_rating" => patch.user_rating.is_some(),
                "user_notes_md" => patch.user_notes_md.is_some(),
                "is_archived" => patch.is_archived.is_some(),
                "status" => patch.status.is_some(),
                _ => false,
            };
            if !touched {
                continue;
            }
            let pid = uuid::Uuid::now_v7().to_string();
            sqlx::query(
                r#"INSERT INTO field_provenance
                    (id, entity_kind, entity_id, field, provenance, confidence, updated_at)
                   VALUES (?1, 'job', ?2, ?3, 'manual', 1.0, ?4)
                   ON CONFLICT (entity_kind, entity_id, field) DO UPDATE SET
                     provenance = 'manual', confidence = 1.0, updated_at = excluded.updated_at"#,
            )
            .bind(&pid)
            .bind(id.as_str())
            .bind(field)
            .bind(&ts)
            .execute(db.writer())
            .await
            .map_err(db_err)?;
        }
    }
    Ok(result.rows_affected())
}

/// Mark every score for this job stale. A cheap UPDATE beats deleting rows: the UI can keep
/// showing the last known value while a recompute is queued (FR-M-03).
pub async fn mark_scores_stale(db: &Db, job_id: &JobId) -> Result<u64> {
    let result = sqlx::query("UPDATE match_score SET is_stale = 1 WHERE job_id = ?1")
        .bind(job_id.as_str())
        .execute(db.writer())
        .await
        .map_err(db_err)?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::company;
    use jobseeker_core::domain::salary::SalaryPeriod;

    /// Insert a job directly; the full write path lives in `jobseeker-pipeline`, and these
    /// tests are about query behaviour.
    #[allow(clippy::too_many_arguments)]
    async fn insert_job(
        db: &Db,
        company_name: &str,
        title: &str,
        status: JobStatus,
        work_mode: WorkMode,
        salary_max: Option<i64>,
        period: SalaryPeriod,
        posted_at: &str,
    ) -> JobId {
        let company_id = company::resolve_or_create(db, company_name, None)
            .await
            .unwrap();
        let id = JobId::new();
        sqlx::query(
            "INSERT INTO job (id, company_id, slug, title, title_normalized, status, work_mode,
                              salary_max_cents, salary_period, posted_at, description_text,
                              content_hash, first_seen_at, last_seen_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'h', ?10, ?10, ?10, ?10)",
        )
        .bind(id.as_str())
        .bind(company_id.as_str())
        .bind(jobseeker_core::slug::slugify(title))
        .bind(title)
        .bind(jobseeker_core::slug::normalize_job_title(title))
        .bind(status.as_str())
        .bind(work_mode.as_str())
        .bind(salary_max)
        .bind(period.as_str())
        .bind(posted_at)
        .bind(format!("Description for {title}"))
        .execute(db.writer())
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn lists_newest_first_by_default() {
        let db = Db::open_in_memory().await.unwrap();
        insert_job(
            &db,
            "Acme",
            "Old Role",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-01-01T00:00:00Z",
        )
        .await;
        insert_job(
            &db,
            "Acme",
            "New Role",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-01T00:00:00Z",
        )
        .await;

        let page = list(&db, &JobFilter::default()).await.unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].title, "New Role");
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn filters_compose() {
        let db = Db::open_in_memory().await.unwrap();
        insert_job(
            &db,
            "Acme",
            "Remote Open",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-01T00:00:00Z",
        )
        .await;
        insert_job(
            &db,
            "Acme",
            "Onsite Open",
            JobStatus::Open,
            WorkMode::Onsite,
            None,
            SalaryPeriod::Year,
            "2026-09-02T00:00:00Z",
        )
        .await;
        insert_job(
            &db,
            "Acme",
            "Remote Closed",
            JobStatus::Closed,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-03T00:00:00Z",
        )
        .await;

        let page = list(
            &db,
            &JobFilter {
                status: Some(JobStatus::Open),
                work_mode: Some(WorkMode::Remote),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].title, "Remote Open");
    }

    #[tokio::test]
    async fn salary_filter_annualizes_hourly_rates() {
        let db = Db::open_in_memory().await.unwrap();
        // $90/hr annualizes to $187,200 — above a $150k floor.
        insert_job(
            &db,
            "Acme",
            "Contract",
            JobStatus::Open,
            WorkMode::Remote,
            Some(90_00),
            SalaryPeriod::Hour,
            "2026-09-01T00:00:00Z",
        )
        .await;
        // $120k/yr is below it.
        insert_job(
            &db,
            "Acme",
            "Salaried",
            JobStatus::Open,
            WorkMode::Remote,
            Some(120_000_00),
            SalaryPeriod::Year,
            "2026-09-02T00:00:00Z",
        )
        .await;

        let page = list(
            &db,
            &JobFilter {
                min_salary_cents: Some(150_000_00),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].title, "Contract");
    }

    #[tokio::test]
    async fn archived_jobs_are_hidden_unless_asked_for() {
        let db = Db::open_in_memory().await.unwrap();
        let id = insert_job(
            &db,
            "Acme",
            "Archived",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-01T00:00:00Z",
        )
        .await;
        sqlx::query("UPDATE job SET is_archived = 1 WHERE id = ?1")
            .bind(id.as_str())
            .execute(db.writer())
            .await
            .unwrap();

        assert!(list(&db, &JobFilter::default())
            .await
            .unwrap()
            .items
            .is_empty());
        let with_archived = list(
            &db,
            &JobFilter {
                include_archived: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(with_archived.items.len(), 1);
    }

    #[tokio::test]
    async fn keyset_pagination_walks_every_row_exactly_once() {
        let db = Db::open_in_memory().await.unwrap();
        for day in 1..=7 {
            insert_job(
                &db,
                "Acme",
                &format!("Role {day}"),
                JobStatus::Open,
                WorkMode::Remote,
                None,
                SalaryPeriod::Year,
                &format!("2026-09-0{day}T00:00:00Z"),
            )
            .await;
        }

        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = list(
                &db,
                &JobFilter {
                    limit: Some(3),
                    cursor: cursor.clone(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            seen.extend(page.items.iter().map(|j| j.id.clone()));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }

        assert_eq!(seen.len(), 7, "every row exactly once");
        let unique: std::collections::HashSet<_> = seen.iter().collect();
        assert_eq!(unique.len(), 7, "no duplicates across pages");
    }

    #[tokio::test]
    async fn full_text_search_finds_requirement_text() {
        let db = Db::open_in_memory().await.unwrap();
        let id = insert_job(
            &db,
            "Acme",
            "Platform Engineer",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-01T00:00:00Z",
        )
        .await;
        sqlx::query(
            "INSERT INTO requirement (id, job_id, text, normalized_text, kind, necessity,
                                      created_at, updated_at)
             VALUES ('r1', ?1, '3+ years operating Kubernetes', 'years operating kubernetes',
                     'skill', 'required', 'now', 'now')",
        )
        .bind(id.as_str())
        .execute(db.writer())
        .await
        .unwrap();
        reindex_fts(&db, &id).await.unwrap();

        let page = list(
            &db,
            &JobFilter {
                query: Some("kubernetes".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(page.items.len(), 1, "requirement text must be searchable");
        assert_eq!(page.items[0].title, "Platform Engineer");
    }

    #[tokio::test]
    async fn reindexing_twice_does_not_duplicate_hits() {
        let db = Db::open_in_memory().await.unwrap();
        let id = insert_job(
            &db,
            "Acme",
            "Rust Engineer",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-01T00:00:00Z",
        )
        .await;
        reindex_fts(&db, &id).await.unwrap();
        reindex_fts(&db, &id).await.unwrap();

        let page = list(
            &db,
            &JobFilter {
                query: Some("rust".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(page.items.len(), 1, "reindex must replace, not append");
    }

    #[tokio::test]
    async fn status_counts_exclude_archived_and_deleted() {
        let db = Db::open_in_memory().await.unwrap();
        insert_job(
            &db,
            "Acme",
            "A",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-01T00:00:00Z",
        )
        .await;
        insert_job(
            &db,
            "Acme",
            "B",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-02T00:00:00Z",
        )
        .await;
        let closed = insert_job(
            &db,
            "Acme",
            "C",
            JobStatus::Closed,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-03T00:00:00Z",
        )
        .await;
        let archived = insert_job(
            &db,
            "Acme",
            "D",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-04T00:00:00Z",
        )
        .await;
        sqlx::query("UPDATE job SET is_archived = 1 WHERE id = ?1")
            .bind(archived.as_str())
            .execute(db.writer())
            .await
            .unwrap();

        let counts: std::collections::HashMap<_, _> =
            count_by_status(&db).await.unwrap().into_iter().collect();
        assert_eq!(counts.get("open"), Some(&2));
        assert_eq!(counts.get("closed"), Some(&1));
        assert!(closed.as_str().len() > 0);
    }

    #[tokio::test]
    async fn marking_scores_stale_preserves_the_last_known_value() {
        let db = Db::open_in_memory().await.unwrap();
        let job = insert_job(
            &db,
            "Acme",
            "Role",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-01T00:00:00Z",
        )
        .await;
        sqlx::query(
            "INSERT INTO profile (id, name, created_at, updated_at)
             VALUES ('p1', 'default', 'now', 'now');
             INSERT INTO match_score (id, job_id, profile_id, algorithm_version, overall,
                                      weights_json, inputs_hash, computed_at)
             VALUES ('m1', ?1, 'p1', '1.0.0', 0.81, '{}', 'h', 'now')",
        )
        .bind(job.as_str())
        .execute(db.writer())
        .await
        .unwrap();

        assert_eq!(mark_scores_stale(&db, &job).await.unwrap(), 1);
        let (overall, stale): (f64, i64) =
            sqlx::query_as("SELECT overall, is_stale FROM match_score WHERE id = 'm1'")
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(stale, 1);
        assert!(
            (overall - 0.81).abs() < 1e-9,
            "the value must survive staleness"
        );
    }

    #[tokio::test]
    async fn list_attaches_skills_and_years_from_verdicts() {
        let db = Db::open_in_memory().await.unwrap();
        let job = insert_job(
            &db,
            "Acme",
            "Scientist",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-01T00:00:00Z",
        )
        .await;
        sqlx::query(
            "INSERT INTO profile (id, name, created_at, updated_at)
             VALUES ('p1', 'default', 'now', 'now');
             INSERT INTO requirement (id, job_id, text, normalized_text, kind, necessity,
                                      created_at, updated_at)
             VALUES ('r-years', ?1, '5+ years data science', 'years data science',
                     'skill', 'required', 'now', 'now'),
                    ('r-pref', ?1, 'Nice PhD', 'nice phd',
                     'education', 'preferred', 'now', 'now');
             INSERT INTO match_score (id, job_id, profile_id, algorithm_version, overall,
                                      weights_json, inputs_hash, computed_at)
             VALUES ('m1', ?1, 'p1', '1.0.0', 0.52, '{}', 'h', 'now');
             INSERT INTO requirement_match (id, match_score_id, requirement_id, status, score,
                                            weight, years_have, years_needed, rationale, created_at)
             VALUES ('rm1', 'm1', 'r-years', 'gap', 0.2, 1.0, 1.0, 5.0, '1 of 5', 'now'),
                    ('rm2', 'm1', 'r-pref', 'gap', 0.0, 1.0, NULL, NULL, 'no PhD', 'now')",
        )
        .bind(job.as_str())
        .execute(db.writer())
        .await
        .unwrap();

        let page = list(&db, &JobFilter::default()).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert!((page.items[0].match_overall.unwrap() - 0.52).abs() < 1e-6);
        assert!(
            (page.items[0].skills_coverage.unwrap() - 1.0).abs() < 1e-6,
            "year-bar with years_have is a skill hit: {:?}",
            page.items[0].skills_coverage
        );
        assert!(
            (page.items[0].years_fit.unwrap() - 0.2).abs() < 1e-6,
            "1/5 years: {:?}",
            page.items[0].years_fit
        );
    }

    #[tokio::test]
    async fn list_derives_breakdown_from_explanation_when_verdicts_missing() {
        let db = Db::open_in_memory().await.unwrap();
        let job = insert_job(
            &db,
            "Acme",
            "Thin Capture",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-01T00:00:00Z",
        )
        .await;
        let explanation = serde_json::json!([{
            "requirement_id": jobseeker_core::ids::RequirementId::new(),
            "status": "gap",
            "score": 0.2,
            "weight": 1.0,
            "evidence": [],
            "years_have": 1.0,
            "years_needed": 5.0,
            "rationale": "1 of 5"
        }])
        .to_string();
        sqlx::query(
            "INSERT INTO profile (id, name, created_at, updated_at)
             VALUES ('p1', 'default', 'now', 'now');
             INSERT INTO match_score (id, job_id, profile_id, algorithm_version, overall,
                                      weights_json, explanation_json, inputs_hash, computed_at)
             VALUES ('m1', ?1, 'p1', '1.0.0', 0.74, '{}', ?2, 'h', 'now')",
        )
        .bind(job.as_str())
        .bind(&explanation)
        .execute(db.writer())
        .await
        .unwrap();

        let page = list(&db, &JobFilter::default()).await.unwrap();
        assert!((page.items[0].match_overall.unwrap() - 0.74).abs() < 1e-6);
        assert!((page.items[0].skills_coverage.unwrap() - 1.0).abs() < 1e-6);
        assert!((page.items[0].years_fit.unwrap() - 0.2).abs() < 1e-6);
    }

    #[tokio::test]
    async fn merge_moves_listings_and_unions_requirements() {
        let db = Db::open_in_memory().await.unwrap();
        let keeper = insert_job(
            &db,
            "Zillow",
            "Data Scientist",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-01T00:00:00Z",
        )
        .await;
        let donor = insert_job(
            &db,
            "Zillow",
            "Data Scientist",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-02T00:00:00Z",
        )
        .await;
        sqlx::query(
            "INSERT INTO requirement (id, job_id, text, normalized_text, kind, necessity,
                                      created_at, updated_at)
             VALUES ('rk', ?1, '5+ years data science', 'years data science',
                     'skill', 'required', 'now', 'now'),
                    ('rd1', ?2, '5+ years data science', 'years data science',
                     'skill', 'required', 'now', 'now'),
                    ('rd2', ?2, 'Python', 'python',
                     'skill', 'required', 'now', 'now')",
        )
        .bind(keeper.as_str())
        .bind(donor.as_str())
        .execute(db.writer())
        .await
        .unwrap();
        let keep_listing = crate::repo::listing::upsert_by_url(
            &db,
            &crate::repo::listing::UpsertListing {
                source: jobseeker_core::domain::enums::SourceKind::Workday,
                url: "https://zillow.wd5.example/job/P1".into(),
                url_canonical: "https://zillow.wd5.example/job/P1".into(),
                source_job_id: Some("P1".into()),
                title_at_source: Some("Data Scientist".into()),
                company_name_at_source: Some("Zillow".into()),
            },
        )
        .await
        .unwrap()
        .0;
        let donor_listing = crate::repo::listing::upsert_by_url(
            &db,
            &crate::repo::listing::UpsertListing {
                source: jobseeker_core::domain::enums::SourceKind::LinkedIn,
                url: "https://www.linkedin.com/jobs/view/1".into(),
                url_canonical: "https://www.linkedin.com/jobs/view/1".into(),
                source_job_id: Some("1".into()),
                title_at_source: Some("Data Scientist".into()),
                company_name_at_source: Some("Zillow".into()),
            },
        )
        .await
        .unwrap()
        .0;
        crate::repo::listing::attach_to_job(&db, &keep_listing, &keeper)
            .await
            .unwrap();
        crate::repo::listing::attach_to_job(&db, &donor_listing, &donor)
            .await
            .unwrap();

        let report = merge(&db, &donor, &keeper).await.unwrap();
        assert_eq!(report.listings_moved, 1);
        assert_eq!(report.requirements_added, 1);

        assert!(get(&db, &donor).await.unwrap().is_none());
        let kept = get(&db, &keeper).await.unwrap().unwrap();
        assert_eq!(kept.listings.len(), 2);
        assert!(kept.listings.iter().any(|l| l.source == "workday"));
        assert!(kept.listings.iter().any(|l| l.source == "linkedin"));
        assert_eq!(kept.requirements.len(), 2);
        let page = list(&db, &JobFilter::default()).await.unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, keeper.as_str());
    }

    #[tokio::test]
    async fn merge_refuses_a_different_company() {
        let db = Db::open_in_memory().await.unwrap();
        let a = insert_job(
            &db,
            "Zillow",
            "Data Scientist",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-01T00:00:00Z",
        )
        .await;
        let b = insert_job(
            &db,
            "Harbor",
            "Data Scientist",
            JobStatus::Open,
            WorkMode::Remote,
            None,
            SalaryPeriod::Year,
            "2026-09-02T00:00:00Z",
        )
        .await;
        assert!(merge(&db, &a, &b).await.is_err());
    }
}
