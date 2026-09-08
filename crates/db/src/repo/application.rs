//! One application per (job, default profile). This is the user's pipeline,
//! not whether the requisition is still live (`job.status`).

use jobseeker_core::domain::enums::ApplicationStatus;
use jobseeker_core::ids::{ApplicationId, JobId, ProfileId};
use jobseeker_core::{Error, Result};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{db_err, Db};

/// What the job page and the list row need.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationView {
    pub id: String,
    pub job_id: String,
    pub profile_id: String,
    pub status: String,
    pub applied_at: Option<String>,
    pub last_activity_at: String,
    pub updated_at: String,
}

/// Latest application for this job on the default profile, if any.
pub async fn get_for_job(db: &Db, job_id: &JobId) -> Result<Option<ApplicationView>> {
    if crate::repo::job::get(db, job_id).await?.is_none() {
        return Err(Error::NotFound("job"));
    }
    let Some(profile_id) = crate::repo::profile::default_id(db).await? else {
        return Ok(None);
    };
    get_pair(db, job_id, &profile_id).await
}

/// Create or update pipeline status. Writes an append-only `status_change` event.
/// Stamps `applied_at` the first time status reaches applied or later.
pub async fn set_status(
    db: &Db,
    job_id: &JobId,
    status: ApplicationStatus,
) -> Result<ApplicationView> {
    if crate::repo::job::get(db, job_id).await?.is_none() {
        return Err(Error::NotFound("job"));
    }
    let profile_id = crate::repo::profile::ensure_default(db).await?;
    let ts = jobseeker_core::time::to_rfc3339(&jobseeker_core::time::now());
    let existing = get_pair(db, job_id, &profile_id).await?;
    let stamp_applied =
        should_stamp_applied(status) && existing.as_ref().is_none_or(|e| e.applied_at.is_none());

    let view = if let Some(row) = existing {
        if row.status == status.as_str() {
            return Ok(row);
        }
        if stamp_applied {
            sqlx::query(
                "UPDATE application SET status = ?1, applied_at = COALESCE(applied_at, ?2),
                        last_activity_at = ?2, updated_at = ?2
                  WHERE id = ?3",
            )
            .bind(status.as_str())
            .bind(&ts)
            .bind(&row.id)
            .execute(db.writer())
            .await
            .map_err(db_err)?;
        } else {
            sqlx::query(
                "UPDATE application SET status = ?1, last_activity_at = ?2, updated_at = ?2
                  WHERE id = ?3",
            )
            .bind(status.as_str())
            .bind(&ts)
            .bind(&row.id)
            .execute(db.writer())
            .await
            .map_err(db_err)?;
        }
        write_status_event(db, &row.id, Some(&row.status), status.as_str(), &ts).await?;
        get_by_id(db, &row.id)
            .await?
            .ok_or(Error::NotFound("application"))?
    } else {
        let id = ApplicationId::new();
        sqlx::query(
            "INSERT INTO application
                (id, job_id, profile_id, status, applied_at, last_activity_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)",
        )
        .bind(id.as_str())
        .bind(job_id.as_str())
        .bind(profile_id.as_str())
        .bind(status.as_str())
        .bind(if stamp_applied { Some(ts.as_str()) } else { None })
        .bind(&ts)
        .execute(db.writer())
        .await
        .map_err(db_err)?;
        write_status_event(db, id.as_str(), None, status.as_str(), &ts).await?;
        get_by_id(db, id.as_str())
            .await?
            .ok_or(Error::NotFound("application"))?
    };
    Ok(view)
}

fn should_stamp_applied(status: ApplicationStatus) -> bool {
    matches!(
        status,
        ApplicationStatus::Applied
            | ApplicationStatus::Screening
            | ApplicationStatus::Interviewing
            | ApplicationStatus::Offer
            | ApplicationStatus::Accepted
    )
}

async fn get_pair(
    db: &Db,
    job_id: &JobId,
    profile_id: &ProfileId,
) -> Result<Option<ApplicationView>> {
    let row = sqlx::query(
        "SELECT id, job_id, profile_id, status, applied_at, last_activity_at, updated_at
           FROM application WHERE job_id = ?1 AND profile_id = ?2",
    )
    .bind(job_id.as_str())
    .bind(profile_id.as_str())
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    row.map(row_to_view).transpose()
}

