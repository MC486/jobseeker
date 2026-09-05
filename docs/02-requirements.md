# Requirements

Requirement IDs are stable and referenced from tests, ADRs, and issues.
`FR-*` = functional, `NFR-*` = non-functional, `CON-*` = constraint.

Priority: **MUST** (M0/M1 blocking), **SHOULD** (planned), **MAY** (opportunistic).

---

## 1. Functional requirements

### 1.1 Acquisition

| ID | Priority | Requirement |
|---|---|---|
| FR-A-01 | MUST | Given an HTTP(S) URL, the system SHALL create a `job_source_listing` and enqueue an ingestion task, returning a task id within 100 ms without waiting for fetch or extraction. |
| FR-A-02 | MUST | The system SHALL accept a client-supplied capture (URL + HTML + optional visible text, screenshot, and source hint) at an authenticated endpoint, and treat it identically to a server-side fetch thereafter. |
| FR-A-03 | MUST | The system SHALL persist every acquisition verbatim and immutably (raw bytes on disk + `capture` row with content hash) before any parsing occurs. Parsing failures SHALL never lose the raw input. |
| FR-A-04 | MUST | The system SHALL accept pasted text/HTML with no URL and produce a job record. |
| FR-A-05 | MUST | Server-side HTTP fetches SHALL honor `robots.txt` for the target host, apply a per-host rate limit (default ≥ 2 s between requests), send a descriptive User-Agent, and time out (default 20 s). |
| FR-A-06 | MUST NOT | The system SHALL NOT store or transmit third-party site credentials, and SHALL NOT perform background/bulk crawling of sites requiring authentication. |
| FR-A-07 | SHOULD | For known ATS hosts (Greenhouse, Lever, Ashby, SmartRecruiters, Workable, Recruitee), the system SHALL prefer the site's public JSON endpoint over HTML scraping. |
| FR-A-08 | MUST | Re-capturing a previously seen URL SHALL create a new `capture` revision, and SHALL update the canonical job only where fields changed, recording a change event. |
| FR-A-09 | MUST | The system SHALL detect closure signals (HTTP 404/410, ATS `closed` state, known "no longer accepting applications" markers) and set listing status to `closed` with a timestamp, without deleting data. |
| FR-A-10 | MUST | Multiple listings determined to be the same posting SHALL resolve to a single canonical `job`, with all source URLs retained and one marked canonical. |
| FR-A-11 | SHOULD | Deduplication SHALL be reversible: a user can split an incorrectly merged job. |

### 1.2 Extraction

| ID | Priority | Requirement |
|---|---|---|
| FR-E-01 | MUST | Extraction SHALL run as an ordered pipeline: structured (JSON-LD/microdata/ATS JSON) → site adapter → generic heuristics → LLM, where each stage only fills fields still unset. |
| FR-E-02 | MUST | Every extracted field SHALL record its provenance (`jsonld`, `microdata`, `api`, `adapter`, `rules`, `llm`, `manual`) and a confidence in `[0,1]`. |
| FR-E-03 | MUST | A field with provenance `manual` SHALL NOT be overwritten by any automated stage. |
| FR-E-04 | MUST | The system SHALL extract at minimum: title, company name, description, work mode, employment type, ≥1 location (or explicit "remote"), apply URL. Missing values SHALL be `null`, never guessed placeholders. |
| FR-E-05 | MUST | Salary text SHALL normalize to `{min, max, currency, period, is_estimate}` in integer minor units; ambiguous input SHALL yield `null` plus retained raw text. |
| FR-E-06 | MUST | Relative dates ("3 days ago") SHALL resolve against the capture timestamp and record a `precision` of day/hour/exact. |
| FR-E-07 | MUST | The description SHALL be stored as normalized Markdown with navigation/footer/cookie/legal-boilerplate removed, preserving heading and list structure. |
| FR-E-08 | MUST | The system SHALL atomize the description into `requirement` records, each with kind, necessity, verbatim text, and — where present — min/max years, level, education level, and linked taxonomy skill. |
| FR-E-09 | MUST | LLM extraction SHALL be constrained to a JSON schema, validated on receipt, and rejected (not partially applied) on validation failure, with the failure recorded. |
| FR-E-10 | MUST | LLM responses SHALL be cached by (provider, model, prompt hash) so re-running extraction costs nothing and is deterministic. |
| FR-E-11 | MUST | With no LLM provider configured, ingestion SHALL still succeed using deterministic stages only, and the job SHALL be marked `extraction_partial`. |
| FR-E-12 | SHOULD | An eval harness SHALL score extraction against a labeled fixture corpus per field and per adapter, and CI SHALL fail on regression beyond a threshold. |
| FR-E-13 | MUST | Extraction SHALL be idempotent: re-running on an unchanged capture produces an identical result (given cache hits). |
| FR-E-14 | SHOULD | Company names SHALL be resolved to an existing `company` via normalized-name matching before creating a new one. |

