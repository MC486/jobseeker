//! Device tokens and one-time pairing codes.
//!
//! Plaintext secrets are returned once and stored as `b3:` hashes. A database dump
//! cannot mint a usable `Authorization: Bearer` header.

use jobseeker_core::hash::content_hash;
use jobseeker_core::time::{now, to_rfc3339};
use jobseeker_core::{Error, Result};
use serde::Serialize;
use sqlx::Row;

use crate::{db_err, Db};

const PAIRING_TTL_SECS: i64 = 15 * 60;
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

#[derive(Debug, Clone, Serialize)]
pub struct DeviceTokenRow {
    pub id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IssuedToken {
    pub token: String,
    pub row: DeviceTokenRow,
}

#[derive(Debug, Clone)]
pub struct IssuedPairingCode {
    pub code: String,
    pub name: String,
    pub expires_at: String,
}

/// Hash the secret the same way sessions will: BLAKE3, prefixed `b3:`.
pub fn hash_secret(secret: &str) -> String {
    content_hash(secret.as_bytes())
}

fn generate_token() -> String {
    format!(
        "jst_{}{}",
        uuid::Uuid::now_v7().simple(),
        uuid::Uuid::now_v7().simple()
    )
}

fn generate_code() -> String {
    let uuid = uuid::Uuid::now_v7();
    let bytes = uuid.as_bytes();
    let mut out = String::with_capacity(9);
    for (i, b) in bytes.iter().take(8).enumerate() {
        if i == 4 {
            out.push('-');
        }
        out.push(CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char);
    }
    out
}

/// Normalize a pairing code so `7k2m-9qwx` and `7K2M9QWX` match.
pub fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Print a one-time pairing code. The extension exchanges it via `POST /auth/pair`.
pub async fn create_pairing_code(db: &Db, name: &str) -> Result<IssuedPairingCode> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::BadRequest("device name is required".into()));
    }
    let code = generate_code();
    let id = uuid::Uuid::now_v7().to_string();
    let created = now();
    let expires = created + chrono::Duration::seconds(PAIRING_TTL_SECS);
    sqlx::query(
        "INSERT INTO pairing_code (id, code_hash, name, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&id)
    .bind(hash_secret(&normalize_code(&code)))
    .bind(name)
    .bind(to_rfc3339(&created))
    .bind(to_rfc3339(&expires))
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    Ok(IssuedPairingCode {
        code,
        name: name.to_string(),
        expires_at: to_rfc3339(&expires),
    })
}

/// Consume a pairing code and mint an ingest-scoped device token.
pub async fn exchange_pairing_code(db: &Db, code: &str) -> Result<IssuedToken> {
    let normalized = normalize_code(code);
    if normalized.len() != 8 {
        return Err(Error::BadRequest("malformed pairing code".into()));
    }
    let hash = hash_secret(&normalized);
    let now_s = to_rfc3339(&now());
    let row =
        sqlx::query("SELECT id, name, expires_at, used_at FROM pairing_code WHERE code_hash = ?1")
            .bind(&hash)
            .fetch_optional(db.reader())
            .await
            .map_err(db_err)?;
    let Some(row) = row else {
        return Err(Error::Unauthorized);
    };
    let used_at: Option<String> = row.try_get("used_at").map_err(db_err)?;
    if used_at.is_some() {
        return Err(Error::Unauthorized);
    }
    let expires_at: String = row.try_get("expires_at").map_err(db_err)?;
    if expires_at < now_s {
        return Err(Error::Unauthorized);
    }
    let pairing_id: String = row.try_get("id").map_err(db_err)?;
    let name: String = row.try_get("name").map_err(db_err)?;

    let issued = issue_token(db, &name, &["ingest".into()]).await?;
    sqlx::query("UPDATE pairing_code SET used_at = ?2 WHERE id = ?1")
        .bind(&pairing_id)
        .bind(&now_s)
        .execute(db.writer())
        .await
        .map_err(db_err)?;
    Ok(issued)
}

/// Mint a device token. The plaintext is returned once.
pub async fn issue_token(db: &Db, name: &str, scopes: &[String]) -> Result<IssuedToken> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::BadRequest("device name is required".into()));
    }
    let scopes = if scopes.is_empty() {
        vec!["ingest".to_string()]
    } else {
        scopes.to_vec()
    };
    let token = generate_token();
    let id = uuid::Uuid::now_v7().to_string();
    let ts = to_rfc3339(&now());
    let scopes_json = serde_json::to_string(&scopes)?;
    sqlx::query(
        "INSERT INTO device_token (id, token_hash, name, scopes_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&id)
    .bind(hash_secret(&token))
    .bind(name)
    .bind(&scopes_json)
    .bind(&ts)
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    Ok(IssuedToken {
        token,
        row: DeviceTokenRow {
            id,
            name: name.to_string(),
            scopes,
            created_at: ts,
            last_used_at: None,
            expires_at: None,
            revoked_at: None,
        },
    })
}