async fn get_by_id(db: &Db, id: &str) -> Result<Option<ApplicationView>> {
    let row = sqlx::query(
        "SELECT id, job_id, profile_id, status, applied_at, last_activity_at, updated_at
           FROM application WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    row.map(row_to_view).transpose()
}

fn row_to_view(row: sqlx::sqlite::SqliteRow) -> Result<ApplicationView> {
    Ok(ApplicationView {
        id: row.try_get("id").map_err(db_err)?,
        job_id: row.try_get("job_id").map_err(db_err)?,
        profile_id: row.try_get("profile_id").map_err(db_err)?,
        status: row.try_get("status").map_err(db_err)?,
        applied_at: row.try_get("applied_at").map_err(db_err)?,
        last_activity_at: row.try_get("last_activity_at").map_err(db_err)?,
        updated_at: row.try_get("updated_at").map_err(db_err)?,
    })
}

async fn write_status_event(
    db: &Db,
    application_id: &str,
    from: Option<&str>,
    to: &str,
    ts: &str,
) -> Result<()> {
    let eid = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO application_event
            (id, application_id, kind, occurred_at, title, from_status, to_status, created_at)
         VALUES (?1, ?2, 'status_change', ?3, ?4, ?5, ?6, ?3)",
    )
    .bind(&eid)
    .bind(application_id)
    .bind(ts)
    .bind(format!("status → {to}"))
    .bind(from)
    .bind(to)
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::job::{self, JobFilter, JobPatch};

    async fn seed_job(db: &Db) -> JobId {
        let company_id = crate::repo::company::resolve_or_create(db, "Zillow", None)
            .await
            .unwrap();
        let id = JobId::new();
        sqlx::query(
            "INSERT INTO job (id, company_id, slug, title, title_normalized, status, work_mode,
                              salary_period, posted_at, description_text, content_hash,
                              first_seen_at, last_seen_at, created_at, updated_at)
             VALUES (?1, ?2, 'ds', 'Data Scientist', 'data scientist', 'open', 'remote',
                     'year', '2026-09-01T00:00:00Z', 'desc', 'h',
                     '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z',
                     '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z')",
        )
        .bind(id.as_str())
        .bind(company_id.as_str())
        .execute(db.writer())
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn missing_application_is_none_until_set() {
        let db = Db::open_in_memory().await.unwrap();
        let id = seed_job(&db).await;
        assert!(get_for_job(&db, &id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn applied_stamps_applied_at_and_writes_an_event() {
        let db = Db::open_in_memory().await.unwrap();
        let id = seed_job(&db).await;
        let first = set_status(&db, &id, ApplicationStatus::Interested)
            .await
            .unwrap();
        assert_eq!(first.status, "interested");
        assert!(first.applied_at.is_none());

        let applied = set_status(&db, &id, ApplicationStatus::Applied)
            .await
            .unwrap();
        assert_eq!(applied.status, "applied");
        assert!(applied.applied_at.is_some());
        assert_eq!(applied.id, first.id);

        let later = set_status(&db, &id, ApplicationStatus::Interviewing)
            .await
            .unwrap();
        assert_eq!(later.applied_at, applied.applied_at);

        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM application_event WHERE application_id = ?1 AND kind = 'status_change'",
        )
        .bind(&applied.id)
        .fetch_one(db.reader())
        .await
        .unwrap();
        assert_eq!(events, 3);

        let page = job::list(&db, &JobFilter::default()).await.unwrap();
        assert_eq!(
            page.items[0].application_status.as_deref(),
            Some("interviewing")
        );
    }

    #[tokio::test]
    async fn job_status_applied_is_still_rejected() {
        let db = Db::open_in_memory().await.unwrap();
        let id = seed_job(&db).await;
        let err = job::patch(
            &db,
            &id,
            &JobPatch {
                status: Some("applied".into()),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(err, Err(Error::BadRequest(_))));
    }
}
