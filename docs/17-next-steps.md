# Next steps

The walking skeleton (M0) is in place: a pasted Greenhouse-style page becomes a
SQLite row, atomized requirements, `job.md` / `job.json` / `requirements.json`,
and an explainable match score against the default profile. The extension can
pair with a hashed, ingest-scoped device token.

## Done in this repo

- Domain model, config, error taxonomy (`crates/core`)
- SQLite schema, dual pools, FTS5, task queue, persist (`crates/db`)
- Normalizers, acquisition guards, extraction, store, matching, resume
- Pipeline: `ingest_url` / `ingest_paste` / `ingest_capture` → extract → files → score
- Default profile (Rust 8y, Kubernetes 5y) seeded on `Pipeline::open`
- Field provenance rows; PATCH `/api/v1/jobs/{id}` is sticky `manual`
- HTTP API: health, ready, meta, ingest, jobs, match, tasks, OpenAPI, SPA fallback
- Auth: `POST /api/v1/auth/pair`, hashed `device_token`, `jobseeker pair` / `tokens` / `revoke`
- CLI: `serve`, `migrate`, `add`, `list`, `show`, `pair`, `reconcile`, `openapi`
- React/Vite/Tailwind shell in `web/` (score + confidence dots)
- MV3 extension with pairing in `extension/`
- Paste fixture: `fixtures/greenhouse-platform-engineer.html`

## Do next (M0 remaining → M1)

1. **Password auth.** Session cookie + Argon2id when `auth.mode = password`.
2. **TanStack Router + Query.** The shell still uses a hand-rolled router; SSE
   already invalidates the job list / task / match views.
3. **Workday adapter.** SPA; extension path. Location lives in
   `data-automation-id`; `reqId` is the durable identifier.

## Just shipped

- **Site adapters.** LinkedIn / Indeed / Greenhouse / Lever / Ashby fill only
  empty fields (`Provenance::Adapter`, 0.85) after JSON-LD and before rules.
  Selectors are fallback lists; a miss degrades to heuristics. LinkedIn /
  Indeed URLs still return `needs_browser` and are never server-fetched.
  Fixtures: `fixtures/linkedin-job-capture.html`,
  `fixtures/greenhouse-board-job.html`.

- **Reconcile.** `jobseeker reconcile --check` / `--to-files` / `--from-files`.
  `--from-files` rebuilds jobs from `jobs/**/job.json` and skips tombstones
  (AS-06 / FR-S-04 / FR-S-07). `--rebuild-derived` is still just FTS on write.

- **SSE `GET /api/v1/events?since=`** — `event_log` + in-process broadcast;
  TaskPage no longer polls.
- **Generated TS client.** `jobseeker openapi [--out]` dumps the spec without a
  server; `just gen-client` writes committed `web/src/api/generated.ts` (ADR-0009).

## Acceptance for the current spine

```
cargo test -p jobseeker-pipeline -p jobseeker-api -p jobseeker-db
# pairing_code_mints_an_ingest_token_that_cannot_read_jobs must stay green
# reconcile_from_files_rebuilds_after_the_db_is_deleted must stay green

cargo run -p jobseeker-cli -- pair --name "Firefox on laptop"
# paste the code into the extension, or:
cargo run -p jobseeker-cli -- pair --name "Firefox on laptop" --emit-token
```

A LinkedIn URL must still fail with `needs_browser` and must not be fetched.

## Files that are safe starting points

- `crates/api/src/lib.rs` — SSE events, password login
- `crates/pipeline/src/lib.rs` — `refresh_listing`, reconcile handlers
- `web/src/App.tsx` — replace the hand-rolled router with TanStack Router
- `extension/popup.js` — capture polish / origin allowlist