pub async fn lookup(db: &Db, token: &str) -> Result<Option<DeviceTokenRow>> {
    let token = token.trim();
    if !token.starts_with("jst_") {
        return Ok(None);
    }
    let row = sqlx::query(
        "SELECT id, name, scopes_json, created_at, last_used_at, expires_at, revoked_at
           FROM device_token WHERE token_hash = ?1",
    )
    .bind(hash_secret(token))
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    let Some(row) = row else { return Ok(None) };
    let revoked: Option<String> = row.try_get("revoked_at").map_err(db_err)?;
    if revoked.is_some() {
        return Ok(None);
    }
    let expires: Option<String> = row.try_get("expires_at").map_err(db_err)?;
    let now_s = to_rfc3339(&now());
    if expires.as_deref().is_some_and(|e| e < now_s.as_str()) {
        return Ok(None);
    }
    Ok(Some(row_from(row)?))
}

pub async fn touch(db: &Db, id: &str) -> Result<()> {
    sqlx::query("UPDATE device_token SET last_used_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind(to_rfc3339(&now()))
        .execute(db.writer())
        .await
        .map(|_| ())
        .map_err(db_err)
}

pub async fn list(db: &Db) -> Result<Vec<DeviceTokenRow>> {
    let rows = sqlx::query(
        "SELECT id, name, scopes_json, created_at, last_used_at, expires_at, revoked_at
           FROM device_token
          ORDER BY created_at DESC",
    )
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    rows.into_iter().map(row_from).collect()
}

pub async fn revoke(db: &Db, id: &str) -> Result<u64> {
    let result =
        sqlx::query("UPDATE device_token SET revoked_at = COALESCE(revoked_at, ?2) WHERE id = ?1")
            .bind(id)
            .bind(to_rfc3339(&now()))
            .execute(db.writer())
            .await
            .map_err(db_err)?;
    Ok(result.rows_affected())
}

fn row_from(row: sqlx::sqlite::SqliteRow) -> Result<DeviceTokenRow> {
    let scopes_json: String = row.try_get("scopes_json").map_err(db_err)?;
    let scopes: Vec<String> = serde_json::from_str(&scopes_json).unwrap_or_default();
    Ok(DeviceTokenRow {
        id: row.try_get("id").map_err(db_err)?,
        name: row.try_get("name").map_err(db_err)?,
        scopes,
        created_at: row.try_get("created_at").map_err(db_err)?,
        last_used_at: row.try_get("last_used_at").map_err(db_err)?,
        expires_at: row.try_get("expires_at").map_err(db_err)?,
        revoked_at: row.try_get("revoked_at").map_err(db_err)?,
    })
}

impl DeviceTokenRow {
    pub fn allows(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == "*" || s == scope)
    }

    pub fn is_owner(&self) -> bool {
        self.scopes
            .iter()
            .any(|s| s == "*" || s == "admin" || s == "read")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn issued_token_round_trips_by_hash_not_plaintext() {
        let db = Db::open_in_memory().await.unwrap();
        let issued = issue_token(&db, "Firefox on laptop", &["ingest".into()])
            .await
            .unwrap();
        assert!(issued.token.starts_with("jst_"));
        let stored: String =
            sqlx::query_scalar("SELECT token_hash FROM device_token WHERE id = ?1")
                .bind(&issued.row.id)
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert!(
            !stored.contains(&issued.token),
            "plaintext must not be stored"
        );
        assert!(lookup(&db, &issued.token).await.unwrap().is_some());
        assert!(lookup(&db, "jst_not-a-real-token").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn pairing_code_exchanges_once_and_then_fails() {
        let db = Db::open_in_memory().await.unwrap();
        let code = create_pairing_code(&db, "Phone").await.unwrap();
        assert_eq!(normalize_code(&code.code).len(), 8);
        let first = exchange_pairing_code(&db, &code.code.to_ascii_lowercase())
            .await
            .unwrap();
        assert!(first.row.allows("ingest"));
        assert!(!first.row.is_owner());
        assert!(exchange_pairing_code(&db, &code.code).await.is_err());
    }

    #[tokio::test]
    async fn revoke_hides_the_token() {
        let db = Db::open_in_memory().await.unwrap();
        let issued = issue_token(&db, "old laptop", &["*".into()]).await.unwrap();
        assert_eq!(revoke(&db, &issued.row.id).await.unwrap(), 1);
        assert!(lookup(&db, &issued.token).await.unwrap().is_none());
    }

    #[test]
    fn codes_normalize_dashes_and_case() {
        assert_eq!(normalize_code("7k2m-9qwx"), "7K2M9QWX");
        assert_eq!(normalize_code(" 7K2M 9QWX "), "7K2M9QWX");
    }
}
