//! Captures: immutable raw acquisitions.
//!
//! Inserts only — a capture is never mutated. The one mutable field is `extract_status`,
//! which records what the pipeline did with it.

use jobseeker_core::domain::capture::{CaptureMethod, ExtractStatus};
use jobseeker_core::ids::{CaptureId, ListingId};
use jobseeker_core::time::{now, to_rfc3339, Timestamp};
use jobseeker_core::Result;
use sqlx::Row;

use crate::{db_err, Db};

#[derive(Debug, Clone)]
pub struct NewCapture {
    pub listing_id: Option<ListingId>,
    pub url: Option<String>,
    pub method: CaptureMethod,
    pub http_status: Option<u16>,
    pub content_type: Option<String>,
    pub byte_len: i64,
    pub content_hash: String,
    pub storage_path: String,
    pub screenshot_path: Option<String>,
    pub captured_at: Option<Timestamp>,
    pub user_agent: Option<String>,
    pub client_version: Option<String>,
    pub notes: Option<String>,
}

pub async fn insert(db: &Db, capture: &NewCapture) -> Result<CaptureId> {
    let id = CaptureId::new();
    let captured_at = to_rfc3339(&capture.captured_at.unwrap_or_else(now));
    sqlx::query(
        "INSERT INTO capture
            (id, listing_id, url, method, http_status, content_type, byte_len, content_hash,
             storage_path, screenshot_path, captured_at, user_agent, client_version, notes,
             extract_status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 'pending')",
    )
    .bind(id.as_str())
    .bind(capture.listing_id.as_ref().map(|l| l.as_str()))
    .bind(capture.url.as_deref())
    .bind(capture.method.as_str())
    .bind(capture.http_status.map(|s| s as i64))
    .bind(capture.content_type.as_deref())
    .bind(capture.byte_len)
    .bind(&capture.content_hash)
    .bind(&capture.storage_path)
    .bind(capture.screenshot_path.as_deref())
    .bind(&captured_at)
    .bind(capture.user_agent.as_deref())
    .bind(capture.client_version.as_deref())
    .bind(capture.notes.as_deref())
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    Ok(id)
}

/// The most recent capture for a listing, which is what extraction reads.
///
/// `captured_at` has second precision, so two captures from one refresh can tie. The id
/// breaks the tie: UUIDv7 is time-ordered, so the larger id is the later insert.
pub async fn latest_for_listing(db: &Db, listing_id: &ListingId) -> Result<Option<CaptureId>> {
    let id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM capture WHERE listing_id = ?1
          ORDER BY captured_at DESC, id DESC LIMIT 1",
    )
    .bind(listing_id.as_str())
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    id.map(|s| s.parse()).transpose()
}

/// Has this exact content been captured before? Lets a refresh short-circuit when the page
/// has not changed, costing nothing.
pub async fn content_seen(db: &Db, listing_id: &ListingId, content_hash: &str) -> Result<bool> {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM capture WHERE listing_id = ?1 AND content_hash = ?2 LIMIT 1",
    )
    .bind(listing_id.as_str())
    .bind(content_hash)
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    Ok(existing.is_some())
}

/// Where the (compressed) body lives, so extraction can read it.
/// A capture as the extraction pipeline sees it.
#[derive(Debug, Clone)]
pub struct CaptureRow {
    pub id: CaptureId,
    pub listing_id: Option<ListingId>,
    pub url: Option<String>,
    pub method: CaptureMethod,
    pub http_status: Option<u16>,
    pub content_type: Option<String>,
    pub byte_len: i64,
    pub content_hash: String,
    pub storage_path: String,
    pub extract_status: ExtractStatus,
}

pub async fn get(db: &Db, id: &CaptureId) -> Result<Option<CaptureRow>> {
    let row = sqlx::query(
        "SELECT id, listing_id, url, method, http_status, content_type, byte_len,
                content_hash, storage_path, extract_status
           FROM capture WHERE id = ?1",
    )
    .bind(id.as_str())
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    let Some(row) = row else { return Ok(None) };
    let method: String = row.try_get("method").map_err(db_err)?;
    let status: String = row.try_get("extract_status").map_err(db_err)?;
    let listing_id: Option<String> = row.try_get("listing_id").map_err(db_err)?;
    let http_status: Option<i64> = row.try_get("http_status").map_err(db_err)?;
    Ok(Some(CaptureRow {
        id: row.try_get::<String, _>("id").map_err(db_err)?.parse()?,
        listing_id: listing_id.map(|s| s.parse()).transpose()?,
        url: row.try_get("url").map_err(db_err)?,
        method: method.parse()?,
        http_status: http_status.map(|s| s as u16),
        content_type: row.try_get("content_type").map_err(db_err)?,
        byte_len: row.try_get("byte_len").map_err(db_err)?,
        content_hash: row.try_get("content_hash").map_err(db_err)?,
        storage_path: row.try_get("storage_path").map_err(db_err)?,
        extract_status: status.parse()?,
    }))
}

pub async fn storage_path(db: &Db, id: &CaptureId) -> Result<Option<String>> {
    sqlx::query_scalar("SELECT storage_path FROM capture WHERE id = ?1")
        .bind(id.as_str())
        .fetch_optional(db.reader())
        .await
        .map_err(db_err)
}

pub async fn set_extract_status(
    db: &Db,
    id: &CaptureId,
    status: ExtractStatus,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE capture SET extract_status = ?1, extract_error = ?2 WHERE id = ?3")
        .bind(status.as_str())
        .bind(error)
        .bind(id.as_str())
        .execute(db.writer())
        .await
        .map(|_| ())
        .map_err(db_err)
}

