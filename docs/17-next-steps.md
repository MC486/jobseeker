# Next steps

The walking skeleton (M0) is in place: a pasted Greenhouse-style page becomes a
SQLite row, atomized requirements, and `job.md` / `job.json` / `requirements.json`.
This file is what the next agent should do, in order.

## Done in this repo

- Domain model, config, error taxonomy (`crates/core`)
- SQLite schema, dual pools, FTS5, task queue, persist (`crates/db`)
- Normalizers, acquisition guards, extraction, store, matching, resume
- Pipeline: `ingest_url` / `ingest_paste` / `ingest_capture` → extract → files
- HTTP API: health, ready, meta, ingest, jobs, tasks, OpenAPI, SPA fallback
- CLI: `serve`, `migrate`, `add`, `list`, `show`
- React/Vite/Tailwind shell in `web/`
- MV3 extension skeleton in `extension/`

## Do next (M0 remaining → M1)

1. **Wire a real public fetch in CI-safe form.** Keep the paste spine as the
   hermetic test. Add a fixture ATS JSON file and a `jobseeker add --paste`
   example under `fixtures/`.
2. **Job detail completeness.** Persist field provenance rows. Surface
   confidence dots in the UI. PATCH that sets `Manual` provenance.
3. **Default profile + score_match.** Load a profile snapshot from SQLite and
   call `jobseeker_matching::score`. Persist `match_score`.
4. **Extension pairing.** `POST /api/v1/auth/pair` + hashed device tokens.
   The extension already sends `Authorization: Bearer` when a token is set.
5. **Generated TS client.** Dump `/openapi.json` and run `openapi-typescript`
   into `web/src/api/schema.d.ts` (gitignored until CI generates it).
6. **SSE `/api/v1/events`.** The UI currently polls a task; replace with the
   event stream described in `docs/10-frontend.md`.
7. **Reconcile.** `jobseeker reconcile --from-files` / `--to-files` / `--check`.
8. **Site adapters.** LinkedIn / Indeed (from extension HTML), Greenhouse /
   Lever / Ashby JSON already partially works via `acquire::plan`.

## Acceptance for the current spine

```
cargo test -p jobseeker-pipeline
# paste_travels_the_spine_to_files_and_a_queryable_row must stay green

cargo run -p jobseeker-cli -- add --paste /tmp/job.html \
  -- https://boards.greenhouse.io/example/jobs/1
cargo run -p jobseeker-cli -- list
```

A LinkedIn URL must still fail with `needs_browser` and must not be fetched.

## Files that are safe starting points

- `crates/pipeline/src/lib.rs` — next handlers (`score_match`, `refresh_listing`)
- `crates/db/src/persist.rs` — provenance + revision writes
- `crates/api/src/lib.rs` — auth, events, PATCH jobs
- `web/src/App.tsx` — replace the hand-rolled router with TanStack Router
- `extension/popup.js` — pairing flow
