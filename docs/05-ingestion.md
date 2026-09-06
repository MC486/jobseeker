# Ingestion

How a URL becomes durable bytes. Semantics come later ([Extraction](06-extraction.md)).

## 1. The four entry points

| Path | Trigger | Auth | Use when |
|---|---|---|---|
| `POST /api/v1/ingest/url` | you paste a link into the UI or run `jobseeker add <url>` | session | the page is publicly reachable |
| `POST /api/v1/ingest/capture` | extension button on a page you are viewing | device token | the page needs your login (LinkedIn, Indeed) or is JS-rendered |
| `POST /api/v1/ingest/paste` | paste text/HTML | session | anything else; also the fixture path for tests |
| `poll_board` task | scheduler, opt-in | n/a | a public ATS board you explicitly named |

All four converge on the same contract: **persist the raw bytes, create a `capture` row,
enqueue `extract_job`, return the task id.** The HTTP response never waits for network I/O
or parsing (NFR-P-01).

```
POST /api/v1/ingest/url  {"url": "...", "notes": "referred by Sam"}
202 Accepted
Location: /api/v1/tasks/01927f3c-...
{"task_id":"01927f3c-...","listing_id":"01927f3c-...","status":"queued"}
```

## 2. URL canonicalization and identity

Before anything else, the URL is canonicalized so the same posting arriving five ways is
recognized once:

1. Lowercase scheme and host, strip default ports, strip fragment.
2. Drop tracking parameters: `utm_*`, `gclid`, `fbclid`, `trk`, `trkInfo`, `refId`,
   `originalSubdomain`, `position`, `pageNum`, `eBP`, `origin`, `from`, `source`, `vjk`, `tk`.
3. Apply per-host rewrite rules that reduce a URL to its identity form:

| Host pattern | Rewrite | Identity |
|---|---|---|
| `linkedin.com/jobs/view/<id>/…` | `https://www.linkedin.com/jobs/view/<id>` | numeric job id |
| `linkedin.com/jobs/collections/…?currentJobId=<id>` | same as above | `currentJobId` |
| `indeed.com/viewjob?jk=<key>` | `https://www.indeed.com/viewjob?jk=<key>` | `jk` |
| `indeed.com/rc/clk?jk=<key>` | as above | `jk` |
| `boards.greenhouse.io/<board>/jobs/<id>` | canonical | `(board, id)` |
| `job-boards.greenhouse.io/<board>/jobs/<id>` | canonical | `(board, id)` |
| `jobs.lever.co/<board>/<uuid>` | canonical | `(board, uuid)` |
| `jobs.ashbyhq.com/<board>/<uuid>` | canonical | `(board, uuid)` |
| `*.myworkdayjobs.com/…/job/…/<slug>_<reqid>` | strip locale + location; keep site | `reqid` (`R-12345`, `P751219-2`, …) |
| `*.smartrecruiters.com/…/<id>-<slug>` | canonical | `id` |
| `*.applytojob.com` (Workable) | canonical | shortcode |

4. `url_hash = blake3(url_canonical)` → `job_source_listing.url_hash` is UNIQUE. Re-ingesting
   the same link updates `last_seen_at` and creates a new `capture` rather than a duplicate
   listing.

Unknown hosts get generic canonicalization only; adding a rule later is safe because
`url_canonical` is recomputed during `reconcile`.

## 3. Server-side fetch (`ingest_url`)

Guardrails, in order — a failure at any step aborts with a specific error code:

1. **Scheme allowlist**: `http`, `https` only.
2. **SSRF guard** (NFR-S-05): resolve DNS, reject loopback, link-local (`169.254/16`,
   `fe80::/10`), private (`10/8`, `172.16/12`, `192.168/16`, `fc00::/7`), CGNAT
   (`100.64/10`), and metadata addresses. Re-check after each redirect; pin the connection
   to the validated IP to defeat DNS rebinding. Overridable by
   `acquire.allow_private_networks` for people hosting their own board.
3. **robots.txt** (FR-A-05): fetched per host, cached 24 h, evaluated for our User-Agent.
   `Disallow` → refuse with `robots_disallowed` and tell the user to use the extension
   instead. This is a deliberate design stance, not a legal opinion:
   [Security, Privacy & Legal](13-security-privacy-legal.md).
