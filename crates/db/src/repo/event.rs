//! Durable `event_log` writes for SSE backfill.

use jobseeker_core::domain::event::DomainEvent;
use jobseeker_core::time::{now, to_rfc3339};
use jobseeker_core::Result;
use sqlx::Row;

use crate::{db_err, Db};

/// Append one domain event and return it with the assigned `id`.
pub async fn append(
    db: &Db,
    kind: &str,
    entity_kind: &str,
    entity_id: &str,
    payload: serde_json::Value,
) -> Result<DomainEvent> {
    let created_at = to_rfc3339(&now());
    let payload_s = serde_json::to_string(&payload)?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO event_log (kind, entity_kind, entity_id, payload_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         RETURNING id",
    )
    .bind(kind)
    .bind(entity_kind)
    .bind(entity_id)
    .bind(&payload_s)
    .bind(&created_at)
    .fetch_one(db.writer())
    .await
    .map_err(db_err)?;
    Ok(DomainEvent {
        id,
        kind: kind.to_string(),
        entity_kind: Some(entity_kind.to_string()),
        entity_id: Some(entity_id.to_string()),
        payload,
        created_at,
    })
}

/// Events with `id > after_id`, oldest first. Caps at 500 so a reconnect cannot
/// dump the whole log into one SSE burst.
pub async fn since(db: &Db, after_id: i64, limit: i64) -> Result<Vec<DomainEvent>> {
    let limit = limit.clamp(1, 500);
    let rows = sqlx::query(
        "SELECT id, kind, entity_kind, entity_id, payload_json, created_at
         FROM event_log
         WHERE id > ?1
         ORDER BY id ASC
         LIMIT ?2",
    )
    .bind(after_id)
    .bind(limit)
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let payload_s: Option<String> = row.try_get("payload_json").map_err(db_err)?;
        let payload = match payload_s.as_deref() {
            Some(s) if !s.is_empty() => serde_json::from_str(s).unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        };
        out.push(DomainEvent {
            id: row.try_get("id").map_err(db_err)?,
            kind: row.try_get("kind").map_err(db_err)?,
            entity_kind: row.try_get("entity_kind").map_err(db_err)?,
            entity_id: row.try_get("entity_id").map_err(db_err)?,
            payload,
            created_at: row.try_get("created_at").map_err(db_err)?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn append_then_since_returns_only_newer_rows() {
        let db = Db::open_in_memory().await.unwrap();
        let first = append(
            &db,
            DomainEvent::JOB_CREATED,
            "job",
            "j1",
            serde_json::json!({"job_id": "j1"}),
        )
        .await
        .unwrap();
        let second = append(
            &db,
            DomainEvent::MATCH_UPDATED,
            "job",
            "j1",
            serde_json::json!({"overall": 0.7}),
        )
        .await
        .unwrap();
        assert!(second.id > first.id);

        let after_first = since(&db, first.id, 50).await.unwrap();
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].kind, DomainEvent::MATCH_UPDATED);
        assert_eq!(after_first[0].payload["overall"], 0.7);

        let none = since(&db, second.id, 50).await.unwrap();
        assert!(none.is_empty());
    }
}
