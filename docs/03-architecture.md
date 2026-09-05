# Architecture

## 1. System context

```
┌──────────────────────────── your machine (browser) ────────────────────────────┐
│                                                                                │
│   LinkedIn / Indeed / Glassdoor / ATS pages  (you are logged in)               │
│                     │                                                          │
│                     │  DOM + URL + screenshot, on your click                   │
│           ┌─────────▼──────────┐          ┌──────────────────────┐             │
│           │ Jobseeker MV3 ext  │          │ Web UI (React SPA)   │             │
│           └─────────┬──────────┘          └──────────┬───────────┘             │
└─────────────────────┼────────────────────────────────┼─────────────────────────┘
                      │ POST /api/v1/ingest/capture    │ /api/v1/*  + SSE
                      │ (device token)                 │ (session cookie)
┌─────────────────────▼────────────────────────────────▼─────────────────────────┐
│                       home server: one `jobseeker` process                     │
│                                                                                │
│   ┌──────────────────────┐        ┌───────────────────────────────────────┐    │
│   │ axum HTTP layer      │        │ worker pool (tokio tasks)             │    │
│   │  routes, auth, SSE,  │◄──────►│  ingest → extract → normalize →       │    │
│   │  embedded SPA assets │ queue  │  materialize → embed → score          │    │
│   └──────────┬───────────┘        └───────┬───────────────────┬───────────┘    │
│              │                            │                   │                │
│        ┌─────▼────────────────────────────▼─────┐   ┌─────────▼──────────┐     │
│        │ SQLite (WAL)  jobseeker.db             │   │ data dir (files)   │     │
│        │  relational index + FTS5 + vectors     │   │ jobs/, captures/,  │     │
│        └────────────────────────────────────────┘   │ documents/, media/ │     │
│                                                     └────────────────────┘     │
└───────────────┬──────────────────────────────┬─────────────────────────────────┘
                │ HTTP (opt-in)                │ HTTP (opt-in, egress)
        ┌───────▼─────────┐          ┌─────────▼──────────────────┐
        │ target job URLs │          │ LLM/embeddings provider    │
        │ + public ATS API│          │ Ollama (local) | OpenAI |  │
        └─────────────────┘          │ Anthropic | none           │
                                     └────────────────────────────┘
```

Everything inside the server box is one OS process. There is no broker, no cache server, no
separate worker deployment. See [ADR-0001](adr/0001-rust-axum.md),
[ADR-0002](adr/0002-sqlite-primary.md), [ADR-0006](adr/0006-sqlite-queue.md).

## 2. Crate topology

A Cargo workspace. Dependencies point strictly downward; there are no cycles. The rule that
keeps this honest: **`core` knows nothing about SQL or HTTP; `db` knows nothing about HTTP;
domain crates know nothing about each other's internals, only `core` types.**

```
                         jobseeker-cli  (bin: jobseeker)
                                 │
                         jobseeker-api        (axum router, auth, SSE, OpenAPI, SPA)
                                 │
                         jobseeker-pipeline   (task queue worker + stage orchestration)
        ┌──────────┬──────────┬──┴───────┬──────────┬───────────┬──────────┐
   acquire     extract    normalize   matching    resume      store      llm
        └──────────┴──────────┴──────────┴─────┬────┴───────────┴──────────┘
                                          jobseeker-db      (sqlx, migrations, repos)
                                               │
                                          jobseeker-core    (domain types, ids, errors, config)
```

| Crate | Responsibility | Must not |
|---|---|---|
| `jobseeker-core` | Domain structs/enums (`Job`, `Requirement`, `Salary`, `WorkMode`, `Provenance`, `Confidence`), typed ids, error taxonomy, config types and loading, time/precision helpers. | Touch sqlx, axum, reqwest. |
| `jobseeker-db` | Connection pools (1 writer, N readers), migrations, repositories returning `core` types, FTS queries, vector storage, the task-queue table operations. | Contain business rules or HTTP types. |
| `jobseeker-llm` | `Completer`/`Embedder` traits, providers (ollama, openai, anthropic, mock), JSON-schema-constrained calls, response cache, token/cost accounting. | Know about jobs or resumes specifically. |
| `jobseeker-acquire` | Getting bytes: robots-aware HTTP client, SSRF guard, per-host rate limiter, ATS API clients, capture intake validation, raw persistence. | Parse job semantics. |
| `jobseeker-extract` | Bytes/DOM → `ExtractedJob`: JSON-LD, microdata, ATS JSON, site adapters (trait + registry), boilerplate stripping, HTML→Markdown, LLM extraction, provenance merge. | Write to the database. |
| `jobseeker-normalize` | Field-level canonicalization: salary, location, dates, seniority, employment type, company name; requirement atomization; skill taxonomy resolution + aliases. | Do I/O. Pure functions, heavily unit-tested. |
| `jobseeker-store` | File materialization (`job.md`/`job.json`/`requirements.json`/`raw/`), deterministic serialization, atomic writes, `reconcile` both directions, content-addressed media. | Own the schema. |
| `jobseeker-matching` | Requirement↔evidence verdicts, subscores, weighted overall, blockers, gap aggregation, embedding-based similarity. | Call an LLM directly for scoring (embeddings only, via `llm`). |
| `jobseeker-resume` | Accomplishment selection (coverage optimization under a length budget), phrasing via `llm`, Typst/Markdown rendering, versioning. | Invent facts; only selects bank content. |
| `jobseeker-pipeline` | Task definitions, queue worker loop with leases/backoff, stage orchestration, progress events, idempotency. | Contain parsing or scoring logic. |
| `jobseeker-api` | Routes, DTOs (separate from domain types), auth, CORS for the extension, SSE, OpenAPI generation, embedded SPA serving. | Contain business rules. |
| `jobseeker-cli` | Argument parsing, config resolution, subcommands, human-readable output. | Duplicate logic — it calls the same crates the API does. |

