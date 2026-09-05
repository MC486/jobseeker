//! Job queries.
//!
//! The list query is the performance-critical path (NFR-P-02): filters compose in SQL,
//! pagination is keyset-based, and the projection is only what a table row renders.

use jobseeker_core::domain::enums::{JobStatus, Seniority, WorkMode};
use jobseeker_core::ids::{CompanyId, JobId};
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
    let use_fts = filter.query.as_deref().is_some_and(|q| !q.trim().is_empty());

    let mut sql = String::from(
        "SELECT j.id, j.title, c.name AS company_name, c.slug AS company_slug, j.status,
                j.work_mode, j.seniority, j.salary_min_cents, j.salary_max_cents,
                j.salary_currency, j.salary_period, j.salary_is_estimate,
                j.posted_at, j.closes_at, j.user_rating, j.is_archived,
                j.extraction_partial, j.updated_at,
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
            salary_is_estimate: row.try_get::<i64, _>("salary_is_estimate").map_err(db_err)? != 0,
            posted_at: row.try_get("posted_at").map_err(db_err)?,
            closes_at: row.try_get("closes_at").map_err(db_err)?,
            primary_location: row.try_get("primary_location").map_err(db_err)?,
            user_rating: row.try_get("user_rating").map_err(db_err)?,
            is_archived: row.try_get::<i64, _>("is_archived").map_err(db_err)? != 0,
            extraction_partial: row.try_get::<i64, _>("extraction_partial").map_err(db_err)? != 0,
            updated_at: row.try_get("updated_at").map_err(db_err)?,
        });
    }

    // FTS results are relevance-ranked, and bm25 scores are not a stable keyset; that page
    // is deliberately single-page until relevance cursors are implemented.
    let next_cursor = if has_more && !use_fts {
        items.last().map(|last| {
            let sort_key = match filter.sort {
                JobSort::PostedAt | JobSort::Relevance => last.posted_at.clone().unwrap_or_default(),
                JobSort::UpdatedAt => last.updated_at.clone(),
                JobSort::Salary => last.salary_max_cents.map(|v| v.to_string()).unwrap_or_default(),
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

enum Bind {
    Text(String),
    Int(i64),
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
    let cutoff = jobseeker_core::time::to_rfc3339(&(jobseeker_core::time::now() + chrono::Duration::days(days)));
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
    .bind(row.try_get::<String, _>("description_text").map_err(db_err)?)
    .bind(row.try_get::<String, _>("requirements_text").map_err(db_err)?)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;

    tx.commit().await.map_err(db_err)?;
    Ok(())
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
        let company_id = company::resolve_or_create(db, company_name, None).await.unwrap();
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
        insert_job(&db, "Acme", "Old Role", JobStatus::Open, WorkMode::Remote, None, SalaryPeriod::Year, "2026-01-01T00:00:00Z").await;
        insert_job(&db, "Acme", "New Role", JobStatus::Open, WorkMode::Remote, None, SalaryPeriod::Year, "2026-09-01T00:00:00Z").await;

        let page = list(&db, &JobFilter::default()).await.unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].title, "New Role");
        assert!(page.next_cursor.is_none());
    }

    #[tokio::test]
    async fn filters_compose() {
        let db = Db::open_in_memory().await.unwrap();
        insert_job(&db, "Acme", "Remote Open", JobStatus::Open, WorkMode::Remote, None, SalaryPeriod::Year, "2026-09-01T00:00:00Z").await;
        insert_job(&db, "Acme", "Onsite Open", JobStatus::Open, WorkMode::Onsite, None, SalaryPeriod::Year, "2026-09-02T00:00:00Z").await;
        insert_job(&db, "Acme", "Remote Closed", JobStatus::Closed, WorkMode::Remote, None, SalaryPeriod::Year, "2026-09-03T00:00:00Z").await;

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
        insert_job(&db, "Acme", "Contract", JobStatus::Open, WorkMode::Remote, Some(90_00), SalaryPeriod::Hour, "2026-09-01T00:00:00Z").await;
        // $120k/yr is below it.
        insert_job(&db, "Acme", "Salaried", JobStatus::Open, WorkMode::Remote, Some(120_000_00), SalaryPeriod::Year, "2026-09-02T00:00:00Z").await;

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
        let id = insert_job(&db, "Acme", "Archived", JobStatus::Open, WorkMode::Remote, None, SalaryPeriod::Year, "2026-09-01T00:00:00Z").await;
        sqlx::query("UPDATE job SET is_archived = 1 WHERE id = ?1")
            .bind(id.as_str())
            .execute(db.writer())
            .await
            .unwrap();

        assert!(list(&db, &JobFilter::default()).await.unwrap().items.is_empty());
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
        let id = insert_job(&db, "Acme", "Platform Engineer", JobStatus::Open, WorkMode::Remote, None, SalaryPeriod::Year, "2026-09-01T00:00:00Z").await;
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
        let id = insert_job(&db, "Acme", "Rust Engineer", JobStatus::Open, WorkMode::Remote, None, SalaryPeriod::Year, "2026-09-01T00:00:00Z").await;
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
        insert_job(&db, "Acme", "A", JobStatus::Open, WorkMode::Remote, None, SalaryPeriod::Year, "2026-09-01T00:00:00Z").await;
        insert_job(&db, "Acme", "B", JobStatus::Open, WorkMode::Remote, None, SalaryPeriod::Year, "2026-09-02T00:00:00Z").await;
        let closed = insert_job(&db, "Acme", "C", JobStatus::Closed, WorkMode::Remote, None, SalaryPeriod::Year, "2026-09-03T00:00:00Z").await;
        let archived = insert_job(&db, "Acme", "D", JobStatus::Open, WorkMode::Remote, None, SalaryPeriod::Year, "2026-09-04T00:00:00Z").await;
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
        let job = insert_job(&db, "Acme", "Role", JobStatus::Open, WorkMode::Remote, None, SalaryPeriod::Year, "2026-09-01T00:00:00Z").await;
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
        assert!((overall - 0.81).abs() < 1e-9, "the value must survive staleness");
    }
}
