//! SQLite storage: connection pools, migrations and repositories.
//!
//! Two pools, because SQLite has exactly one writer. Routing every mutation through a
//! single-connection writer pool makes `SQLITE_BUSY` structurally impossible instead of
//! something to retry (see `docs/03-architecture.md` §3.2).

pub mod persist;
pub mod queue;
pub mod repo;

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use jobseeker_core::{Error, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Pool, Sqlite};

pub type SqlitePool = Pool<Sqlite>;

/// Migrations are embedded in the binary so a deployment is one artifact.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Handle to the database. Cheap to clone: the pools are `Arc` internally.
#[derive(Clone)]
pub struct Db {
    /// Single connection. All writes go through here.
    writer: SqlitePool,
    /// WAL readers, which never block on the writer.
    reader: SqlitePool,
}

impl Db {
    /// Open (creating if absent) the database at `path` and configure both pools.
    pub async fn open(path: &Path, reader_connections: u32) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(30))
            .connect_with(connect_options(path, false)?)
            .await
            .map_err(db_err)?;

        let reader = SqlitePoolOptions::new()
            .max_connections(reader_connections.max(1))
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(connect_options(path, true)?)
            .await
            .map_err(db_err)?;

        Ok(Self { writer, reader })
    }

    /// An in-memory database for tests that do not exercise file locking or WAL behaviour.
    /// Repository tests should prefer [`Db::open`] against a temp file, because those are
    /// exactly the properties production depends on.
    pub async fn open_in_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(db_err)?
            .foreign_keys(true);
        // A shared single connection: `:memory:` databases are per-connection, so a pool of
        // several would each get their own empty database.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await
            .map_err(db_err)?;
        let db = Self {
            writer: pool.clone(),
            reader: pool,
        };
        db.migrate().await?;
        Ok(db)
    }

    /// Pool for mutations.
    pub fn writer(&self) -> &SqlitePool {
        &self.writer
    }

    /// Pool for queries.
    pub fn reader(&self) -> &SqlitePool {
        &self.reader
    }

    /// Apply all pending migrations. Forward-only and checksummed by sqlx.
    pub async fn migrate(&self) -> Result<()> {
        MIGRATOR
            .run(&self.writer)
            .await
            .map_err(|e| Error::Migration(e.to_string()))
    }

    /// Number of migrations this binary knows about, reported by `/api/v1/meta`.
    pub fn known_migrations() -> usize {
        MIGRATOR.migrations.len()
    }

    /// Highest migration version applied to this database.
    pub async fn applied_version(&self) -> Result<Option<i64>> {
        // The table does not exist before the first migration runs.
        let exists: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'",
        )
        .fetch_optional(&self.reader)
        .await
        .map_err(db_err)?;
        if exists.is_none() {
            return Ok(None);
        }
        sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
            .fetch_one(&self.reader)
            .await
            .map_err(db_err)
    }

    /// Refuse to run against a database written by a newer binary (NFR-O-06): failing loudly
    /// beats silently corrupting data an older schema cannot represent.
    pub async fn assert_schema_not_newer(&self) -> Result<()> {
        let applied = self.applied_version().await?;
        let known = MIGRATOR.migrations.last().map(|m| m.version);
        match (applied, known) {
            (Some(applied), Some(known)) if applied > known => Err(Error::Migration(format!(
                "database schema version {applied} is newer than this binary understands \
                 ({known}); upgrade jobseeker instead of downgrading the data"
            ))),
            _ => Ok(()),
        }
    }

    /// Liveness of the connection, for `/readyz`.
    pub async fn ping(&self) -> Result<()> {
        sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(&self.reader)
            .await
            .map(|_| ())
            .map_err(db_err)
    }

    /// Consistent hot snapshot for backups, without stopping the service.
    pub async fn vacuum_into(&self, dest: &Path) -> Result<()> {
        let sql = format!(
            "VACUUM INTO '{}'",
            dest.display().to_string().replace('\'', "''")
        );
        // SQLite cannot bind a filename here. `dest` is an operator-supplied path and single
        // quotes are doubled above, so the literal cannot be closed early.
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&self.writer)
            .await
            .map(|_| ())
            .map_err(db_err)
    }

    pub async fn close(&self) {
        self.writer.close().await;
        self.reader.close().await;
    }
}