Why this many crates: compile-time parallelism, enforced layering, and — the real payoff —
`normalize` and `matching` are pure and can be tested exhaustively without a database or a
network.

## 3. Runtime structure

### 3.1 Process layout

```
main (jobseeker-cli)
 ├─ load + validate config
 ├─ init tracing (+ optional OTLP)
 ├─ open DB, run migrations, assert schema version
 ├─ build AppState { pools, config, llm registry, adapter registry, event bus }
 ├─ spawn worker pool     (N = config.worker.concurrency, default = cores.clamp(2,8))
 ├─ spawn scheduler       (cron-ish: refresh stale jobs, reclaim leases, prune caches)
 └─ serve axum on the configured bind address, graceful shutdown on SIGINT/SIGTERM
```

`AppState` is cheap to clone (`Arc` internals) and is the single dependency-injection point.
Handlers receive `State<AppState>`; workers receive the same struct. There is no global
mutable state and no lazily-initialized singletons.

### 3.2 SQLite access model

Two pools, because SQLite has exactly one writer:

- **Writer pool**: `max_connections = 1`. All mutations go through it. Serialization is
  explicit rather than accidental, so `SQLITE_BUSY` becomes structurally impossible rather
  than a retry loop.
- **Reader pool**: `max_connections = cores.clamp(4, 16)`, WAL readers do not block on the
  writer.

Boot pragmas (applied to every connection):

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;      -- safe with WAL for this workload
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store = MEMORY;
PRAGMA cache_size = -65536;       -- 64 MiB per connection
PRAGMA mmap_size = 268435456;     -- 256 MiB
PRAGMA wal_autocheckpoint = 1000;
```

Long transactions are forbidden in request handlers. Multi-step pipeline work commits per
stage so a crash resumes at stage granularity.

### 3.3 Task queue

Table-backed (`task`), leased, with `dedupe_key` for coalescing. Claim is a single atomic
statement:

```sql
UPDATE task SET status='running', lease_expires_at=?, started_at=?, attempts=attempts+1
WHERE id = (
  SELECT id FROM task
  WHERE status='queued' AND available_at <= ?
  ORDER BY priority DESC, available_at ASC, created_at ASC
  LIMIT 1
)
RETURNING *;
```

- Leases (default 120 s, renewed by long handlers) make crash recovery automatic: the
  scheduler returns expired-lease tasks to `queued`.
- Retries use exponential backoff with jitter, `max_attempts` default 5, then terminal
  `failed` with the error retained.
- Handlers must be **idempotent** — at-least-once delivery is guaranteed, exactly-once is
  not. Idempotency is achieved by keying writes on content hashes and natural keys.
- Workers wake on a notify channel from the enqueuer and otherwise poll at a low frequency
  (default 1 s), so idle CPU is ~0 (NFR-P-05).

Task kinds: `ingest_url`, `ingest_capture`, `extract_job`, `refresh_listing`,
`dedupe_job`, `materialize_job`, `embed`, `score_match`, `aggregate_gaps`,
`render_document`, `import_resume`, `reconcile_files`, `poll_board`.

### 3.4 Event bus / progress

An in-process `tokio::sync::broadcast` of `DomainEvent` (task state changes, job created,
extraction finished, score updated). The SSE endpoint subscribes and filters per client.
Events are also written to `event_log` for the UI to backfill after reconnect, so a dropped
connection does not lose progress. Nothing depends on the bus for correctness — it is
strictly a notification path.

## 4. The ingestion pipeline

```
                       ┌────────────────────────────────────────────────┐
  POST /ingest/url     │  1. persist raw            (durable, immutable)│
  POST /ingest/capture ├─►  capture row + raw/ file, content hash        │
  POST /ingest/paste   │                                                │
  poll_board           │  2. acquire (if needed)                        │
                       │     robots + SSRF + rate limit + ATS API pref  │
                       │                                                │
                       │  3. extract  (ordered, fill-only-if-empty)     │
                       │     a. ATS JSON / JSON-LD / microdata          │
                       │     b. site adapter (registry by host)         │
                       │     c. generic heuristics + readability        │
                       │     d. LLM structured extraction (optional)    │
                       │       → ExtractedJob { field: (value, prov, c) }│
                       │                                                │
                       │  4. normalize                                  │
                       │     salary/location/date/seniority/company     │
                       │     description → Markdown                     │
                       │     description → Vec<Requirement> (atomize)   │
                       │     requirement.skill ← taxonomy + aliases     │
                       │                                                │
                       │  5. persist + dedupe                           │
                       │     upsert company, job, listing, requirements │
                       │     merge cross-posts into canonical job       │
                       │     preserve `manual` provenance fields        │
                       │                                                │
                       │  6. materialize files (atomic)                 │
                       │                                                │
                       │  7. enqueue embed → score_match(all profiles)  │
                       └────────────────────────────────────────────────┘
