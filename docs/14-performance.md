# Performance engineering

Budgets come from [Requirements §2.1](02-requirements.md). This is how they are met and how
regressions are caught.

## Budgets

| Operation | Budget | Strategy |
|---|---|---|
| `POST /ingest/*` | < 100 ms p95 | two file writes + two inserts, everything else queued |
| Job list, filtered, 10k jobs | < 50 ms p95 | covering indexes, cursor pagination, no `OFFSET` |
| FTS query, first page | < 100 ms p95 | contentless FTS5 + bm25, filters pushed into SQL |
| Job detail | < 30 ms | one query per child collection, no N+1 |
| Match one (job, profile) | < 200 ms | cached embeddings, pure CPU |
| Rescore 5,000 jobs | < 10 s | parallel over rayon-style chunks, single write txn |
| Deterministic extraction | < 500 ms | no LLM call |
| LLM extraction (local GPU) | < 3 s | ask only for missing fields, chunked |
| Idle | ~0% CPU, < 100 MB RSS | notify-driven workers, no busy polling |
| Web TTI on LAN | < 1 s | < 200 KB gz initial bundle |

## SQLite

**Pragmas** (every connection): see [Architecture §3.2](03-architecture.md). The
non-obvious ones: `mmap_size = 256 MiB` avoids a copy on reads, and
`synchronous = NORMAL` is safe under WAL for this workload (a power loss can lose the last
transaction, not corrupt the database) and is roughly an order of magnitude faster than
`FULL` on small writes.

**One writer, many readers.** A `max_connections = 1` writer pool makes lock contention
structurally impossible rather than a retry loop. Readers never block under WAL.

**Indexes that matter** (each one exists because a specific query needs it):

```sql
CREATE INDEX idx_job_status_posted   ON job(status, posted_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX idx_job_company         ON job(company_id)             WHERE deleted_at IS NULL;
CREATE INDEX idx_job_closing         ON job(closes_at)              WHERE status = 'open';
CREATE INDEX idx_job_dedupe          ON job(company_id, title_normalized);
CREATE INDEX idx_job_salary          ON job(salary_max_cents)       WHERE salary_max_cents IS NOT NULL;
CREATE INDEX idx_req_job             ON requirement(job_id, ordinal);
CREATE INDEX idx_req_skill_necessity ON requirement(skill_id, necessity);
CREATE INDEX idx_listing_urlhash     ON job_source_listing(url_hash);
CREATE INDEX idx_listing_recheck     ON job_source_listing(status, last_checked_at);
CREATE INDEX idx_match_profile       ON match_score(profile_id, overall DESC) WHERE is_stale = 0;
CREATE INDEX idx_match_stale         ON match_score(is_stale) WHERE is_stale = 1;
CREATE INDEX idx_task_claim          ON task(status, available_at, priority DESC);
CREATE INDEX idx_embedding_owner     ON embedding(owner_kind, owner_id, model);
CREATE INDEX idx_app_next_action     ON application(next_action_due) WHERE next_action_due IS NOT NULL;
```

Partial indexes (`WHERE deleted_at IS NULL`, `WHERE status = 'open'`) keep the hot indexes
small — most queries only ever touch live rows, so the index should only contain them.

**Query rules.**

- No `SELECT *` in list endpoints; project only what the row renderer needs.
- Batch child fetches with `WHERE parent_id IN (…)` — no per-row queries, ever.
- `EXPLAIN QUERY PLAN` is checked for every new list query; a `SCAN` on `job` fails review.
- `ANALYZE` after bulk imports so the planner has real statistics.
- Prepared-statement cache is on (sqlx default); statement text is static, never formatted
  from user input.

**Write batching.** Bulk operations (rescore, reconcile, board poll) use one transaction per
chunk of ~500 rows. Committing per row is roughly 100× slower; committing everything in one
transaction holds the writer for too long and blocks the UI.

## Async and the runtime

