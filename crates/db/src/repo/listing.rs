//! Listings: one posting as seen at one URL.
//!
//! `url_hash` is the natural idempotency key. Re-ingesting the same link must touch
//! `last_seen_at` and add a capture, never create a second listing.

use jobseeker_core::domain::enums::SourceKind;
use jobseeker_core::hash::content_hash;
use jobseeker_core::ids::{JobId, ListingId, SourceId};
use jobseeker_core::time::{now, to_rfc3339};
use jobseeker_core::Result;
use sqlx::Row;

use crate::{db_err, Db};

#[derive(Debug, Clone)]
pub struct UpsertListing {
    pub source: SourceKind,
    pub url: String,
    pub url_canonical: String,
    pub source_job_id: Option<String>,
    pub title_at_source: Option<String>,
    pub company_name_at_source: Option<String>,
}

/// Insert the listing, or return the existing one for this canonical URL.
///
/// Returns `(listing_id, is_new)`. `is_new = false` means the caller is looking at a
/// re-ingestion, which should record a fresh capture and diff rather than create a job.
pub async fn upsert_by_url(db: &Db, input: &UpsertListing) -> Result<(ListingId, bool)> {
    let source_id = source_id_for(db, input.source).await?;
    let url_hash = content_hash(input.url_canonical.as_bytes());
    let ts = to_rfc3339(&now());

    if let Some(existing) = find_by_url_hash(db, &url_hash).await? {
        sqlx::query(
            "UPDATE job_source_listing
                SET last_seen_at = ?1, updated_at = ?1,
                    title_at_source = COALESCE(?2, title_at_source),
                    company_name_at_source = COALESCE(?3, company_name_at_source),
                    source_job_id = COALESCE(?4, source_job_id)
              WHERE id = ?5",
        )
        .bind(&ts)
        .bind(input.title_at_source.as_deref())
        .bind(input.company_name_at_source.as_deref())
        .bind(input.source_job_id.as_deref())
        .bind(existing.as_str())
        .execute(db.writer())
        .await
        .map_err(db_err)?;
        return Ok((existing, false));
    }

    let id = ListingId::new();
    sqlx::query(
        "INSERT INTO job_source_listing
            (id, source_id, source_job_id, url, url_canonical, url_hash, title_at_source,
             company_name_at_source, first_seen_at, last_seen_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?9, ?9)",
    )
    .bind(id.as_str())
    .bind(source_id.as_str())
    .bind(input.source_job_id.as_deref())
    .bind(&input.url)
    .bind(&input.url_canonical)
    .bind(&url_hash)
    .bind(input.title_at_source.as_deref())
    .bind(input.company_name_at_source.as_deref())
    .bind(&ts)
    .execute(db.writer())
    .await
    .map_err(db_err)?;

    Ok((id, true))
}

/// A listing as the pipeline and API see it.
#[derive(Debug, Clone)]
pub struct ListingRow {
    pub id: ListingId,
    pub job_id: Option<JobId>,
    pub url: String,
    pub url_canonical: String,
    pub source: SourceKind,
    pub source_job_id: Option<String>,
    pub status: String,
}

pub async fn get(db: &Db, id: &ListingId) -> Result<Option<ListingRow>> {
    let row = sqlx::query(
        "SELECT l.id, l.job_id, l.url, l.url_canonical, l.source_job_id, l.status, s.kind
           FROM job_source_listing l
           JOIN source s ON s.id = l.source_id
          WHERE l.id = ?1",
    )
    .bind(id.as_str())
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    let Some(row) = row else { return Ok(None) };
    let kind: String = row.try_get("kind").map_err(db_err)?;
    let job_id: Option<String> = row.try_get("job_id").map_err(db_err)?;
    Ok(Some(ListingRow {
        id: row.try_get::<String, _>("id").map_err(db_err)?.parse()?,
        job_id: job_id.map(|s| s.parse()).transpose()?,
        url: row.try_get("url").map_err(db_err)?,
        url_canonical: row.try_get("url_canonical").map_err(db_err)?,
        source: kind.parse()?,
        source_job_id: row.try_get("source_job_id").map_err(db_err)?,
        status: row.try_get("status").map_err(db_err)?,
    }))
}

pub async fn find_by_url_hash(db: &Db, url_hash: &str) -> Result<Option<ListingId>> {
    let id: Option<String> =
        sqlx::query_scalar("SELECT id FROM job_source_listing WHERE url_hash = ?1")
            .bind(url_hash)
            .fetch_optional(db.reader())
            .await
            .map_err(db_err)?;
    id.map(|s| s.parse()).transpose()
}

