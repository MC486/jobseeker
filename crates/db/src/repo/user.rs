//! Owner account: username + Argon2id password hash.
//!
//! Parameters match `docs/13-security-privacy-legal.md`: Argon2id, m=19 MiB, t=2, p=1.
//! A dummy hash is verified when the username is unknown so responses stay constant-time.

use std::sync::OnceLock;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use jobseeker_core::time::{now, to_rfc3339};
use jobseeker_core::{Error, Result};
use sqlx::Row;

use crate::{db_err, Db};

const MIN_PASSWORD_LEN: usize = 8;

#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub role: String,
    pub disabled: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

impl UserRow {
    pub fn is_owner(&self) -> bool {
        self.role == "owner"
    }
}

fn argon2() -> Argon2<'static> {
    let params = Params::new(19 * 1024, 2, 1, None).expect("documented Argon2id params");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub fn hash_password(password: &str) -> Result<String> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(Error::BadRequest(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    let salt = SaltString::encode_b64(uuid::Uuid::now_v7().as_bytes())
        .map_err(|e| Error::Internal(format!("argon2 salt: {e}")))?;
    argon2()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Internal(format!("argon2: {e}")))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    argon2()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| hash_password("dummy-password-not-used").expect("dummy argon2 hash"))
}

/// Normalize a login name: trim, lowercase, reject empty / oversized / odd characters.
pub fn normalize_username(raw: &str) -> Result<String> {
    let name = raw.trim().to_ascii_lowercase();
    if name.is_empty() || name.len() > 64 {
        return Err(Error::BadRequest("username is required".into()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
    {
        return Err(Error::BadRequest(
            "username may contain letters, digits, '.', '_' and '-'".into(),
        ));
    }
    Ok(name)
}

/// Create the owner user or rotate their password. Single-user: one owner is enough.
pub async fn upsert_owner(db: &Db, username: &str, password: &str) -> Result<UserRow> {
    let username = normalize_username(username)?;
    let password_hash = hash_password(password)?;
    let ts = to_rfc3339(&now());
    if let Some(existing) = get_by_username(db, &username).await? {
        sqlx::query("UPDATE user SET password_hash = ?2 WHERE id = ?1")
            .bind(&existing.id)
            .bind(&password_hash)
            .execute(db.writer())
            .await
            .map_err(db_err)?;
        return Ok(UserRow {
            password_hash,
            ..existing
        });
    }
    let id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO user (id, username, password_hash, role, disabled, created_at)
         VALUES (?1, ?2, ?3, 'owner', 0, ?4)",
    )
    .bind(&id)
    .bind(&username)
    .bind(&password_hash)
    .bind(&ts)
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    Ok(UserRow {
        id,
        username,
        password_hash,
        role: "owner".into(),
        disabled: false,
        created_at: ts,
        last_login_at: None,
    })
}

pub async fn get_by_username(db: &Db, username: &str) -> Result<Option<UserRow>> {
    let username = normalize_username(username)?;
    let row = sqlx::query(
        "SELECT id, username, password_hash, role, disabled, created_at, last_login_at
           FROM user WHERE username = ?1",
    )
    .bind(&username)
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    row.map(row_from).transpose()
}

pub async fn get(db: &Db, id: &str) -> Result<Option<UserRow>> {
    let row = sqlx::query(
        "SELECT id, username, password_hash, role, disabled, created_at, last_login_at
           FROM user WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    row.map(row_from).transpose()
}

pub async fn count(db: &Db) -> Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM user")
        .fetch_one(db.reader())
        .await
        .map_err(db_err)
}

/// Verify username + password. Always runs Argon2id so a miss does not leak existence.
pub async fn authenticate(db: &Db, username: &str, password: &str) -> Result<UserRow> {
    let looked_up = match normalize_username(username) {
        Ok(name) => get_by_username(db, &name).await?,
        Err(_) => None,
    };
    let hash = looked_up
        .as_ref()
        .map(|u| u.password_hash.as_str())
        .unwrap_or_else(|| dummy_hash());
    let ok = verify_password(password, hash);
    let Some(user) = looked_up else {
        return Err(Error::Unauthorized);
    };
    if !ok || user.disabled {
        return Err(Error::Unauthorized);
    }
    Ok(user)
}

pub async fn record_login(db: &Db, user_id: &str) -> Result<()> {
    sqlx::query("UPDATE user SET last_login_at = ?2 WHERE id = ?1")
        .bind(user_id)
        .bind(to_rfc3339(&now()))
        .execute(db.writer())
        .await
        .map(|_| ())
        .map_err(db_err)
}

fn row_from(row: sqlx::sqlite::SqliteRow) -> Result<UserRow> {
    let disabled: i64 = row.try_get("disabled").map_err(db_err)?;
    Ok(UserRow {
        id: row.try_get("id").map_err(db_err)?,
        username: row.try_get("username").map_err(db_err)?,
        password_hash: row.try_get("password_hash").map_err(db_err)?,
        role: row.try_get("role").map_err(db_err)?,
        disabled: disabled != 0,
        created_at: row.try_get("created_at").map_err(db_err)?,
        last_login_at: row.try_get("last_login_at").map_err(db_err)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_passwords_are_rejected_before_hashing() {
        assert!(hash_password("short").is_err());
    }

    #[test]
    fn a_hash_verifies_only_the_original_password() {
        let hash = hash_password("correct-horse").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct-horse", &hash));
        assert!(!verify_password("wrong-password", &hash));
    }

    #[tokio::test]
    async fn upsert_creates_then_rotates() {
        let db = Db::open_in_memory().await.unwrap();
        let first = upsert_owner(&db, "Owner", "first-password").await.unwrap();
        assert_eq!(first.username, "owner");
        assert_eq!(count(&db).await.unwrap(), 1);
        authenticate(&db, "owner", "first-password").await.unwrap();

        upsert_owner(&db, "owner", "second-password").await.unwrap();
        assert_eq!(count(&db).await.unwrap(), 1);
        assert!(authenticate(&db, "owner", "first-password").await.is_err());
        authenticate(&db, "OWNER", "second-password").await.unwrap();
    }

    #[tokio::test]
    async fn unknown_user_is_unauthorized_not_not_found() {
        let db = Db::open_in_memory().await.unwrap();
        let err = authenticate(&db, "nobody", "long-enough")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "unauthorized");
    }
}