4. **Per-host rate limit**: token bucket, default one request per 2 s per host, jittered.
5. **Redirects**: max 5, each re-validated.
6. **Limits**: 20 s total timeout, 8 MB body cap, `Accept-Encoding: gzip, br`.
7. **Conditional requests** on refresh: send `If-None-Match` / `If-Modified-Since` from the
   previous capture. `304` short-circuits to "unchanged", costing nothing.

### ATS API preference (FR-A-07)

If the canonical URL matches a known ATS, fetch the JSON API instead of the HTML. Higher
fidelity, stable schema, cheaper, and explicitly published for programmatic use:

| ATS | Endpoint shape |
|---|---|
| Greenhouse | `https://boards-api.greenhouse.io/v1/boards/<board>/jobs/<id>?questions=false` |
| Lever | `https://api.lever.co/v0/postings/<board>/<id>` |
| Ashby | `https://api.ashbyhq.com/posting-api/job-board/<board>` (board-level, filter by id) |
| SmartRecruiters | `https://api.smartrecruiters.com/v1/companies/<company>/postings/<id>` |
| Workable | `https://apply.workable.com/api/v1/widget/accounts/<account>?details=true` |
| Recruitee | `https://<company>.recruitee.com/api/offers/` |
| Workday | `https://<host>/wday/cxs/<tenant>/<site>/job/<location>/<slug>_<reqid>` |

These return `provenance = api` fields with confidence 0.95+ and generally make the LLM stage
unnecessary. Endpoint shapes drift; each client owns a fixture test so breakage is loud.

### JS-rendered pages

If the fetched HTML has no JSON-LD, no adapter match, and less than ~400 characters of
extractable text, the pipeline concludes the page needs a browser and fails the task with
`needs_browser`. The UI then prompts: "Open in browser and use the extension." Optionally
(`acquire.browser.enabled`, off by default) a local headless Chrome with a persistent
profile handles it — see [ADR-0004](adr/0004-extension-capture.md) for why that is not the
default.

## 4. Extension capture (`ingest_capture`)

The primary path for authenticated sites. The extension has no scraping intelligence; it
ships the page and lets the server think.

```jsonc
POST /api/v1/ingest/capture
Authorization: Bearer <device-token>
{
  "url": "https://www.linkedin.com/jobs/view/4123456789/",
  "captured_at": "2026-09-05T20:21:00Z",
  "html": "<!doctype html>…",          // documentElement.outerHTML, post-hydration
  "text": "…",                          // innerText of the detected job container
  "selected_html": "…",                 // optional: user's selection, wins if present
  "screenshot_png_b64": "…",            // optional
  "source_hint": "linkedin",
  "client": { "name": "jobseeker-ext", "version": "0.1.0", "browser": "firefox/141" },
  "page_meta": { "title": "…", "jsonld": [ … ] }   // pre-extracted for robustness
}
```

Server-side handling:

1. Validate token scope (`ingest`) and size limits (8 MB HTML, 12 MB screenshot; NFR-S-09).
2. Write `html` zstd-compressed to `captures/<hh>/<hash>.html.zst`, screenshot to
   `media/<hh>/<hash>.png`; both content-addressed, so re-saving a page you already have is
   free.
3. Insert `capture` (`method='extension'`) and upsert `job_source_listing` by `url_hash`.
4. Enqueue `extract_job` with `dedupe_key = "extract:<capture_hash>"`.
5. Return `202` with the task id. **Total server work is two file writes and two inserts**,
   comfortably inside the 100 ms budget.

The extension expands "show more" descriptions and waits for hydration before capturing —
see [Extension](../apps/extension/README.md).

## 5. Refresh (`refresh_listing`)

Scheduled (default: open jobs older than 24 h, applied-to jobs every 12 h) or manual.

```
conditional GET ─┬─ 304 ─────────────► touch last_checked_at, done
                 ├─ 200 same hash ───► touch last_seen_at, done
                 ├─ 200 new hash ────► new capture → re-extract → job_revision diff
                 │                      material change (salary/title/status/closes_at)
                 │                      → event + UI badge
                 ├─ 404 / 410 ───────► status = closed, closed_at = now
                 ├─ 200 + closure marker ─► status = closed
                 └─ network error ───► check_failures += 1; after 3, status = unknown
```

