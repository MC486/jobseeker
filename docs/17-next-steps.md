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
- Default profile (Rust 8y, Kubernetes 5y) seeded on `Pipeline::open`;
  replaced by `jobseeker profile import` / `POST /api/v1/profiles/default/import-resume`
- Field provenance rows; PATCH `/api/v1/jobs/{id}` is sticky `manual`
- HTTP API: health, ready, meta, ingest, jobs, match, tasks, OpenAPI, SPA fallback
- Auth: pairing tokens, plus password sessions (`jobseeker user set-password`)
- CLI: `serve`, `migrate`, `add`, `list`, `show`, `merge`, `split`, `pair`, `user set-password`,
  `profile import|show`, `reconcile`, `openapi`
- React/Vite/Tailwind shell in `web/` (score + confidence dots)
- MV3 extension with pairing in `extension/`
- Paste fixture: `fixtures/greenhouse-platform-engineer.html`

## Do next (M0 remaining → M1)

1. **File-based routes + virtualized job table.** Split is in. Next is
   file-based routes and TanStack Table.

## Just shipped

- **Split (FR-A-11).** `POST /api/v1/jobs/:id/split {listing_id}` and
  `jobseeker split <job> <listing>` peel one listing off a merged job into a
  new stub at the same company. The original keeps its title, description, and
  unioned requirements. The peeled listing is re-extracted from its latest
  capture when one exists, so the new job is not a clone of the unioned bars.
  Cannot split the only listing. Job detail shows **Split off** next to each
  source when there are two or more.

- **Merge.** User-initiated only. `POST /api/v1/jobs/:id/merge {into_job_id}`
  (`:id` is the donor) and `jobseeker merge <from> <into>` absorb a cross-post
  into another job of the **same company**. Listings move onto the keeper;
  `attach_to_job` re-picks the highest-fidelity source as canonical (Workday
  beats LinkedIn). Requirements union by `normalized_text`; locations union by
  `raw`; empty `apply_url` / salary copy onto the keeper; conflicting salary
  bands become an unresolved `extraction_conflict`. The donor is soft-deleted
  and tombstoned `merged_into:{into}`. The keeper is rescored in-process.
  Compare shows merge actions when two columns share a company — keep the
  fuller ATS posting. Silent auto-merge is not this slice.

- **Compare.** `/jobs/compare?ids=` lines 2–4 jobs up: location, mode, comp,
  Overall / Skills / Years / Required / Preferred / Seniority / Comp /
  Location, plus each posting's required bars. Highest value in a score
  row is highlighted. Overall is unchanged. The list checkboxes feed the
  URL so a compare is shareable and the back button works.

- **TanStack Router + Query.** `/`, `/jobs`, `/jobs/$jobId`, `/profile`,
  `/tasks/$taskId` are a code-based route tree. Search `?q=` and sort
  `?sort=overall|skills|years` live in the URL (sort is client-side on the
  current page and does not change overall). SSE invalidates Query keys
  instead of a `live` counter. Back button and reload keep the list
  filter.

- **Match breakdown.** Overall is unchanged. The job page and the jobs list
  show Skills (required bars with tenure stripped) and Years (required
  year-count asks) next to overall, plus Required / Preferred / Seniority /
  Comp / Location on the detail page. A 5-year wishlist no longer hides a
  real skills match — or a recruiter who will screen on years. Verdicts that
  stated a year bar also show `have / asked`. List rows derive the split
  from `requirement_match` (or `explanation_json` when verdict rows are
  missing); they are not new weights.

- **Workday adapter.** `ingest_url` on a public `*.myworkdayjobs.com` posting
  fetches the CXS JSON (`/wday/cxs/{tenant}/{site}/job/…`) instead of the SPA
  shell. Requisition ids include product-style ids (`P751219-2`), not only
  `R-` / `REQ` / `JR`. Locale and location slugs collapse to one identity, so
  Remote-USA and Seattle options of the same req do not fork. HTML fallback
  reads `data-automation-id`. LinkedIn of the same posting still returns
  `needs_browser`. Fixtures: `fixtures/workday-cxs-job.json`,
  `fixtures/workday-job-capture.html`.

- **Experience-bank import.** Markdown (evidence bank or a conventional resume)
  parses deterministically into `experience_item` + `accomplishment` +
  `profile_skill`. Placeholder Rust/K8s years are replaced. Jobs rescore
  against honest tenure and the imported skill bank. CLI:
  `jobseeker profile import --file bank.md`. UI: `/profile`.
  Fixture: `fixtures/evidence-bank.md` (sanitized). Do not commit a personal
  bank that contains a phone number, employer email, or internal names.

- **Password auth.** `auth.mode = password` gates the API. Login is
  `POST /api/v1/auth/login` → `js_session` cookie (HttpOnly, SameSite=Lax,
  Secure behind TLS). Passwords are Argon2id (m=19 MiB, t=2, p=1). Sessions
  are 256-bit `jss_` tokens stored as BLAKE3 hashes. Failed logins are
  rate-limited per IP. Bootstrap: `jobseeker user set-password`. Bearer
  owner tokens still work for the CLI.

- **Site adapters.** LinkedIn / Indeed / Greenhouse / Lever / Ashby / Workday fill only
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
- `web/src/router.tsx` — TanStack route tree; `web/src/compare.ts` — compare ids
- `extension/popup.js` — capture polish / origin allowlist