### 1.3 Storage

| ID | Priority | Requirement |
|---|---|---|
| FR-S-01 | MUST | Each canonical job SHALL be materialized to a directory containing `job.md`, `job.json`, `requirements.json`, and `raw/` captures. |
| FR-S-02 | MUST | File paths SHALL be stable, human-readable, and derived from company slug + posting slug + short id. |
| FR-S-03 | MUST | Serialization SHALL be deterministic (stable key order, LF endings, no volatile timestamps in content) so unchanged data produces no file churn. |
| FR-S-04 | MUST | `jobseeker reconcile --from-files` SHALL rebuild a complete database from the file tree alone. |
| FR-S-05 | MUST | File writes SHALL be atomic (write temp + rename) and SHALL NOT leave partial files on crash. |
| FR-S-06 | MUST | The database SHALL run in WAL mode with `foreign_keys=ON` and `busy_timeout` set; schema changes SHALL be applied via versioned, forward-only migrations. |
| FR-S-07 | MUST | Deleting a job SHALL delete its dependent rows and, on request, its files, and SHALL be recorded so a subsequent reconcile does not resurrect it. |
| FR-S-08 | SHOULD | `jobseeker backup` SHALL produce a consistent snapshot (`VACUUM INTO`) plus a files archive. |

### 1.4 Profile & experience bank

| ID | Priority | Requirement |
|---|---|---|
| FR-P-01 | MUST | The system SHALL store experience items (role/project/education/certification/award/publication/oss/volunteer) with org, title, dates, location, and description. |
| FR-P-02 | MUST | The system SHALL store accomplishments as child records of experience items, with free text, optional quantified impact (metric, value, unit), linked skills, and a user strength rating. |
| FR-P-03 | MUST | The system SHALL support ≥1 profile; all matching and generation SHALL be parameterized by profile id. |
| FR-P-04 | SHOULD | The system SHALL import an existing resume (PDF/DOCX/MD) into draft experience items and accomplishments for user review before commit. |
| FR-P-05 | SHOULD | Accomplishments SHALL support named phrasing variants used by generation. |
| FR-P-06 | MUST | Profile skills SHALL record years, level, and last-used year. |

### 1.5 Matching

| ID | Priority | Requirement |
|---|---|---|
| FR-M-01 | MUST | For a (job, profile) pair the system SHALL produce a per-requirement verdict of `met`/`partial`/`gap`/`unknown` with pointers to the evidence used. |
| FR-M-02 | MUST | The overall score SHALL be a documented weighted combination of named subscores, each individually reported. No unexplained single number. |
| FR-M-03 | MUST | Scores SHALL record `algorithm_version` and an `inputs_hash`; changing the job, the profile, or the algorithm SHALL mark existing scores stale. |
| FR-M-04 | MUST | Component weights SHALL be user-configurable, and changing them SHALL recompute without re-invoking any LLM. |
| FR-M-05 | MUST | Hard blockers (missing required clearance, degree, work authorization, on-site in an unacceptable location) SHALL be surfaced separately from graded gaps. |
| FR-M-06 | SHOULD | Semantic similarity SHALL use embeddings stored locally; absence of an embedding provider SHALL degrade the score to lexical/taxonomy matching only, clearly flagged. |
| FR-M-07 | MUST | The system SHALL aggregate gaps across all non-archived jobs into a ranked list with frequency counts. |
| FR-M-08 | SHOULD | Matching SHALL complete in < 200 ms per (job, profile) pair once embeddings exist. |

