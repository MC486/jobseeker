//! Generated resumes and cover letters. Each write is a new version.

use jobseeker_core::ids::{DocumentId, JobId, ProfileId};
use jobseeker_core::time::{now, to_rfc3339};
use jobseeker_core::Result;
use serde::Serialize;
use sqlx::Row;

use crate::{db_err, Db};

#[derive(Debug, Clone, Serialize)]
pub struct DocumentRow {
    pub id: String,
    pub job_id: String,
    pub profile_id: String,
    pub kind: String,
    pub title: String,
    pub format: String,
    pub source_content: String,
    pub version: i64,
    pub parent_document_id: Option<String>,
    pub selection_json: serde_json::Value,
    pub coverage_json: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct NewDocument {
    pub profile_id: ProfileId,
    pub job_id: JobId,
    pub kind: String,
    pub title: String,
    pub source_content: String,
    pub selection_json: String,
    pub coverage_json: String,
}

pub async fn list_for_job(db: &Db, job_id: &JobId) -> Result<Vec<DocumentRow>> {
    let rows = sqlx::query(
        "SELECT id, job_id, profile_id, kind, title, format, source_content, version,
                parent_document_id, selection_json, coverage_json, created_at
           FROM document
          WHERE job_id = ?1
          ORDER BY created_at DESC, id DESC",
    )
    .bind(job_id.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    rows.iter().map(read_row).collect()
}

pub async fn insert_version(db: &Db, new: &NewDocument) -> Result<DocumentRow> {
    let mut tx = db.writer().begin().await.map_err(db_err)?;
    let version: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM document
          WHERE job_id = ?1 AND profile_id = ?2 AND kind = ?3",
    )
    .bind(new.job_id.as_str())
    .bind(new.profile_id.as_str())
    .bind(&new.kind)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    let parent: Option<String> = sqlx::query_scalar(
        "SELECT id FROM document
          WHERE job_id = ?1 AND profile_id = ?2 AND kind = ?3
          ORDER BY version DESC, id DESC
          LIMIT 1",
    )
    .bind(new.job_id.as_str())
    .bind(new.profile_id.as_str())
    .bind(&new.kind)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    let id = DocumentId::new();
    let ts = to_rfc3339(&now());
    sqlx::query(
        "INSERT INTO document (
            id, profile_id, job_id, kind, title, template, format, source_content,
            version, parent_document_id, selection_json, coverage_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'bank', 'markdown', ?6, ?7, ?8, ?9, ?10, ?11)",
    )
    .bind(id.as_str())
    .bind(new.profile_id.as_str())
    .bind(new.job_id.as_str())
    .bind(&new.kind)
    .bind(&new.title)
    .bind(&new.source_content)
    .bind(version)
    .bind(parent.as_deref())
    .bind(&new.selection_json)
    .bind(&new.coverage_json)
    .bind(&ts)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    let rows = list_for_job(db, &new.job_id).await?;
    rows.into_iter()
        .find(|r| r.id == id.as_str())
        .ok_or_else(|| jobseeker_core::Error::Internal("document insert vanished".into()))
}

fn read_row(r: &sqlx::sqlite::SqliteRow) -> Result<DocumentRow> {
    let selection: Option<String> = r.try_get("selection_json").map_err(db_err)?;
    let coverage: Option<String> = r.try_get("coverage_json").map_err(db_err)?;
    Ok(DocumentRow {
        id: r.try_get("id").map_err(db_err)?,
        job_id: r.try_get("job_id").map_err(db_err)?,
        profile_id: r.try_get("profile_id").map_err(db_err)?,
        kind: r.try_get("kind").map_err(db_err)?,
        title: r.try_get("title").map_err(db_err)?,
        format: r.try_get("format").map_err(db_err)?,
        source_content: r.try_get("source_content").map_err(db_err)?,
        version: r.try_get("version").map_err(db_err)?,
        parent_document_id: r.try_get("parent_document_id").map_err(db_err)?,
        selection_json: selection
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null),
        coverage_json: coverage
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null),
        created_at: r.try_get("created_at").map_err(db_err)?,
    })
}
