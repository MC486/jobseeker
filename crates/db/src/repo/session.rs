//! Browser sessions. The cookie value is never stored; only its BLAKE3 hash is.

use jobseeker_core::time::{now, to_rfc3339};
use jobseeker_core::Result;
use sqlx::Row;

use crate::repo::token::hash_secret;
use crate::repo::user::UserRow;
use crate::{db_err, Db};

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: String,
    pub user_id: String,
    pub created_at: String,
    pub expires_at: String,
    pub last_seen_at: String,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IssuedSession {
    /// Plaintext cookie value, shown once. Prefixed `jss_` so it is greppable.
    pub token: String,
    pub row: SessionRow,
}

fn generate_token() -> String {
    format!(
        "jss_{}{}",
        uuid::Uuid::now_v7().simple(),
        uuid::Uuid::now_v7().simple()
    )
}

pub async fn issue(
    db: &Db,
    user: &UserRow,
    ttl_days: u32,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> Result<IssuedSession> {
    let token = generate_token();
    let id = hash_secret(&token);
    let created = now();
    let ttl = i64::from(ttl_days.max(1));
    let expires = created + chrono::Duration::days(ttl);
    let created_s = to_rfc3339(&created);
    let expires_s = to_rfc3339(&expires);
    sqlx::query(
        "INSERT INTO session (id, user_id, created_at, expires_at, last_seen_at, user_agent, ip)
         VALUES (?1, ?2, ?3, ?4, ?3, ?5, ?6)",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&created_s)
    .bind(&expires_s)
    .bind(user_agent)
    .bind(ip)
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    Ok(IssuedSession {
        token,
        row: SessionRow {
            id,
            user_id: user.id.clone(),
            created_at: created_s.clone(),
            expires_at: expires_s,
            last_seen_at: created_s,
            user_agent: user_agent.map(str::to_string),
            ip: ip.map(str::to_string),
            revoked_at: None,
        },
    })
}

pub async fn lookup(db: &Db, token: &str) -> Result<Option<(SessionRow, UserRow)>> {
    let token = token.trim();
    if !token.starts_with("jss_") {
        return Ok(None);
    }
    let now_s = to_rfc3339(&now());
    let row = sqlx::query(
        "SELECT id, user_id, created_at, expires_at, last_seen_at, user_agent, ip, revoked_at
           FROM session WHERE id = ?1",
    )
    .bind(hash_secret(token))
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let session = session_from(row)?;
    if session.revoked_at.is_some() || session.expires_at < now_s {
        return Ok(None);
    }
    let Some(user) = crate::repo::user::get(db, &session.user_id).await? else {
        return Ok(None);
    };
    if user.disabled {
        return Ok(None);
    }
    Ok(Some((session, user)))
}

pub async fn touch(db: &Db, id: &str) -> Result<()> {
    sqlx::query("UPDATE session SET last_seen_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind(to_rfc3339(&now()))
        .execute(db.writer())
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn revoke(db: &Db, id: &str) -> Result<u64> {
    let result =
        sqlx::query("UPDATE session SET revoked_at = COALESCE(revoked_at, ?2) WHERE id = ?1")
            .bind(id)
            .bind(to_rfc3339(&now()))
            .execute(db.writer())
            .await
            .map_err(db_err)?;
    Ok(result.rows_affected())
}

pub async fn revoke_token(db: &Db, token: &str) -> Result<u64> {
    if !token.starts_with("jss_") {
        return Ok(0);
    }
    revoke(db, &hash_secret(token)).await
}

fn session_from(row: sqlx::sqlite::SqliteRow) -> Result<SessionRow> {
    Ok(SessionRow {
        id: row.try_get("id").map_err(db_err)?,
        user_id: row.try_get("user_id").map_err(db_err)?,
        created_at: row.try_get("created_at").map_err(db_err)?,
        expires_at: row.try_get("expires_at").map_err(db_err)?,
        last_seen_at: row.try_get("last_seen_at").map_err(db_err)?,
        user_agent: row.try_get("user_agent").map_err(db_err)?,
        ip: row.try_get("ip").map_err(db_err)?,
        revoked_at: row.try_get("revoked_at").map_err(db_err)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::user;

    #[tokio::test]
    async fn issued_session_round_trips_and_revoke_hides_it() {
        let db = Db::open_in_memory().await.unwrap();
        let user = user::upsert_owner(&db, "owner", "long-password")
            .await
            .unwrap();
        let issued = issue(&db, &user, 30, Some("test"), Some("127.0.0.1"))
            .await
            .unwrap();
        assert!(issued.token.starts_with("jss_"));
        assert_ne!(issued.row.id, issued.token);
        assert!(issued.row.id.starts_with("b3:"));
        let found = lookup(&db, &issued.token).await.unwrap().unwrap();
        assert_eq!(found.1.username, "owner");
        assert_eq!(revoke_token(&db, &issued.token).await.unwrap(), 1);
        assert!(lookup(&db, &issued.token).await.unwrap().is_none());
    }

    #[test]
    fn session_tokens_are_hashed_like_device_tokens() {
        let t = "jss_abc";
        assert_eq!(
            hash_secret(t),
            jobseeker_core::hash::content_hash(t.as_bytes())
        );
    }
}
