# Next steps

The walking skeleton (M0) is in place: a pasted Greenhouse-style page becomes a
SQLite row, atomized requirements, `job.md` / `job.json` / `requirements.json`,
and an explainable match score against the default profile.

## Done in this repo

- Domain model, config, error taxonomy (`crates/core`)
- SQLite schema, dual pools, FTS5, task queue, persist (`crates/db`)
- Normalizers, acquisition guards, extraction, store, matching, resume
- Pipeline: `ingest_url` / `ingest_paste` / `ingest_capture` → extract → files → score
- Default profile (Rust 8y, Kubernetes 5y) seeded on `Pipeline::open`
- Field provenance rows; PATCH `/api/v1/jobs/{id}` is sticky `manual`
- HTTP API: health, ready, meta, ingest, jobs, match, tasks, OpenAPI, SPA fallback
- CLI: `serve`, `migrate`, `add`, `list`, `show`
- React/Vite/Tailwind shell in `web/` (score + confidence dots)
- MV3 extension skeleton in `extension/`
- Paste fixture: `fixtures/greenhouse-platform-engineer.html`

## Do next (M0 remaining → M1)

1. **Extension pairing.** `POST /api/v1/auth/pair` + hashed device tokens.
   The extension already sends `Authorization: Bearer` when a token is set.
2. **Generated TS client.** Dump `/openapi.json` and run `openapi-typescript`
   into `web/src/api/schema.d.ts` (gitignored until CI generates it).
3. **SSE `/api/v1/events`.** The UI currently polls a task; replace with the
   event stream described in `docs/10-frontend.md`.
4. **Reconcile.** `jobseeker reconcile --from-files` / `--to-files` / `--check`.
5. **Site adapters.** LinkedIn / Indeed (from extension HTML), Greenhouse /
   Lever / Ashby JSON already partially works via `acquire::plan`.

## Acceptance for the current spine

```
cargo test -p jobseeker-pipeline
# paste_travels_the_spine_to_files_and_a_queryable_row must stay green
# and a match_score row must exist with rust/k8s credit > 0

cargo run -p jobseeker-cli -- add --paste fixtures/greenhouse-platform-engineer.html \
  -- https://boards.greenhouse.io/acmerobotics/jobs/5512034
cargo run -p jobseeker-cli -- list
```

A LinkedIn URL must still fail with `needs_browser` and must not be fetched.

## Files that are safe starting points

- `crates/api/src/lib.rs` — auth pairing, SSE events
- `crates/pipeline/src/lib.rs` — `refresh_listing`, reconcile handlers
- `web/src/App.tsx` — replace the hand-rolled router with TanStack Router
- `extension/popup.js` — pairing flow
