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
- CLI: `serve`, `migrate`, `add`, `list`, `show`, `pair`
- React/Vite/Tailwind shell in `web/` (score + confidence dots)
- MV3 extension with pairing in `extension/`
- Paste fixture: `fixtures/greenhouse-platform-engineer.html`

## Do next (M0 remaining → M1)

1. **Reconcile.** `jobseeker reconcile --from-files` / `--to-files` / `--check`.
2. **Site adapters.** LinkedIn / Indeed (from extension HTML), Greenhouse /
   Lever / Ashby JSON already partially works via `acquire::plan`.
3. **Password auth.** Session cookie + Argon2id when `auth.mode = password`.
4. **TanStack Router + Query.** The shell still uses a hand-rolled router; SSE
   already invalidates the job list / task / match views.

## Just shipped

- **SSE `GET /api/v1/events?since=`** — `event_log` + in-process broadcast;
  TaskPage no longer polls.
- **Generated TS client.** `jobseeker openapi [--out]` dumps the spec without a
  server; `just gen-client` writes committed `web/src/api/generated.ts` (ADR-0009).

## Acceptance for the current spine

```
cargo test -p jobseeker-pipeline -p jobseeker-api -p jobseeker-db
# pairing_code_mints_an_ingest_token_that_cannot_read_jobs must stay green

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