Closure markers are per-adapter strings ("no longer accepting applications", "This job is
no longer available", Greenhouse `"live": false`). Data is never deleted on closure — a
closed posting is exactly the evidence you need when a recruiter asks what you applied to.

## 6. Deduplication (`dedupe_job`)

Runs after extraction. Cross-posting is the norm: the same role on LinkedIn, Indeed, and the
company's Greenhouse board.

**Stage 1 — exact keys.** Same `(source_id, source_job_id)`, or same `url_hash`, or same ATS
`(board, req_id)` extracted from an aggregator's apply link (LinkedIn "Apply on company
website" often exposes the real ATS URL). Exact match → merge, no ambiguity.

**Stage 2 — strong heuristic.** Candidates share `company_id` and their `title_normalized`
tokens have Jaccard ≥ 0.8, and either primary location matches or both are remote, and
`posted_at` is within 45 days. Then compare descriptions: token-set cosine ≥ 0.85 (plus
embedding cosine ≥ 0.93 when embeddings exist) → merge.

**Stage 3 — flag, do not merge.** Score in the ambiguous band (0.6–0.85) creates a
`possible_duplicate` event surfaced in the UI for a one-click merge/split decision. Silent
wrong merges are far worse than visible duplicates.

**Merge semantics.** Highest-`source.fidelity` listing becomes canonical and its fields win
per the provenance order; other listings attach to the same `job`; requirements are unioned
by `normalized_text`; salary conflicts create an `extraction_conflict` row (you want to know
that the aggregator's "estimate" was $20k below the ATS's real band). Merges record enough
information to be reversible (FR-A-11).

**User-initiated merge (implemented).** Stages 1–2 auto-merge are still design; the server
does not silently fold two jobs. `POST /api/v1/jobs/:id/merge {into_job_id}` and
`jobseeker merge <from> <into>` do the union described above, require the same
`company_id`, keep the keeper's title and description, soft-delete the donor, and write a
tombstone `reason = merged_into:{into}`.

**Stage 3 flag (implemented).** `GET /api/v1/jobs/:id/duplicates` lists live jobs at the
same company whose `title_normalized` Jaccard is ≥ 0.6. A candidate is `strong` when
Jaccard ≥ 0.8 and both are remote or share a location string. The UI offers Compare /
Merge; it never merges on its own.

**User-initiated split (FR-A-11).** `POST /api/v1/jobs/:id/split {listing_id}` peels that
listing into a new job at the same company. The original keeps its fields. The listing is
re-extracted from its latest capture when one exists so the new job is not a clone of the
unioned requirement set. The only listing on a job cannot be split.

**Conflict resolve (implemented).** `GET /api/v1/jobs/:id/conflicts` lists disagreements
written on merge (salary raw strings today). `POST
/api/v1/jobs/:id/conflicts/:id/resolve {choice}` applies `a`, `b`, or `keep`. A pick
re-parses the salary and stamps provenance `manual`; `keep` only marks the row resolved.
The server never chooses a side.

## 7. Error taxonomy

| Code | HTTP | Meaning | Retry |
|---|---|---|---|
| `invalid_url` | 400 | unparseable or non-http scheme | no |
| `blocked_private_network` | 400 | SSRF guard | no |
| `robots_disallowed` | 422 | robots.txt refuses | no — use the extension |
| `fetch_timeout` | — | task-level | yes, backoff |
| `fetch_status_4xx` | — | 401/403 → likely needs auth | no — use the extension |
| `fetch_status_5xx` | — | site broken | yes, backoff |
| `needs_browser` | — | no extractable content | no — use the extension |
| `payload_too_large` | 413 | over caps | no |
| `unsupported_content_type` | 415 | PDF/image only | no (PDF support is M4) |
| `extract_failed` | — | all stages failed | yes ×2, then failed with raw retained |
| `llm_unavailable` | — | provider down | task succeeds partially; `extraction_partial = true` |

Every failure keeps the raw capture, so a fixed adapter can re-extract historical captures
with `jobseeker reextract --since <date>` — bugs are recoverable rather than lossy.
