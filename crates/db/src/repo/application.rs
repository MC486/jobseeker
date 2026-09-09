//! One application per (job, default profile). This is the user's pipeline,
//! not whether the requisition is still live (`job.status`).

use chrono::NaiveDate;
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
    #[serde(default)]
    pub next_action: Option<String>,
    #[serde(default)]
    pub next_action_due: Option<String>,
    pub last_activity_at: String,
    pub updated_at: String,
}

/// Partial update. `None` leaves a field alone; `Some("")` clears next-action fields.
#[derive(Debug, Clone, Default)]
pub struct ApplicationPatch {
    pub status: Option<ApplicationStatus>,
    pub next_action: Option<String>,
    pub next_action_due: Option<String>,
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
    patch(
        db,
        job_id,
        ApplicationPatch {
            status: Some(status),
            ..Default::default()
        },
    )
    .await
}

/// Create or update status and/or next action. Empty patch is a bad request.
pub async fn patch(db: &Db, job_id: &JobId, patch: ApplicationPatch) -> Result<ApplicationView> {
    if patch.status.is_none() && patch.next_action.is_none() && patch.next_action_due.is_none() {
        return Err(Error::BadRequest(
            "status, next_action, or next_action_due required".into(),
        ));
    }
    if crate::repo::job::get(db, job_id).await?.is_none() {
        return Err(Error::NotFound("job"));
    }
    let profile_id = crate::repo::profile::ensure_default(db).await?;
    let ts = jobseeker_core::time::to_rfc3339(&jobseeker_core::time::now());
    // Outer None = leave unchanged. Inner None = clear.
    let next_action = patch.next_action.as_deref().map(blank_to_none);
    let next_due = match &patch.next_action_due {
        None => None,
        Some(s) => Some(parse_due(s)?),
    };
    let existing = get_pair(db, job_id, &profile_id).await?;

    let view = if let Some(row) = existing {
        let new_status = patch
            .status
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| row.status.clone());
        let new_action = match &next_action {
            Some(v) => v.clone(),
            None => row.next_action.clone(),
        };
        let new_due = match &next_due {
            Some(v) => v.clone(),
            None => row.next_action_due.clone(),
        };
        let status_changed = new_status != row.status;
        let action_changed = new_action != row.next_action || new_due != row.next_action_due;
        if !status_changed && !action_changed {
            return Ok(row);
        }
        let stamp_applied =
            patch.status.is_some_and(should_stamp_applied) && row.applied_at.is_none();
        let applied_at = if stamp_applied {
            Some(ts.as_str())
        } else {
            row.applied_at.as_deref()
        };
        sqlx::query(
            "UPDATE application SET status = ?1, applied_at = ?2, next_action = ?3,
                    next_action_due = ?4, last_activity_at = ?5, updated_at = ?5
              WHERE id = ?6",
        )
        .bind(&new_status)
        .bind(applied_at)
        .bind(new_action.as_deref())
        .bind(new_due.as_deref())
        .bind(&ts)
        .bind(&row.id)
        .execute(db.writer())
        .await
        .map_err(db_err)?;
        if status_changed {
            write_status_event(db, &row.id, Some(&row.status), &new_status, &ts).await?;
        }
        if action_changed {
            write_reminder_event(db, &row.id, new_action.as_deref(), new_due.as_deref(), &ts)
                .await?;
        }
        get_by_id(db, &row.id)
            .await?
            .ok_or(Error::NotFound("application"))?
    } else {
        let status = patch.status.unwrap_or(ApplicationStatus::Interested);
        let stamp_applied = should_stamp_applied(status);
        let action = next_action.flatten();
        let due = next_due.flatten();
        let id = ApplicationId::new();
        sqlx::query(
            "INSERT INTO application
                (id, job_id, profile_id, status, applied_at, next_action, next_action_due,
                 last_activity_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?8)",
        )
        .bind(id.as_str())
        .bind(job_id.as_str())
        .bind(profile_id.as_str())
        .bind(status.as_str())
        .bind(if stamp_applied {
            Some(ts.as_str())
        } else {
            None
        })
        .bind(action.as_deref())
        .bind(due.as_deref())
        .bind(&ts)
        .execute(db.writer())
        .await
        .map_err(db_err)?;
        write_status_event(db, id.as_str(), None, status.as_str(), &ts).await?;
        if action.is_some() || due.is_some() {
            write_reminder_event(db, id.as_str(), action.as_deref(), due.as_deref(), &ts).await?;
        }
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

fn blank_to_none(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn parse_due(raw: &str) -> Result<Option<String>> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let date = if t.len() >= 10 { &t[..10] } else { t };
    NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| {
        Error::BadRequest(format!("next_action_due must be YYYY-MM-DD, got {raw:?}"))
    })?;
    Ok(Some(date.to_string()))
}