/// Captures awaiting extraction, so a fixed adapter can be re-run over history
/// (`jobseeker reextract`). Bugs in extraction are recoverable precisely because the raw
/// bytes were persisted first.
pub async fn pending_extraction(db: &Db, limit: i64) -> Result<Vec<CaptureId>> {
    let rows = sqlx::query(
        "SELECT id FROM capture WHERE extract_status = 'pending'
          ORDER BY captured_at ASC, id ASC LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    rows.into_iter()
        .map(|r| r.try_get::<String, _>("id").map_err(db_err)?.parse())
        .collect()
}

/// Distinct blobs referenced on disk, for the storage report and for garbage collection of
/// orphaned files.
pub async fn referenced_blobs(db: &Db) -> Result<Vec<String>> {
    sqlx::query_scalar("SELECT DISTINCT storage_path FROM capture")
        .fetch_all(db.reader())
        .await
        .map_err(db_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::listing::{self, UpsertListing};
    use jobseeker_core::domain::enums::SourceKind;

    async fn a_listing(db: &Db) -> ListingId {
        listing::upsert_by_url(
            db,
            &UpsertListing {
                source: SourceKind::Greenhouse,
                url: "https://boards.greenhouse.io/acme/jobs/1".into(),
                url_canonical: "https://boards.greenhouse.io/acme/jobs/1".into(),
                source_job_id: Some("1".into()),
                title_at_source: None,
                company_name_at_source: None,
            },
        )
        .await
        .unwrap()
        .0
    }

    fn a_capture(listing_id: ListingId, hash: &str) -> NewCapture {
        NewCapture {
            listing_id: Some(listing_id),
            url: Some("https://boards.greenhouse.io/acme/jobs/1".into()),
            method: CaptureMethod::Api,
            http_status: Some(200),
            content_type: Some("application/json".into()),
            byte_len: 4096,
            content_hash: hash.into(),
            storage_path: format!("captures/ab/{hash}.json.zst"),
            screenshot_path: None,
            captured_at: None,
            user_agent: Some("jobseeker/0.1".into()),
            client_version: None,
            notes: None,
        }
    }

    #[tokio::test]
    async fn captures_accumulate_rather_than_replace() {
        let db = Db::open_in_memory().await.unwrap();
        let listing_id = a_listing(&db).await;

        insert(&db, &a_capture(listing_id.clone(), "b3:aaa"))
            .await
            .unwrap();
        let second = insert(&db, &a_capture(listing_id.clone(), "b3:bbb"))
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capture")
            .fetch_one(db.reader())
            .await
            .unwrap();
        assert_eq!(count, 2, "history is append-only");
        assert_eq!(
            latest_for_listing(&db, &listing_id).await.unwrap(),
            Some(second)
        );
    }

    #[tokio::test]
    async fn unchanged_content_is_detected_so_a_refresh_can_short_circuit() {
        let db = Db::open_in_memory().await.unwrap();
        let listing_id = a_listing(&db).await;
        insert(&db, &a_capture(listing_id.clone(), "b3:same"))
            .await
            .unwrap();

        assert!(content_seen(&db, &listing_id, "b3:same").await.unwrap());
        assert!(!content_seen(&db, &listing_id, "b3:different")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn pending_extraction_is_a_fifo_queue_that_clears() {
        let db = Db::open_in_memory().await.unwrap();
        let listing_id = a_listing(&db).await;
        let id = insert(&db, &a_capture(listing_id, "b3:aaa")).await.unwrap();

        assert_eq!(pending_extraction(&db, 10).await.unwrap(), vec![id.clone()]);
        set_extract_status(&db, &id, ExtractStatus::Ok, None)
            .await
            .unwrap();
        assert!(pending_extraction(&db, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failed_extraction_retains_its_error_and_its_bytes() {
        let db = Db::open_in_memory().await.unwrap();
        let listing_id = a_listing(&db).await;
        let id = insert(&db, &a_capture(listing_id, "b3:aaa")).await.unwrap();

        set_extract_status(
            &db,
            &id,
            ExtractStatus::Failed,
            Some("no job container found"),
        )
        .await
        .unwrap();
        let (status, error, path): (String, Option<String>, String) = sqlx::query_as(
            "SELECT extract_status, extract_error, storage_path FROM capture WHERE id = ?1",
        )
        .bind(id.as_str())
        .fetch_one(db.reader())
        .await
        .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(error.as_deref(), Some("no job container found"));
        assert!(
            !path.is_empty(),
            "the raw bytes must survive so a fixed adapter can re-run"
        );
    }

    #[tokio::test]
    async fn a_capture_outlives_its_listing() {
        let db = Db::open_in_memory().await.unwrap();
        let listing_id = a_listing(&db).await;
        insert(&db, &a_capture(listing_id.clone(), "b3:aaa"))
            .await
            .unwrap();

        sqlx::query("DELETE FROM job_source_listing WHERE id = ?1")
            .bind(listing_id.as_str())
            .execute(db.writer())
            .await
            .unwrap();

        // ON DELETE SET NULL, not CASCADE: the evidence must not vanish with the listing.
        let (count, orphaned): (i64, i64) =
            sqlx::query_as("SELECT COUNT(*), SUM(listing_id IS NULL) FROM capture")
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(count, 1);
        assert_eq!(orphaned, 1);
    }

    #[tokio::test]
    async fn referenced_blobs_are_reported_for_garbage_collection() {
        let db = Db::open_in_memory().await.unwrap();
        let listing_id = a_listing(&db).await;
        insert(&db, &a_capture(listing_id.clone(), "b3:aaa"))
            .await
            .unwrap();
        insert(&db, &a_capture(listing_id, "b3:bbb")).await.unwrap();

        let blobs = referenced_blobs(&db).await.unwrap();
        assert_eq!(blobs.len(), 2);
        assert!(blobs.iter().all(|b| b.starts_with("captures/")));
    }
}
