//! Extraction conflicts: sources disagreed, and the user decides.
//!
//! Merge writes an unresolved `extraction_conflict` when salary raw strings differ.
//! Nothing is auto-resolved. The user picks A, B, or keep-current; that write is
//! provenance `manual` so a later extract cannot silently overwrite it.

use jobseeker_core::ids::JobId;
use jobseeker_core::{Error, Result};
use jobseeker_normalize::salary::parse_salary;
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{db_err, Db};

/// One disagreement between two sources for a single job field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionConflict {
    pub id: String,
    pub job_id: String,
    pub field: String,
    pub value_a: Option<String>,
    pub provenance_a: Option<String>,
    pub listing_a_id: Option<String>,
    pub value_b: Option<String>,
    pub provenance_b: Option<String>,
    pub listing_b_id: Option<String>,
    pub resolved_value: Option<String>,
    pub resolution: String,
    pub created_at: String,
}

/// Which side the user kept. Never inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolveChoice {
    A,
    B,
    Keep,
}

impl ResolveChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
            Self::Keep => "keep",
        }
    }
}

impl std::str::FromStr for ResolveChoice {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "a" | "A" => Ok(Self::A),
            "b" | "B" => Ok(Self::B),
            "keep" | "current" => Ok(Self::Keep),
            other => Err(Error::BadRequest(format!(
                "choice must be a, b, or keep (got {other})"
            ))),
        }
    }
}

/// Record a disagreement. Callers (merge today) never pick a winner.
pub async fn insert(
    db: &Db,
    job_id: &JobId,
    field: &str,
    value_a: &str,
    provenance_a: Option<&str>,
    listing_a_id: Option<&str>,
    value_b: &str,
    provenance_b: Option<&str>,
    listing_b_id: Option<&str>,
) -> Result<String> {
    let id = uuid::Uuid::now_v7().to_string();
    let ts = jobseeker_core::time::to_rfc3339(&jobseeker_core::time::now());
    sqlx::query(
        "INSERT INTO extraction_conflict
            (id, job_id, field, value_a, provenance_a, listing_a_id,
             value_b, provenance_b, listing_b_id, resolution, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'unresolved', ?10)",
    )
    .bind(&id)
    .bind(job_id.as_str())
    .bind(field)
    .bind(value_a)
    .bind(provenance_a)
    .bind(listing_a_id)
    .bind(value_b)
    .bind(provenance_b)
    .bind(listing_b_id)
    .bind(&ts)
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    Ok(id)
}