```

Each numbered step is a separate task or a separate committed unit inside one task, so
failures are resumable and observable. Details in [Ingestion](05-ingestion.md) and
[Extraction](06-extraction.md).

### Provenance merge

Extraction produces, per field, a `Sourced<T> { value, provenance, confidence }`. The merge
rule is a total order over provenance:

```
manual > api > jsonld > microdata > adapter > llm > rules > inferred
```

with confidence as a tiebreaker. A lower-precedence stage never overwrites a
higher-precedence value; instead it records an `extraction_conflict` row when it disagrees
materially, which surfaces in the UI as "LinkedIn said $150k, Greenhouse said $165k."

## 5. Extension boundaries (how to extend without touching the core)

| Extension point | Mechanism | Where |
|---|---|---|
| New job site | implement `SiteAdapter { fn matches(&Url) -> bool; fn extract(&Capture) -> Result<ExtractedJob> }`, register in `AdapterRegistry` | `jobseeker-extract/src/adapters/` |
| New ATS API | implement `AtsClient` | `jobseeker-acquire/src/ats/` |
| New LLM/embedding provider | implement `Completer` / `Embedder` | `jobseeker-llm/src/providers/` |
| New task kind | add variant + `TaskHandler` impl, register in the dispatcher | `jobseeker-pipeline/src/tasks/` |
| New match component | implement `ScoreComponent { fn id(); fn score(&Ctx) -> Subscore }`, add weight to config | `jobseeker-matching/src/components/` |
| New resume template | add a Typst file + template manifest | `templates/resume/` |
| New export format | implement `Exporter` | `jobseeker-store/src/export/` |

Registries are built once in `AppState`; adapters are stateless and cheap.

## 6. API design

- Base path `/api/v1`. Additive changes only within a major version; breaking changes get
  `/api/v2` and a deprecation window.
- **DTOs are distinct types from domain types.** The wire format is a contract; the domain
  model must be free to change. Conversion is explicit `From` impls in `jobseeker-api::dto`.
- OpenAPI 3.1 is generated from the handlers (`utoipa`) and served at `/openapi.json`. The
  web client's TypeScript types are generated from that document by
  `just gen-client` — the frontend never hand-writes a response type.
- Cursor pagination everywhere (`{items, next_cursor}`); cursors are opaque base64 of
  `(sort_key, id)`. No `OFFSET` on large tables.
- Error envelope: `{"error":{"code":"job_not_found","message":"...","details":{...},"trace_id":"..."}}`.
  `code` is a stable machine string; `message` is for humans.
- Writes that trigger background work return `202 Accepted` with `{task_id}` and a
  `Location` header pointing at the task.

Full surface in [API](09-api.md).

## 7. Frontend architecture

- React 19 + TypeScript + Vite, Tailwind for styling, TanStack Query for server state,
  TanStack Router (file-based, type-safe) for routing, and generated API types.
- **Server state lives in TanStack Query only.** No Redux/global store for data; client
  state (filters, selection, palette) uses URL search params first, `useState` second.
  Filters in the URL means views are shareable and the back button works.
- Route-level code splitting; the initial bundle contains only the shell + dashboard.
- SSE subscription invalidates precise query keys on domain events — the UI updates as the
  pipeline progresses without polling.
- Optimistic updates for cheap mutations (rating, tags, status drag) with rollback.
- Virtualized lists for the jobs table so 10k rows are fine.
- Built assets are embedded into the binary (`rust-embed`, feature `embed-web`), served
  with precompressed brotli/gzip, long-lived immutable caching on hashed filenames, and
  `index.html` as SPA fallback. Dev mode proxies `/api` to the Rust server instead.

See [Frontend](10-frontend.md).

## 8. Configuration

Layered, later wins: defaults → `/etc/jobseeker/config.toml` → `$XDG_CONFIG_HOME/jobseeker/config.toml`
→ `--config <path>` → `JOBSEEKER_*` env vars → CLI flags. Validated into a strongly typed
`Config` at boot; unknown keys are an error (typos should fail loudly).

```toml
[server]
bind = "127.0.0.1:8787"
public_url = "http://jobs.home.arpa:8787"

