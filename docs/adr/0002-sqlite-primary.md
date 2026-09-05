# ADR-0002 — SQLite as the primary datastore

**Status:** accepted · **Date:** 2026-09-05

## Context

The user named MongoDB, DuckDB, and SQLite as acceptable "machine-parsable database"
options. The deployment target is a personal home server and CON-02 forbids requiring any
external service. Expected scale is 10³–10⁵ jobs, with a highly relational shape:
jobs ↔ requirements ↔ skills ↔ accomplishments ↔ matches ↔ applications.

## Decision

SQLite (WAL) is the single primary datastore, accessed via `sqlx`, with FTS5 for full-text
search and a `BLOB` column for embeddings. DuckDB is an optional **read-only** analytics
attachment later; MongoDB is not used.

## Rationale

- **The data is relational, not documentary.** Nearly every interesting query is a join:
  "requirements across open jobs where my evidence is a gap, grouped by skill, weighted by
  my interest." In a document store those joins move into application code, which is both
  slower and more bug-prone. The genuinely open-ended parts (raw provider payloads,
  explanation blobs) live in JSON columns, and SQLite's JSON1 functions query them fine.
- **Zero operational surface.** One file. No daemon, no port, no auth config, no version
  skew, no backup agent. `VACUUM INTO` gives a consistent hot snapshot. This is the single
  largest quality-of-life factor for software that runs on a box you do not want to babysit.
- **Fast enough by a wide margin.** At 10⁵ rows with correct indexes, filtered list queries
  are sub-millisecond and FTS5 with bm25 returns a page in single-digit milliseconds. The
  performance budgets in [14-performance.md](../14-performance.md) are met with room to
  spare.
- **FTS5 is built in** and supports contentless external-content tables, porter stemming, and
  bm25 field weighting — no Elasticsearch, no second index to keep consistent.
- **Portability matches the file-based design.** A single file plus a directory tree copies
  to another machine and works, which is the property [ADR-0003](0003-files-and-db.md)
  depends on.

## Alternatives considered

**MongoDB.** Attractive for the semi-structured nature of scraped postings, and a natural fit
for "dump the raw payload." Rejected because (a) it requires running a server, violating
CON-02; (b) the query workload is join-heavy and `$lookup` is a poor substitute; (c) it
provides no full-text solution as good as FTS5 without Atlas Search; (d) schema-on-read would
let extraction quality problems hide, whereas `CHECK` constraints and `NOT NULL` surface them
at write time — which is exactly what we want for a system whose main risk is bad data.

**DuckDB.** Excellent at the analytics in F8/H7, and embedded like SQLite. Rejected as
primary because it is a column store optimized for scans, with weaker support for the
high-frequency small row updates this workload is built on (task queue claims, staleness
flags, incremental upserts), and a single-writer model with less mature concurrent-read
behavior for a live server. Kept as an optional attachment: point DuckDB at the SQLite file
or exported Parquet for ad-hoc analysis, with no second write path to keep consistent.

**Postgres.** The obvious "grown-up" choice, with `pg_trgm`, `pgvector`, and real
full-text search. Rejected on CON-02: requiring a database server for a single-user personal
tool is a meaningful ongoing cost (upgrades, backups, connection config) for benefits that do
not bind at this scale.

**Hybrid SQLite + a vector database.** Rejected: brute-force cosine over ≤10⁵ f32 vectors
takes milliseconds in Rust. `sqlite-vec` is a drop-in if the corpus ever grows past ~200k
vectors.

## Consequences

- **One writer.** Enforced structurally with a `max_connections = 1` writer pool, so
  `SQLITE_BUSY` cannot occur rather than being retried. Bulk writes are batched per ~500
  rows.
- **TEXT primary keys** (UUIDv7) mean FTS5 needs an integer surrogate (`job.fts_rowid`).
  Accepted for the merge-safety and time-ordering that UUIDv7 gives.
- **No server-side vector index** until `sqlite-vec`; documented in the scaling-limits table.
- **Migrations are forward-only** and must be hand-written; SQLite's limited `ALTER TABLE`
  occasionally requires the table-rebuild dance. Acceptable at this schema's rate of change.
- **Not on a network filesystem.** NFS breaks SQLite locking; documented as unsupported.