/// Conflicts for one job. Unresolved first, then newest.
pub async fn list_for_job(db: &Db, job_id: &JobId) -> Result<Vec<ExtractionConflict>> {
    if crate::repo::job::get(db, job_id).await?.is_none() {
        return Err(Error::NotFound("job"));
    }
    let rows = sqlx::query(
        "SELECT id, job_id, field, value_a, provenance_a, listing_a_id,
                value_b, provenance_b, listing_b_id, resolved_value, resolution, created_at
           FROM extraction_conflict
          WHERE job_id = ?1
          ORDER BY CASE resolution WHEN 'unresolved' THEN 0 ELSE 1 END,
                   created_at DESC",
    )
    .bind(job_id.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    rows.into_iter().map(row_to_conflict).collect()
}

/// Apply the user's pick. Already-resolved rows stay put (never silent rewrite).
pub async fn resolve(
    db: &Db,
    job_id: &JobId,
    conflict_id: &str,
    choice: ResolveChoice,
) -> Result<ExtractionConflict> {
    if crate::repo::job::get(db, job_id).await?.is_none() {
        return Err(Error::NotFound("job"));
    }
    let existing = get(db, conflict_id)
        .await?
        .ok_or(Error::NotFound("conflict"))?;
    if existing.job_id != job_id.as_str() {
        return Err(Error::NotFound("conflict"));
    }
    if existing.resolution != "unresolved" {
        return Err(Error::Conflict("conflict already resolved".into()));
    }

    let current = current_field_value(db, job_id, &existing.field).await?;
    let resolved_value = match choice {
        ResolveChoice::A => existing
            .value_a
            .clone()
            .ok_or_else(|| Error::BadRequest("value_a is empty".into()))?,
        ResolveChoice::B => existing
            .value_b
            .clone()
            .ok_or_else(|| Error::BadRequest("value_b is empty".into()))?,
        ResolveChoice::Keep => current.clone().unwrap_or_default(),
    };

    let ts = jobseeker_core::time::to_rfc3339(&jobseeker_core::time::now());
    if choice != ResolveChoice::Keep {
        apply_field(db, job_id, &existing.field, &resolved_value, &ts).await?;
        write_manual_provenance(db, job_id, &existing.field, &ts).await?;
    }

    sqlx::query(
        "UPDATE extraction_conflict
            SET resolved_value = ?1, resolution = 'manual'
          WHERE id = ?2 AND job_id = ?3 AND resolution = 'unresolved'",
    )
    .bind(&resolved_value)
    .bind(conflict_id)
    .bind(job_id.as_str())
    .execute(db.writer())
    .await
    .map_err(db_err)?;

    get(db, conflict_id)
        .await?
        .ok_or(Error::NotFound("conflict"))
}

async fn get(db: &Db, conflict_id: &str) -> Result<Option<ExtractionConflict>> {
    let row = sqlx::query(
        "SELECT id, job_id, field, value_a, provenance_a, listing_a_id,
                value_b, provenance_b, listing_b_id, resolved_value, resolution, created_at
           FROM extraction_conflict WHERE id = ?1",
    )
    .bind(conflict_id)
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    row.map(row_to_conflict).transpose()
}

fn row_to_conflict(row: sqlx::sqlite::SqliteRow) -> Result<ExtractionConflict> {
    Ok(ExtractionConflict {
        id: row.try_get("id").map_err(db_err)?,
        job_id: row.try_get("job_id").map_err(db_err)?,
        field: row.try_get("field").map_err(db_err)?,
        value_a: row.try_get("value_a").map_err(db_err)?,
        provenance_a: row.try_get("provenance_a").map_err(db_err)?,
        listing_a_id: row.try_get("listing_a_id").map_err(db_err)?,
        value_b: row.try_get("value_b").map_err(db_err)?,
        provenance_b: row.try_get("provenance_b").map_err(db_err)?,
        listing_b_id: row.try_get("listing_b_id").map_err(db_err)?,
        resolved_value: row.try_get("resolved_value").map_err(db_err)?,
        resolution: row.try_get("resolution").map_err(db_err)?,
        created_at: row.try_get("created_at").map_err(db_err)?,
    })
}

async fn current_field_value(db: &Db, job_id: &JobId, field: &str) -> Result<Option<String>> {
    match field {
        "salary" => sqlx::query_scalar("SELECT salary_raw FROM job WHERE id = ?1")
            .bind(job_id.as_str())
            .fetch_optional(db.reader())
            .await
            .map_err(db_err),
        _ => Ok(None),
    }
}

async fn apply_field(db: &Db, job_id: &JobId, field: &str, value: &str, ts: &str) -> Result<()> {
    match field {
        "salary" => apply_salary(db, job_id, value, ts).await,
        other => Err(Error::BadRequest(format!(
            "resolving {other} is not implemented"
        ))),
    }
}

async fn apply_salary(db: &Db, job_id: &JobId, raw: &str, ts: &str) -> Result<()> {
    let parsed = parse_salary(raw, None, None);
    sqlx::query(
        "UPDATE job SET salary_raw = ?1, salary_min_cents = ?2, salary_max_cents = ?3,
                        salary_currency = ?4, salary_period = ?5, salary_is_estimate = ?6,
                        updated_at = ?7
          WHERE id = ?8 AND deleted_at IS NULL",
    )
    .bind(raw)
    .bind(parsed.as_ref().and_then(|s| s.min_cents))
    .bind(parsed.as_ref().and_then(|s| s.max_cents))
    .bind(parsed.as_ref().map(|s| s.currency.as_str()))
    .bind(
        parsed
            .as_ref()
            .map(|s| s.period.as_str())
            .unwrap_or("unknown"),
    )
    .bind(i64::from(parsed.as_ref().is_some_and(|s| s.is_estimate)))
    .bind(ts)
    .bind(job_id.as_str())
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn write_manual_provenance(db: &Db, job_id: &JobId, field: &str, ts: &str) -> Result<()> {
    let pid = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        r#"INSERT INTO field_provenance
            (id, entity_kind, entity_id, field, provenance, confidence, updated_at)
           VALUES (?1, 'job', ?2, ?3, 'manual', 1.0, ?4)
           ON CONFLICT (entity_kind, entity_id, field) DO UPDATE SET
             provenance = 'manual', confidence = 1.0, updated_at = excluded.updated_at"#,
    )
    .bind(&pid)
    .bind(job_id.as_str())
    .bind(field)
    .bind(ts)
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::job;

    async fn seed_pair(db: &Db) -> (JobId, JobId) {
        let keeper = insert(db, "Zillow", "Data Scientist", "$100,000 - $120,000 a year").await;
        let donor = insert(db, "Zillow", "Data Scientist", "$150,000 - $180,000 a year").await;
        (keeper, donor)
    }

    async fn insert(db: &Db, company: &str, title: &str, salary_raw: &str) -> JobId {
        let company_id = crate::repo::company::resolve_or_create(db, company, None)
            .await
            .unwrap();
        let id = JobId::new();
        sqlx::query(
            "INSERT INTO job (id, company_id, slug, title, title_normalized, status, work_mode,
                              salary_raw, salary_period, posted_at, description_text,
                              content_hash, first_seen_at, last_seen_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'open', 'remote', ?6, 'year',
                     '2026-09-01T00:00:00Z', 'desc', 'h',
                     '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z',
                     '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z')",
        )
        .bind(id.as_str())
        .bind(company_id.as_str())
        .bind(jobseeker_core::slug::slugify(title))
        .bind(title)
        .bind(jobseeker_core::slug::normalize_job_title(title))
        .bind(salary_raw)
        .execute(db.writer())
        .await
        .unwrap();
        id
    }

    async fn attach_sources(db: &Db, keeper: &JobId, donor: &JobId) {
        let keep_listing = crate::repo::listing::upsert_by_url(
            db,
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
            db,
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
        crate::repo::listing::attach_to_job(db, &keep_listing, keeper)
            .await
            .unwrap();
        crate::repo::listing::attach_to_job(db, &donor_listing, donor)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn merge_records_an_unresolved_salary_conflict() {
        let db = Db::open_in_memory().await.unwrap();
        let (keeper, donor) = seed_pair(&db).await;
        attach_sources(&db, &keeper, &donor).await;
        job::merge(&db, &donor, &keeper).await.unwrap();

        let found = list_for_job(&db, &keeper).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].field, "salary");
        assert_eq!(found[0].resolution, "unresolved");
        assert_eq!(
            found[0].value_a.as_deref(),
            Some("$100,000 - $120,000 a year")
        );
        assert_eq!(
            found[0].value_b.as_deref(),
            Some("$150,000 - $180,000 a year")
        );
        assert_eq!(found[0].provenance_a.as_deref(), Some("workday"));
        assert_eq!(found[0].provenance_b.as_deref(), Some("linkedin"));
        assert!(found[0].listing_a_id.is_some());
        assert!(found[0].listing_b_id.is_some());
        let kept = job::get(&db, &keeper).await.unwrap().unwrap();
        assert_eq!(
            kept.salary_raw.as_deref(),
            Some("$100,000 - $120,000 a year")
        );
    }

    #[tokio::test]
    async fn resolve_b_applies_the_donor_salary_and_is_sticky() {
        let db = Db::open_in_memory().await.unwrap();
        let (keeper, donor) = seed_pair(&db).await;
        attach_sources(&db, &keeper, &donor).await;
        job::merge(&db, &donor, &keeper).await.unwrap();
        let conflict = list_for_job(&db, &keeper).await.unwrap().remove(0);

        let resolved = resolve(&db, &keeper, &conflict.id, ResolveChoice::B)
            .await
            .unwrap();
        assert_eq!(resolved.resolution, "manual");
        assert_eq!(
            resolved.resolved_value.as_deref(),
            Some("$150,000 - $180,000 a year")
        );

        let kept = job::get(&db, &keeper).await.unwrap().unwrap();
        assert_eq!(
            kept.salary_raw.as_deref(),
            Some("$150,000 - $180,000 a year")
        );
        assert_eq!(kept.salary_min_cents, Some(15_000_000));
        assert_eq!(kept.salary_max_cents, Some(18_000_000));
        let prov = kept
            .provenance
            .iter()
            .find(|p| p.field == "salary")
            .expect("manual salary provenance");
        assert_eq!(prov.provenance, "manual");
        assert!((prov.confidence - 1.0).abs() < 1e-9);

        let again = resolve(&db, &keeper, &conflict.id, ResolveChoice::A).await;
        assert!(matches!(again, Err(Error::Conflict(_))));
    }

    #[tokio::test]
    async fn resolve_keep_leaves_the_keeper_salary() {
        let db = Db::open_in_memory().await.unwrap();
        let (keeper, donor) = seed_pair(&db).await;
        attach_sources(&db, &keeper, &donor).await;
        job::merge(&db, &donor, &keeper).await.unwrap();
        let conflict = list_for_job(&db, &keeper).await.unwrap().remove(0);

        let resolved = resolve(&db, &keeper, &conflict.id, ResolveChoice::Keep)
            .await
            .unwrap();
        assert_eq!(resolved.resolution, "manual");
        assert_eq!(
            resolved.resolved_value.as_deref(),
            Some("$100,000 - $120,000 a year")
        );
        let kept = job::get(&db, &keeper).await.unwrap().unwrap();
        assert_eq!(
            kept.salary_raw.as_deref(),
            Some("$100,000 - $120,000 a year")
        );
        assert!(kept.provenance.iter().all(|p| p.field != "salary"));
    }

    #[tokio::test]
    async fn list_is_empty_when_salaries_match() {
        let db = Db::open_in_memory().await.unwrap();
        let keeper = insert(&db, "Harbor", "Applied DS", "$145,000 a year").await;
        let donor = insert(&db, "Harbor", "Applied DS", "$145,000 a year").await;
        attach_sources(&db, &keeper, &donor).await;
        job::merge(&db, &donor, &keeper).await.unwrap();
        assert!(list_for_job(&db, &keeper).await.unwrap().is_empty());
    }
}