fn connect_options(path: &Path, read_only: bool) -> Result<SqliteConnectOptions> {
    let mut opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(!read_only)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        // Safe under WAL for this workload: a power loss can cost the last transaction, not
        // the database. Roughly an order of magnitude faster than FULL on small writes.
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .pragma("temp_store", "MEMORY")
        .pragma("cache_size", "-65536") // 64 MiB per connection
        .pragma("mmap_size", "268435456") // 256 MiB
        .pragma("wal_autocheckpoint", "1000");
    if read_only {
        opts = opts.read_only(true);
    }
    Ok(opts)
}

pub(crate) fn db_err(e: sqlx::Error) -> Error {
    match &e {
        sqlx::Error::RowNotFound => Error::NotFound("row"),
        sqlx::Error::Database(dbe) if dbe.message().contains("UNIQUE constraint failed") => {
            Error::Conflict(dbe.message().to_string())
        }
        _ => Error::Database(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_apply_to_an_empty_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("test.db"), 4).await.unwrap();
        assert_eq!(db.applied_version().await.unwrap(), None);
        db.migrate().await.unwrap();
        assert!(db.applied_version().await.unwrap().is_some());
        db.ping().await.unwrap();
        db.assert_schema_not_newer().await.unwrap();
    }

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Db::open(&path, 4).await.unwrap();
        db.migrate().await.unwrap();
        let first = db.applied_version().await.unwrap();
        db.migrate().await.unwrap();
        assert_eq!(db.applied_version().await.unwrap(), first);
    }

    #[tokio::test]
    async fn seed_sources_are_present_with_sane_fidelity() {
        let db = Db::open_in_memory().await.unwrap();
        let (kind, fidelity): (String, i64) =
            sqlx::query_as("SELECT kind, fidelity FROM source ORDER BY fidelity DESC LIMIT 1")
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(kind, "manual");
        assert_eq!(fidelity, 100);

        let greenhouse: i64 =
            sqlx::query_scalar("SELECT fidelity FROM source WHERE kind = 'greenhouse'")
                .fetch_one(db.reader())
                .await
                .unwrap();
        let linkedin: i64 =
            sqlx::query_scalar("SELECT fidelity FROM source WHERE kind = 'linkedin'")
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert!(
            greenhouse > linkedin,
            "an employer's ATS must outrank an aggregator"
        );
    }

    #[tokio::test]
    async fn foreign_keys_and_checks_are_enforced() {
        let db = Db::open_in_memory().await.unwrap();

        // Foreign keys on: a job cannot reference a company that does not exist.
        let orphan = sqlx::query(
            "INSERT INTO job (id, company_id, slug, title, title_normalized, content_hash,
                              first_seen_at, last_seen_at, created_at, updated_at)
             VALUES ('a', 'nope', 's', 't', 't', 'h', 'now', 'now', 'now', 'now')",
        )
        .execute(db.writer())
        .await;
        assert!(orphan.is_err(), "foreign keys must be enforced");

        // CHECK constraints catch enum typos at the boundary.
        sqlx::query(
            "INSERT INTO company (id, name, slug, name_normalized, created_at, updated_at)
             VALUES ('c1', 'Acme', 'acme', 'acme', 'now', 'now')",
        )
        .execute(db.writer())
        .await
        .unwrap();
        let bad_enum = sqlx::query(
            "INSERT INTO job (id, company_id, slug, title, title_normalized, content_hash,
                              status, first_seen_at, last_seen_at, created_at, updated_at)
             VALUES ('j1', 'c1', 's', 't', 't', 'h', 'kinda-open', 'now', 'now', 'now', 'now')",
        )
        .execute(db.writer())
        .await;
        assert!(bad_enum.is_err(), "status CHECK must reject unknown values");
    }

    #[tokio::test]
    async fn requirements_cascade_with_their_job() {
        let db = Db::open_in_memory().await.unwrap();
        sqlx::query(
            "INSERT INTO company (id, name, slug, name_normalized, created_at, updated_at)
             VALUES ('c1', 'Acme', 'acme', 'acme', 'now', 'now');
             INSERT INTO job (id, company_id, slug, title, title_normalized, content_hash,
                              first_seen_at, last_seen_at, created_at, updated_at)
             VALUES ('j1', 'c1', 's', 't', 't', 'h', 'now', 'now', 'now', 'now');
             INSERT INTO requirement (id, job_id, text, normalized_text, kind, necessity,
                                      created_at, updated_at)
             VALUES ('r1', 'j1', 'Rust', 'rust', 'skill', 'required', 'now', 'now');",
        )
        .execute(db.writer())
        .await
        .unwrap();

        sqlx::query("DELETE FROM job WHERE id = 'j1'")
            .execute(db.writer())
            .await
            .unwrap();
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM requirement")
            .fetch_one(db.reader())
            .await
            .unwrap();
        assert_eq!(remaining, 0, "requirements must not outlive their job");
    }

    #[tokio::test]
    async fn a_job_cannot_repeat_a_requirement() {
        let db = Db::open_in_memory().await.unwrap();
        sqlx::query(
            "INSERT INTO company (id, name, slug, name_normalized, created_at, updated_at)
             VALUES ('c1', 'Acme', 'acme', 'acme', 'now', 'now');
             INSERT INTO job (id, company_id, slug, title, title_normalized, content_hash,
                              first_seen_at, last_seen_at, created_at, updated_at)
             VALUES ('j1', 'c1', 's', 't', 't', 'h', 'now', 'now', 'now', 'now');
             INSERT INTO requirement (id, job_id, text, normalized_text, kind, necessity,
                                      created_at, updated_at)
             VALUES ('r1', 'j1', '5+ years Rust', 'years rust', 'skill', 'required', 'now', 'now');",
        )
        .execute(db.writer())
        .await
        .unwrap();

        let dup = sqlx::query(
            "INSERT INTO requirement (id, job_id, text, normalized_text, kind, necessity,
                                      created_at, updated_at)
             VALUES ('r2', 'j1', 'Rust, 5 years', 'years rust', 'skill', 'required', 'now', 'now')",
        )
        .execute(db.writer())
        .await;
        assert!(matches!(dup.map_err(db_err), Err(Error::Conflict(_))));
    }

    #[tokio::test]
    async fn json_columns_reject_malformed_json() {
        let db = Db::open_in_memory().await.unwrap();
        let bad = sqlx::query(
            "INSERT INTO task (id, kind, payload_json, available_at, created_at, updated_at)
             VALUES ('t1', 'maintenance', 'not json', 'now', 'now', 'now')",
        )
        .execute(db.writer())
        .await;
        assert!(bad.is_err(), "json_valid CHECK must reject non-JSON");
    }

    #[tokio::test]
    async fn only_one_profile_can_be_default() {
        let db = Db::open_in_memory().await.unwrap();
        sqlx::query(
            "INSERT INTO profile (id, name, is_default, created_at, updated_at)
             VALUES ('p1', 'IC', 1, 'now', 'now')",
        )
        .execute(db.writer())
        .await
        .unwrap();
        let second = sqlx::query(
            "INSERT INTO profile (id, name, is_default, created_at, updated_at)
             VALUES ('p2', 'Manager', 1, 'now', 'now')",
        )
        .execute(db.writer())
        .await;
        assert!(second.is_err(), "two default profiles must be impossible");
    }

    #[tokio::test]
    async fn fts_tables_are_queryable() {
        let db = Db::open_in_memory().await.unwrap();
        sqlx::query(
            "INSERT INTO job_fts (rowid, title, company_name, description_text, requirements_text)
             VALUES (1, 'Senior Platform Engineer', 'Acme Robotics',
                     'Own the ingestion platform', 'kubernetes rust')",
        )
        .execute(db.writer())
        .await
        .unwrap();

        let hits: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM job_fts WHERE job_fts MATCH 'kubernetes'")
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(hits, 1, "requirement text must be searchable");

        // Porter stemming: "engineering" should find "Engineer".
        let stemmed: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM job_fts WHERE job_fts MATCH 'engineering'")
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(stemmed, 1);
    }

    #[tokio::test]
    async fn salary_range_must_be_ordered() {
        let db = Db::open_in_memory().await.unwrap();
        sqlx::query(
            "INSERT INTO company (id, name, slug, name_normalized, created_at, updated_at)
             VALUES ('c1', 'Acme', 'acme', 'acme', 'now', 'now')",
        )
        .execute(db.writer())
        .await
        .unwrap();
        let inverted = sqlx::query(
            "INSERT INTO job (id, company_id, slug, title, title_normalized, content_hash,
                              salary_min_cents, salary_max_cents,
                              first_seen_at, last_seen_at, created_at, updated_at)
             VALUES ('j1', 'c1', 's', 't', 't', 'h', 20000000, 10000000,
                     'now', 'now', 'now', 'now')",
        )
        .execute(db.writer())
        .await;
        assert!(inverted.is_err(), "min must not exceed max");
    }
}