### 1.6 Generation

| ID | Priority | Requirement |
|---|---|---|
| FR-G-01 | MUST | Generated resumes SHALL only contain claims traceable to experience-bank records; every generated bullet SHALL store its source accomplishment id. |
| FR-G-02 | MUST | Generation SHALL respect a length budget (pages or bullet count) and SHALL report which required requirements remain uncovered. |
| FR-G-03 | MUST | Every generation SHALL create an immutable `document` row (with model, template, prompt hash, parent version). |
| FR-G-04 | SHOULD | The system SHALL render Markdown/Typst source to PDF, and SHALL keep working with source-only output if no renderer is installed. |
| FR-G-05 | MUST | Cover letters SHALL be grounded in the job record and selected accomplishments, and SHALL be clearly marked as drafts requiring review. |
| FR-G-06 | SHOULD | An application SHALL be linkable to the exact document version submitted. |

### 1.7 Tracking

| ID | Priority | Requirement |
|---|---|---|
| FR-T-01 | MUST | The system SHALL track applications through a defined status pipeline with an append-only event timeline. |
| FR-T-02 | MUST | Each application SHALL support a next action with a due date, and the system SHALL surface overdue and upcoming actions. |
| FR-T-03 | SHOULD | The system SHALL flag applications with no activity for N days (default 14) as possibly ghosted. |
| FR-T-04 | MUST | The system SHALL surface open jobs with a close date within N days that have no application. |

### 1.8 API, CLI, UI

| ID | Priority | Requirement |
|---|---|---|
| FR-I-01 | MUST | All functionality SHALL be exposed by a versioned HTTP JSON API (`/api/v1`) that is the sole interface used by the web UI. |
| FR-I-02 | MUST | The API SHALL publish an OpenAPI 3.1 document at `/openapi.json` generated from the implementation, and the web client's types SHALL be generated from it. |
| FR-I-03 | MUST | List endpoints SHALL support cursor pagination, sorting, and filtering, and SHALL never return unbounded result sets (default 50, max 200). |
| FR-I-04 | MUST | Errors SHALL use a single envelope: `{error: {code, message, details?, trace_id}}` with appropriate HTTP status. |
| FR-I-05 | MUST | Long-running work SHALL be represented as tasks with progress observable via `GET /api/v1/tasks/:id` and streamed via SSE at `/api/v1/events`. |
| FR-I-06 | MUST | A CLI SHALL cover: `serve`, `migrate`, `add`, `list`, `show`, `refresh`, `match`, `gaps`, `resume`, `export`, `reconcile`, `backup`, `pair`. |
| FR-I-07 | MUST | The web UI SHALL be servable from the same binary and origin as the API (no CORS requirement for normal use). |
| FR-I-08 | MUST | The extension origin SHALL be permitted via explicit CORS allowlist and token auth. |

---

## 2. Non-functional requirements

### 2.1 Performance