- Multi-threaded tokio; worker concurrency defaults to `cores.clamp(2, 8)`.
- **CPU-bound work goes to `spawn_blocking`**: HTML parsing, zstd, embedding math, Typst
  rendering, PDF text extraction. Blocking the reactor with a 200 ms parse stalls every
  request in flight.
- Workers are notify-driven (`tokio::sync::Notify` from the enqueuer) with a 1 s fallback
  poll, so idle CPU is genuinely zero rather than "small".
- Per-host semaphores on outbound HTTP; one shared `reqwest::Client` (connection pooling,
  HTTP/2, keep-alive) rather than a client per request.
- Graceful shutdown drains in-flight requests, releases task leases immediately (so a restart
  does not wait out a 120 s lease), and flushes file writes.

## HTTP layer

- `tower-http` `CompressionLayer` (brotli/gzip/zstd) on JSON responses over ~1 KB.
- Static assets: content-hashed filenames + `Cache-Control: public, max-age=31536000,
  immutable`; precompressed `.br`/`.gz` served directly, never compressed per request.
- `index.html`: `no-cache`, so a deploy is picked up on the next reload.
- ETag/`If-None-Match` on job detail and taxonomy endpoints.
- HTTP/2 when behind TLS; request body limits per route.
- SSE with a 15 s heartbeat and `flush_interval: -1` at the proxy — buffering breaks live
  progress.

## Extraction cost control

The single biggest lever is not calling the model:

1. Deterministic stages first; the LLM sees only the fields still missing.
2. `llm_call` cache keyed by `blake3(provider|model|schema|prompt|params)` — re-extraction of
   an unchanged capture is free and deterministic.
3. Description truncated to `llm.max_input_tokens`, prioritizing the requirement-bearing
   sections (headings matched against the requirement-section list) rather than blindly
   truncating from the top.
4. Requirement atomization batches all unclassified bullets into one call.
5. Embeddings computed once per content hash and reused across every match.

Practically: a Greenhouse posting with JSON-LD costs zero tokens; a LinkedIn capture costs
one atomization call.

## Frontend

- Route-level splitting; the initial chunk is shell + dashboard. Charts, Markdown editor, and
  PDF viewer are lazy.
- Virtualized job and requirement tables (TanStack Virtual) — 10k rows scroll at 60 fps.
- `staleTime` per resource + SSE invalidation, so navigation is instant and still fresh.
- Next-page prefetch on idle; images (screenshots) lazy with explicit dimensions to avoid
  layout shift.
- System font stack — no render-blocking web font.
- Bundle budget enforced in CI (`size-limit`): initial JS ≤ 200 KB gz. Exceeding it fails
  the build, which is the only way a budget survives contact with a feature.

## Benchmarking & regression detection

- `criterion` benches for the pure crates: salary parsing, requirement atomization, cosine
  similarity, scoring a full job.
- A seeded synthetic corpus generator (`just seed 10000`) creates a realistic 10k-job
  database for query benchmarks — you cannot tune indexes against 12 rows.
- `just bench-api` drives the list/search/detail endpoints with `oha` and asserts p95 against
  the budget table above.
- CI runs the criterion benches on a fixed runner and compares against a committed baseline,
  flagging >20% regressions. Machine noise makes anything tighter meaningless.

## Known scaling limits

| Limit | Where it bites | Mitigation when it does |
|---|---|---|
| Single writer | sustained bulk import | batch in one transaction; imports are rare |
| Brute-force vector search | > ~200k vectors | `sqlite-vec` extension (drop-in) |
| Contentless FTS5 rebuild | full reindex of 100k jobs | incremental via triggers; full rebuild is a maintenance task |
| In-process queue | multi-machine workers | out of scope (CON-02/CON-03) |
| Embedded UI in the binary | binary size ~15–25 MB | acceptable; `embed-web` is a feature flag |

Every one of these is a scale this project does not target. Recording them here is how we
avoid pre-optimizing for problems a single-user job search will never have.