[data]
dir = "/srv/jobseeker/data"          # files
db_path = "/srv/jobseeker/data/jobseeker.db"

[auth]
mode = "password"                    # none | token | password
session_ttl_days = 30

[worker]
concurrency = 4
lease_seconds = 120
max_attempts = 5

[acquire]
user_agent = "jobseeker/0.1 (+self-hosted personal use)"
respect_robots = true
per_host_delay_ms = 2000
timeout_seconds = 20
allow_private_networks = false

[llm]
provider = "ollama"                  # ollama | openai | anthropic | none
model = "qwen2.5:14b-instruct"
embedding_provider = "ollama"
embedding_model = "nomic-embed-text"
max_input_tokens = 16000
cache = true

[matching]
weights = { required_coverage = 0.45, preferred_coverage = 0.15, semantic = 0.15, seniority = 0.10, comp = 0.10, location = 0.05 }

[resume]
renderer = "typst"                   # typst | none
default_template = "ats"
```

## 9. Cross-cutting concerns

**Error handling.** `thiserror` per crate with domain-meaningful variants; `anyhow` only in
binaries. `jobseeker-api` maps `core::Error` → HTTP status + stable code in exactly one
place. Never `unwrap()` outside tests and startup assertions.

**Time.** All timestamps stored as UTC RFC3339 strings (sortable, human-readable in the
files, comparable in SQL) with a companion `*_precision` column where the source was vague.
Never store local times.

**Ids.** UUIDv7 rendered as lowercase hex-with-dashes: time-ordered (good index locality),
globally unique (safe to merge two installs), and opaque. Short display form is the last 8
characters. Typed newtypes (`JobId`, `RequirementId`) prevent argument transposition.

**Idempotency.** Content hashes (BLAKE3) of raw captures and of LLM prompts; natural unique
keys on `(source, source_job_id)` and `url_hash`; `INSERT ... ON CONFLICT DO UPDATE`.

**Observability.** `tracing` spans from request → task → LLM call carrying a `trace_id`
returned in error payloads and response headers. Prometheus metrics: ingest counter by
source/outcome, task duration histogram by kind, queue depth gauge, LLM tokens/cost
counters, extraction-confidence histogram, HTTP latency histogram.

**Testing.** Pure unit tests for `normalize`/`matching`; fixture-driven golden tests for
`extract` (checked-in HTML → expected JSON); integration tests booting the real axum app
against a temp-file SQLite with a mock LLM; an eval harness scoring extraction quality on
the fixture corpus. See [Testing](15-testing.md).

## 10. Deliberate non-choices

| Rejected | Why |
|---|---|
| Postgres | Violates CON-02. SQLite handles this scale (10⁴–10⁵ rows) with better ops, and `VACUUM INTO` backups are trivial. |
| MongoDB as primary | The data is highly relational (jobs↔requirements↔skills↔accomplishments↔matches). Document storage would push joins into application code. JSON columns cover the genuinely schemaless parts. |
| DuckDB as primary | Column-store, single-writer, optimized for analytics; wrong for transactional row updates. Kept as an *optional read-only analytics attachment* (D6). |
| Redis / RabbitMQ / Celery-alike | One more daemon to run for a queue that never exceeds a few thousand rows. |
| Server-side rendering / HTMX | The UI is an interactive, stateful, keyboard-driven workbench with heavy local state; an SPA behind a JSON API is the better fit and keeps the API honest by making it the only interface. |
| Microservices | Single-user home server. Process boundaries would add failure modes and buy nothing. |
| Storing site credentials / headless login by default | Violates CON-05 and puts high-value secrets at risk. Extension capture achieves the same result. |
| ORM (Diesel/SeaORM) | `sqlx` with checked queries keeps SQL visible, which matters when tuning SQLite indexes and FTS. |
| Cloud LLM by default | Job search and resume data are sensitive (NFR-S-07). Local-first, cloud opt-in. |