| ID | Priority | Requirement |
|---|---|---|
| NFR-P-01 | MUST | `POST /api/v1/ingest/*` SHALL respond in < 100 ms p95 (excluding request body transfer), deferring all fetching/parsing to the queue. |
| NFR-P-02 | MUST | Job list queries with filters over 10,000 jobs SHALL return in < 50 ms p95 on modest home-server hardware (4 cores, SATA SSD). |
| NFR-P-03 | MUST | Full-text search over 10,000 jobs SHALL return the first page in < 100 ms p95. |
| NFR-P-04 | SHOULD | Full extraction of one posting SHALL complete in < 3 s with a local LLM on GPU, and < 500 ms when deterministic stages suffice. |
| NFR-P-05 | MUST | Idle resource usage SHALL be < 100 MB RSS and effectively 0% CPU. |
| NFR-P-06 | SHOULD | Web UI initial JS payload SHALL be < 200 KB gzipped; time-to-interactive < 1 s on LAN. |
| NFR-P-07 | MUST | The system SHALL remain responsive during background enrichment (writer contention must not block reads: WAL + single writer). |

### 2.2 Reliability & correctness

| ID | Priority | Requirement |
|---|---|---|
| NFR-R-01 | MUST | Tasks SHALL be at-least-once with idempotent handlers, bounded retries with exponential backoff, and a terminal `failed` state that retains the error. |
| NFR-R-02 | MUST | A crash mid-pipeline SHALL leave the database consistent; leased tasks SHALL be reclaimable after lease expiry. |
| NFR-R-03 | MUST | No data loss on ungraceful shutdown of anything already acknowledged (raw capture is durable before ack of the *task*, and the API only acks receipt). |
| NFR-R-04 | MUST | Migrations SHALL be tested forward from an empty database and from the previous release's schema in CI. |
| NFR-R-05 | SHOULD | Unit + integration test coverage of extraction/normalization/matching logic SHALL be maintained; every parsing bug fixed SHALL add a fixture. |

### 2.3 Security & privacy

| ID | Priority | Requirement |
|---|---|---|
| NFR-S-01 | MUST | All state-changing endpoints SHALL require authentication unless `auth.mode = none` is explicitly configured for a trusted network. |
| NFR-S-02 | MUST | Passwords SHALL be stored with Argon2id; session tokens SHALL be random ≥ 256 bits, stored hashed, with expiry and revocation. |
| NFR-S-03 | MUST | Extension tokens SHALL be per-device, revocable, and scoped to ingest endpoints only. |
| NFR-S-04 | MUST | Secrets (LLM API keys, tokens) SHALL be loadable from environment or a file mode 0600, and SHALL NOT be logged, echoed by the API, or written to the data files. |
| NFR-S-05 | MUST | Server-side fetches SHALL block private/link-local/loopback address ranges by default (SSRF protection) for user-supplied URLs. |
| NFR-S-06 | MUST | The system SHALL default to binding `127.0.0.1` and require explicit configuration to listen on other interfaces. |
| NFR-S-07 | MUST | When a remote LLM provider is used, the UI/docs SHALL make clear that job and resume content leaves the machine; local-only providers SHALL be the default. |
| NFR-S-08 | MUST | Rendering of captured HTML in the UI SHALL be sanitized; raw captures SHALL be served with a restrictive CSP and `Content-Disposition` where appropriate. |
| NFR-S-09 | MUST | Uploads SHALL be size-limited (default 8 MB HTML, 12 MB screenshot) and content-type checked. |

### 2.4 Operability

| ID | Priority | Requirement |
|---|---|---|
| NFR-O-01 | MUST | The system SHALL ship as a single static-ish binary with the UI embedded, plus a versioned example config. |
| NFR-O-02 | MUST | Configuration SHALL be validated at startup with actionable error messages naming the offending key. |
| NFR-O-03 | MUST | Logs SHALL be structured, level-controlled, and include a trace id correlating API request → task → LLM call. |
| NFR-O-04 | MUST | `/healthz` (liveness) and `/readyz` (DB + migrations + queue) SHALL exist; `/metrics` SHALL expose Prometheus counters/histograms. |
| NFR-O-05 | SHOULD | A systemd unit and a container image SHALL be provided for home-server deployment. |
| NFR-O-06 | MUST | Upgrades SHALL run migrations automatically on start (configurable) and refuse to start on a newer-than-known schema. |

### 2.5 Maintainability