/// Attach a listing to a canonical job, promoting it to canonical when its source has
/// higher fidelity than the incumbent (an employer's ATS beats an aggregator).
pub async fn attach_to_job(db: &Db, listing_id: &ListingId, job_id: &JobId) -> Result<()> {
    let mut tx = db.writer().begin().await.map_err(db_err)?;
    let ts = to_rfc3339(&now());

    sqlx::query("UPDATE job_source_listing SET job_id = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(job_id.as_str())
        .bind(&ts)
        .bind(listing_id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;

    // Whichever attached listing has the highest source fidelity becomes canonical.
    let best: Option<String> = sqlx::query_scalar(
        "SELECT l.id FROM job_source_listing l
           JOIN source s ON s.id = l.source_id
          WHERE l.job_id = ?1
          ORDER BY s.fidelity DESC, l.first_seen_at ASC
          LIMIT 1",
    )
    .bind(job_id.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;

    if let Some(best) = best {
        sqlx::query("UPDATE job_source_listing SET is_canonical = 0 WHERE job_id = ?1")
            .bind(job_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("UPDATE job_source_listing SET is_canonical = 1 WHERE id = ?1")
            .bind(&best)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("UPDATE job SET canonical_listing_id = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(&best)
            .bind(&ts)
            .bind(job_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
    }

    tx.commit().await.map_err(db_err)?;
    Ok(())
}

/// Record that a listing no longer accepts applications. Data is never deleted: a closed
/// posting is exactly the evidence you need when a recruiter asks what you applied to.
pub async fn mark_closed(db: &Db, listing_id: &ListingId) -> Result<()> {
    let ts = to_rfc3339(&now());
    sqlx::query(
        "UPDATE job_source_listing
            SET status = 'closed', last_checked_at = ?1, updated_at = ?1
          WHERE id = ?2",
    )
    .bind(&ts)
    .bind(listing_id.as_str())
    .execute(db.writer())
    .await
    .map_err(db_err)?;

    // The job closes only once every one of its listings has.
    sqlx::query(
        "UPDATE job
            SET status = 'closed', closed_at = COALESCE(closed_at, ?1), updated_at = ?1
          WHERE id = (SELECT job_id FROM job_source_listing WHERE id = ?2)
            AND NOT EXISTS (
                SELECT 1 FROM job_source_listing l2
                 WHERE l2.job_id = job.id AND l2.status = 'open'
            )",
    )
    .bind(&ts)
    .bind(listing_id.as_str())
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn record_check_failure(db: &Db, listing_id: &ListingId) -> Result<i64> {
    let ts = to_rfc3339(&now());
    let failures: i64 = sqlx::query_scalar(
        "UPDATE job_source_listing
            SET check_failures = check_failures + 1, last_checked_at = ?1, updated_at = ?1
          WHERE id = ?2
          RETURNING check_failures",
    )
    .bind(&ts)
    .bind(listing_id.as_str())
    .fetch_one(db.writer())
    .await
    .map_err(db_err)?;

    // Three consecutive failures means we no longer know the real state; say so rather than
    // guessing "closed".
    if failures >= 3 {
        sqlx::query(
            "UPDATE job_source_listing SET status = 'unknown', updated_at = ?1 WHERE id = ?2",
        )
        .bind(&ts)
        .bind(listing_id.as_str())
        .execute(db.writer())
        .await
        .map_err(db_err)?;
    }
    Ok(failures)
}

/// Listings due for a refresh: server-fetchable, open, and stale.
pub async fn due_for_refresh(db: &Db, older_than_hours: i64, limit: i64) -> Result<Vec<ListingId>> {
    let cutoff = to_rfc3339(&(now() - chrono::Duration::hours(older_than_hours)));
    let rows = sqlx::query(
        "SELECT id FROM job_source_listing
          WHERE status = 'open' AND check_failures < 3
            AND (last_checked_at IS NULL OR last_checked_at < ?1)
          ORDER BY COALESCE(last_checked_at, first_seen_at) ASC
          LIMIT ?2",
    )
    .bind(&cutoff)
    .bind(limit)
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    rows.into_iter()
        .map(|r| r.try_get::<String, _>("id").map_err(db_err)?.parse())
        .collect()
}

async fn source_id_for(db: &Db, kind: SourceKind) -> Result<SourceId> {
    let id: String = sqlx::query_scalar("SELECT id FROM source WHERE kind = ?1")
        .bind(kind.as_str())
        .fetch_one(db.reader())
        .await
        .map_err(db_err)?;
    id.parse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::company;

    fn listing(source: SourceKind, url: &str) -> UpsertListing {
        UpsertListing {
            source,
            url: url.to_string(),
            url_canonical: url.to_string(),
            source_job_id: None,
            title_at_source: Some("Platform Engineer".into()),
            company_name_at_source: Some("Acme".into()),
        }
    }

    async fn a_job(db: &Db) -> JobId {
        let company_id = company::resolve_or_create(db, "Acme", None).await.unwrap();
        let id = JobId::new();
        sqlx::query(
            "INSERT INTO job (id, company_id, slug, title, title_normalized, content_hash,
                              first_seen_at, last_seen_at, created_at, updated_at)
             VALUES (?1, ?2, 's', 't', 't', 'h', 'now', 'now', 'now', 'now')",
        )
        .bind(id.as_str())
        .bind(company_id.as_str())
        .execute(db.writer())
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn reingesting_a_url_updates_rather_than_duplicates() {
        let db = Db::open_in_memory().await.unwrap();
        let url = "https://boards.greenhouse.io/acme/jobs/1";

        let (first, is_new) = upsert_by_url(&db, &listing(SourceKind::Greenhouse, url))
            .await
            .unwrap();
        assert!(is_new);

        let (second, is_new) = upsert_by_url(&db, &listing(SourceKind::Greenhouse, url))
            .await
            .unwrap();
        assert!(!is_new, "the second ingest is a refresh, not a new listing");
        assert_eq!(first, second);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job_source_listing")
            .fetch_one(db.reader())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn the_highest_fidelity_source_becomes_canonical() {
        let db = Db::open_in_memory().await.unwrap();
        let job = a_job(&db).await;

        let (linkedin, _) = upsert_by_url(
            &db,
            &listing(SourceKind::LinkedIn, "https://www.linkedin.com/jobs/view/1"),
        )
        .await
        .unwrap();
        attach_to_job(&db, &linkedin, &job).await.unwrap();

        // With only LinkedIn attached, it is canonical by default.
        let canonical: String =
            sqlx::query_scalar("SELECT canonical_listing_id FROM job WHERE id = ?1")
                .bind(job.as_str())
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(canonical, linkedin.as_str());

        // The employer's own ATS then displaces it.
        let (greenhouse, _) = upsert_by_url(
            &db,
            &listing(
                SourceKind::Greenhouse,
                "https://boards.greenhouse.io/acme/jobs/1",
            ),
        )
        .await
        .unwrap();
        attach_to_job(&db, &greenhouse, &job).await.unwrap();

        let canonical: String =
            sqlx::query_scalar("SELECT canonical_listing_id FROM job WHERE id = ?1")
                .bind(job.as_str())
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(canonical, greenhouse.as_str());

        let flags: Vec<i64> = sqlx::query_scalar(
            "SELECT is_canonical FROM job_source_listing WHERE job_id = ?1 ORDER BY is_canonical",
        )
        .bind(job.as_str())
        .fetch_all(db.reader())
        .await
        .unwrap();
        assert_eq!(flags, vec![0, 1], "exactly one canonical listing");
    }

    #[tokio::test]
    async fn a_job_closes_only_when_every_listing_has() {
        let db = Db::open_in_memory().await.unwrap();
        let job = a_job(&db).await;
        let (a, _) = upsert_by_url(&db, &listing(SourceKind::LinkedIn, "https://l/1"))
            .await
            .unwrap();
        let (b, _) = upsert_by_url(&db, &listing(SourceKind::Greenhouse, "https://g/1"))
            .await
            .unwrap();
        attach_to_job(&db, &a, &job).await.unwrap();
        attach_to_job(&db, &b, &job).await.unwrap();

        mark_closed(&db, &a).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM job WHERE id = ?1")
            .bind(job.as_str())
            .fetch_one(db.reader())
            .await
            .unwrap();
        assert_eq!(status, "open", "one aggregator dropping it proves nothing");

        mark_closed(&db, &b).await.unwrap();
        let (status, closed_at): (String, Option<String>) =
            sqlx::query_as("SELECT status, closed_at FROM job WHERE id = ?1")
                .bind(job.as_str())
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(status, "closed");
        assert!(closed_at.is_some());
    }

    #[tokio::test]
    async fn repeated_check_failures_yield_unknown_not_closed() {
        let db = Db::open_in_memory().await.unwrap();
        let (id, _) = upsert_by_url(&db, &listing(SourceKind::CompanySite, "https://x/1"))
            .await
            .unwrap();

        assert_eq!(record_check_failure(&db, &id).await.unwrap(), 1);
        assert_eq!(record_check_failure(&db, &id).await.unwrap(), 2);
        let status: String =
            sqlx::query_scalar("SELECT status FROM job_source_listing WHERE id = ?1")
                .bind(id.as_str())
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(status, "open");

        assert_eq!(record_check_failure(&db, &id).await.unwrap(), 3);
        let status: String =
            sqlx::query_scalar("SELECT status FROM job_source_listing WHERE id = ?1")
                .bind(id.as_str())
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(status, "unknown", "a flaky site is not a closed posting");
    }

    #[tokio::test]
    async fn refresh_queue_skips_closed_and_broken_listings() {
        let db = Db::open_in_memory().await.unwrap();
        let (fresh, _) = upsert_by_url(&db, &listing(SourceKind::Greenhouse, "https://g/1"))
            .await
            .unwrap();
        let (closed, _) = upsert_by_url(&db, &listing(SourceKind::Greenhouse, "https://g/2"))
            .await
            .unwrap();
        mark_closed(&db, &closed).await.unwrap();

        let due = due_for_refresh(&db, 0, 10).await.unwrap();
        assert_eq!(due, vec![fresh]);
    }
}