async fn get_pair(
    db: &Db,
    job_id: &JobId,
    profile_id: &ProfileId,
) -> Result<Option<ApplicationView>> {
    let row = sqlx::query(
        "SELECT id, job_id, profile_id, status, applied_at, next_action, next_action_due,
                last_activity_at, updated_at
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
        "SELECT id, job_id, profile_id, status, applied_at, next_action, next_action_due,
                last_activity_at, updated_at
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
        next_action: row.try_get("next_action").map_err(db_err)?,
        next_action_due: row.try_get("next_action_due").map_err(db_err)?,
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

async fn write_reminder_event(
    db: &Db,
    application_id: &str,
    action: Option<&str>,
    due: Option<&str>,
    ts: &str,
) -> Result<()> {
    let eid = uuid::Uuid::now_v7().to_string();
    let title = match (action, due) {
        (None, None) => "cleared next action".to_string(),
        (Some(a), Some(d)) => format!("{a} by {d}"),
        (Some(a), None) => a.to_string(),
        (None, Some(d)) => format!("due {d}"),
    };
    sqlx::query(
        "INSERT INTO application_event
            (id, application_id, kind, occurred_at, title, created_at)
         VALUES (?1, ?2, 'reminder', ?3, ?4, ?3)",
    )
    .bind(&eid)
    .bind(application_id)
    .bind(ts)
    .bind(&title)
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

    #[tokio::test]
    async fn next_action_creates_interested_row_and_list_shows_due() {
        let db = Db::open_in_memory().await.unwrap();
        let id = seed_job(&db).await;
        let row = patch(
            &db,
            &id,
            ApplicationPatch {
                next_action: Some("email recruiter".into()),
                next_action_due: Some("2026-09-01".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(row.status, "interested");
        assert_eq!(row.next_action.as_deref(), Some("email recruiter"));
        assert_eq!(row.next_action_due.as_deref(), Some("2026-09-01"));
        assert!(row.applied_at.is_none());

        let reminders: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM application_event WHERE application_id = ?1 AND kind = 'reminder'",
        )
        .bind(&row.id)
        .fetch_one(db.reader())
        .await
        .unwrap();
        assert_eq!(reminders, 1);

        let page = job::list(&db, &JobFilter::default()).await.unwrap();
        assert_eq!(
            page.items[0].next_action.as_deref(),
            Some("email recruiter")
        );
        assert_eq!(page.items[0].next_action_due.as_deref(), Some("2026-09-01"));
        assert_eq!(
            page.items[0].application_status.as_deref(),
            Some("interested")
        );
        assert_eq!(page.items[0].status, "open");
    }

    #[tokio::test]
    async fn clearing_next_action_and_rejecting_bad_due() {
        let db = Db::open_in_memory().await.unwrap();
        let id = seed_job(&db).await;
        set_status(&db, &id, ApplicationStatus::Applied)
            .await
            .unwrap();
        patch(
            &db,
            &id,
            ApplicationPatch {
                next_action: Some("prep loop".into()),
                next_action_due: Some("2026-09-12T15:00:00Z".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let cleared = patch(
            &db,
            &id,
            ApplicationPatch {
                next_action: Some("".into()),
                next_action_due: Some("".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(cleared.status, "applied");
        assert!(cleared.next_action.is_none());
        assert!(cleared.next_action_due.is_none());

        let err = patch(
            &db,
            &id,
            ApplicationPatch {
                next_action_due: Some("soon".into()),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(err, Err(Error::BadRequest(_))));
    }
}