| ID | Priority | Requirement |
|---|---|---|
| NFR-M-01 | MUST | Domain logic SHALL be independent of HTTP and of the database driver (crate boundaries enforce this). |
| NFR-M-02 | MUST | New site adapters SHALL be addable without touching pipeline, API, or schema code (trait + registry). |
| NFR-M-03 | MUST | New LLM providers SHALL be addable behind one trait. |
| NFR-M-04 | MUST | CI SHALL run `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `tsc --noEmit`, ESLint, and the web build. |
| NFR-M-05 | SHOULD | Public items in library crates SHALL be documented; `cargo doc` SHALL build without warnings. |

---

## 3. Constraints

| ID | Constraint |
|---|---|
| CON-01 | Backend is Rust (stable toolchain, edition 2021+). No unstable features. |
| CON-02 | Primary datastore is SQLite; the deployment must not require any external service (no Postgres, Redis, Elasticsearch, or message broker). |
| CON-03 | Target host is a personal home server: assume 2–8 cores, 4–32 GB RAM, Linux x86-64 or aarch64, possibly behind Tailscale/no public ingress. |
| CON-04 | LLM usage must be optional and local-first (Ollama / llama.cpp compatible); cloud providers are opt-in. |
| CON-05 | Must not require the user to hand over third-party site credentials. |
| CON-06 | Single-user (or a few trusted accounts). No multi-tenant isolation guarantees are claimed. |
| CON-07 | Data directory layout is a public contract once M1 ships; changes require a migration path. |
| CON-08 | The project must be usable with zero network egress except to the ingested job URLs. |

---

## 4. Acceptance scenarios

Written as executable-ish specs; each becomes an integration test.

**AS-01 — Public posting, no LLM.** Given a fixture HTML file for a Greenhouse posting with
JSON-LD and no LLM configured, when ingested by paste, then a job exists with title,
company, location, salary range in cents, ≥5 requirements from the rules stage, a file
directory containing four files, and `extraction_partial = true`.

**AS-02 — Authenticated capture.** Given a captured LinkedIn DOM fixture and a valid
extension token, when POSTed to `/api/v1/ingest/capture`, then the response is 202 with a
task id in < 100 ms, the raw HTML is on disk before the response, and after the queue drains
the job has a company, work mode, and ≥1 location.

**AS-03 — Cross-post dedupe.** Given the same role captured from LinkedIn and from the
company's Greenhouse board, when both are ingested, then exactly one `job` exists with two
`job_source_listing` rows, and the Greenhouse listing is canonical (higher-fidelity source).

**AS-04 — Refresh detects closure.** Given an ingested job whose URL now returns 404, when
refreshed, then listing status is `closed`, `closed_at` is set, the job is retained, and a
change event is recorded.

**AS-05 — Manual edit survives re-extraction.** Given a job whose salary was corrected by
hand, when the job is re-extracted, then the salary retains the manual value and provenance
`manual`.

**AS-06 — Rebuild from files.** Given a populated data directory, when the database file is
deleted and `reconcile --from-files` is run, then job/company/requirement counts and content
hashes match the pre-deletion state.

**AS-07 — Explainable match.** Given a profile with a populated experience bank and a job
with 13 required requirements, when matched, then every requirement has a verdict, the
overall score equals the documented weighted sum of reported subscores within 1e-6, and each
`met` verdict cites ≥1 accomplishment or profile skill.

**AS-08 — Tailored resume.** Given a match, when a resume is generated with a 1-page budget,
then every bullet maps to an existing accomplishment id, the document is stored as a version,
and uncovered required requirements are listed in the response.

**AS-09 — Aggregate gaps.** Given 40 saved jobs, when the learning plan is requested, then
skills are ranked by frequency × severity and each entry names the jobs it would unlock.

**AS-10 — Degraded LLM.** Given a configured LLM provider that is unreachable, when a job is
ingested, then ingestion succeeds via deterministic stages, the task ends `done` with a
warning, and the UI shows an "enrichment unavailable" badge rather than an error page.
