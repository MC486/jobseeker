# Agent notes

This file is the contract for anyone (human or agent) writing code in this repo.
The product docs live in `docs/`. Start with `docs/00-vision.md` and
`docs/17-next-steps.md`.

## What this is

A self-hosted, single-user job-search workbench. Paste or capture a posting →
structured, queryable, durable data. Later: match against an experience bank,
write a resume that cannot invent facts, and explain qualification gaps.

## Invariants — do not violate these

- **No third-party credentials.** LinkedIn/Indeed/Glassdoor are captured by the
  browser extension from a page the user is already viewing. Server-side fetch
  is for public pages only. `acquire::plan` must keep returning `NeedsBrowser`
  for authenticated hosts.
- **SSRF + robots.txt + rate floor.** Per-host delay ≥ 500 ms. Private,
  loopback, link-local, CGNAT and metadata addresses are blocked unless the
  operator opts in. Honour robots.txt.
- **Persist raw bytes first.** A capture is written to disk and recorded before
  any parsing. Extraction bugs are re-runs, not lost postings.
- **Provenance is sticky.** `Manual` is never overwritten by automation.
  Weaker stages cannot clobber stronger ones.
- **SQLite is the primary store.** WAL, FTS5, JSON1. Dual pools: one writer
  connection, N readers. Do not introduce a second database.
- **Files and DB together.** Each job materializes `job.md`, `job.json`,
  `requirements.json`. Writes are `tmp → fsync → rename`.
- **LLM is optional and local-first.** Default provider is `None`. Deterministic
  stages must produce a usable record alone. Cache successes, never failures.
  Schema-constrained JSON only.
- **Matching is explainable.** Weighted named subscores, not one opaque
  percentage. An unmet clearance/blocker caps the overall score (default 0.45).
- **Resumes cannot invent facts.** Selection is deterministic. A model may
  rephrase; new digits or taxonomy skills in a rewrite are rejected.
- **Unknown config keys fail boot.** Env prefix is `JOBSEEKER__`.
- **Do not store annualized salary.** Keep the stated period; annualize in
  queries and scores only.
- **Unspaced slashes are not splitters.** `TS/SCI` and `CI/CD` stay whole.
- **`City, CA` is California**, not Canada. Try subdivision before country.

## Crate map

| Crate | Responsibility |
|---|---|
| `jobseeker-core` | Domain, ids, config, errors. No SQL, HTTP, or filesystem. |
| `jobseeker-db` | Pools, migrations, repositories, queue, persist. |
| `jobseeker-normalize` | Pure string functions. Highly tested. |
| `jobseeker-llm` | Trait + Ollama/OpenAI/Anthropic/Mock + cache. |
| `jobseeker-acquire` | Plan, SSRF, robots, rate limit, fetch. |
| `jobseeker-extract` | Capture → `ExtractedJob` + atomized requirements. |
| `jobseeker-store` | Atomic file writes, blobs, `job.md` / `job.json`. |
| `jobseeker-matching` | Explainable scoring. |
| `jobseeker-resume` | Experience-bank projection + no-new-facts. |
| `jobseeker-pipeline` | Ingest → extract → materialize worker. |
| `jobseeker-api` | axum, OpenAPI, SPA fallback. |
| `jobseeker-cli` | `jobseeker` binary. |

## How to work

- Prefer extending an existing crate over adding a new one.
- Repositories are the only place SQL is written.
- API handlers return DTOs, never sqlx types.
- Tests go next to the code they pin. A pipeline paste→files test is the M0
  acceptance check; do not delete it.
- `sqlx` 0.9: wrap dynamic SQL in `sqlx::AssertSqlSafe(...)`.
- reqwest 0.13 TLS features are `rustls` + `webpki-roots`, not `rustls-tls`.
- Keep the public binary looping on `127.0.0.1` by default.

## Commands

```
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p jobseeker-cli -- serve
cargo run -p jobseeker-cli -- add --paste path/to/job.html
cd web && npm install && npm run build
```
