# HTTP API

Base: `/api/v1`. The authoritative machine-readable contract is `GET /openapi.json`,
generated from the handlers. This document is the human-readable overview and the design
rules.

## Conventions

- JSON in, JSON out; `Content-Type: application/json` required on writes.
- Auth: session cookie (`js_session`, `HttpOnly`, `SameSite=Lax`, `Secure` when behind TLS)
  for the UI; `Authorization: Bearer <token>` for the CLI and the extension.
- Errors: `{"error":{"code","message","details?","trace_id"}}`; `code` is a stable string.
- Lists: `{"items":[…],"next_cursor":"…"|null,"total_estimate":n}`; `?limit=` default 50,
  max 200; cursors are opaque.
- Timestamps: RFC3339 UTC. Money: `{"min_cents","max_cents","currency","period","is_estimate"}`.
- Async work: `202 Accepted` + `{"task_id"}` + `Location: /api/v1/tasks/{id}`.
- Concurrency: `PATCH` accepts `If-Match` with the entity's `updated_at` ETag; mismatch → 409.
- Every response carries `X-Trace-Id`.

## Surface

### Health & meta (unauthenticated)

```
GET  /healthz                     → 200 "ok" (process is alive)
GET  /readyz                      → 200 {db, migrations, queue_depth} | 503
GET  /metrics                     → Prometheus text
GET  /api/v1/meta                 → {version, git_sha, schema_version, llm: {provider, model, available}}
GET  /openapi.json
```

### Auth

```
POST /api/v1/auth/login           {username, password} → sets cookie, {user}
POST /api/v1/auth/logout          → 204
GET  /api/v1/auth/me              → {user, auth_mode}
POST /api/v1/auth/tokens          {name, scopes[]} → {token}   ← shown once, stored hashed
GET  /api/v1/auth/tokens          → device tokens (never the secret)
DELETE /api/v1/auth/tokens/:id    → 204
POST /api/v1/auth/pair            {code} → {token}             ← extension pairing
```

### Ingestion

```
POST /api/v1/ingest/url           {url, notes?, tags?}                       → 202
POST /api/v1/ingest/capture       {url, html, text?, screenshot_png_b64?, …} → 202   [scope: ingest]
POST /api/v1/ingest/paste         {text, url?, source_hint?}                 → 202
POST /api/v1/listings/:id/refresh                                            → 202
POST /api/v1/jobs/:id/reextract   {force_llm?}                               → 202
```

### Tasks & events

```
GET  /api/v1/tasks?status=&kind=&limit=
GET  /api/v1/tasks/:id            → {id, kind, status, progress, progress_message, attempts, last_error, …}
POST /api/v1/tasks/:id/cancel     → 202
POST /api/v1/tasks/:id/retry      → 202
GET  /api/v1/events?since=<event_id>   → SSE stream of DomainEvent (backfills from event_log)
```

### Jobs

```
GET  /api/v1/jobs
     ?q=                          full-text (FTS5, supports quoted phrases and OR)
     &status=open|closed|…        &work_mode=remote|hybrid|onsite
     &company_id=  &seniority=    &employment_type=
     &min_salary=  &max_salary=   &currency=
     &country=  &region=  &city=
     &tag=  &has_application=true|false  &archived=
     &min_match=0.7&profile_id=   &requires_clearance=false
     &posted_after=  &closes_before=
     &sort=posted_at|match|salary|updated_at|relevance   &order=asc|desc
     &cursor=&limit=
GET  /api/v1/jobs/:id             → full job + company + locations + listings + counts
PATCH /api/v1/jobs/:id            {title?, salary?, user_rating?, user_notes_md?, status?, is_archived?, …}
                                    → fields set here become provenance `manual`
DELETE /api/v1/jobs/:id           ?purge_files=true  → soft delete by default
GET  /api/v1/jobs/:id/requirements
POST /api/v1/jobs/:id/requirements          create manual requirement
PATCH /api/v1/requirements/:id              retype / reclassify / relink skill
DELETE /api/v1/requirements/:id
GET  /api/v1/jobs/:id/raw/:capture_id       sanitized original capture (restrictive CSP)
GET  /api/v1/jobs/:id/revisions             change history
GET  /api/v1/jobs/:id/conflicts             cross-source disagreements
POST /api/v1/jobs/:id/merge      {into_job_id}
POST /api/v1/jobs/:id/split      {listing_id}
GET  /api/v1/jobs/:id/similar    → nearest neighbours by embedding
```

### Companies, skills, tags, views

```
GET/POST/PATCH  /api/v1/companies[/:id]         GET /:id/jobs
GET  /api/v1/skills?q=  POST /api/v1/skills  POST /api/v1/skills/:id/aliases
GET  /api/v1/skills/candidates  POST /api/v1/skills/candidates/:id/promote
GET/POST/DELETE /api/v1/tags[/:id]              POST/DELETE /api/v1/jobs/:id/tags/:tag_id
GET/POST/PATCH/DELETE /api/v1/views[/:id]       saved filter sets
```

### Profile & experience bank

```
GET/POST/PATCH  /api/v1/profiles[/:id]
GET/POST/PATCH/DELETE  /api/v1/profiles/:id/experience[/:item_id]
GET/POST/PATCH/DELETE  /api/v1/experience/:item_id/accomplishments[/:acc_id]
POST /api/v1/accomplishments/:id/variants        generate phrasing variants
GET/PUT/DELETE          /api/v1/profiles/:id/skills[/:skill_id]
POST /api/v1/profiles/:id/import-resume          multipart → 202, drafts for review
GET  /api/v1/profiles/:id/import-drafts          POST /api/v1/import-drafts/:id/accept
GET  /api/v1/profiles/:id/coverage               skills demanded by saved jobs vs your evidence
GET/POST/PATCH  /api/v1/profiles/:id/answers     question/answer bank
```

### Matching

```
GET  /api/v1/jobs/:id/match?profile_id=          cached; 404 if never computed
POST /api/v1/jobs/:id/match     {profile_id, weights?} → 202 (or 200 with ?sync=true)
GET  /api/v1/profiles/:id/matches?min=&sort=     ranked pipeline
GET  /api/v1/profiles/:id/gaps?status=open       aggregate learning plan
PATCH /api/v1/gaps/:id          {status, target_date, notes}
POST /api/v1/matches/recompute  {profile_id?, only_stale?} → 202
GET  /api/v1/compare?job_ids=a,b,c&profile_id=   side-by-side
```

### Documents

```
POST /api/v1/documents/resume        {job_id, profile_id, template, budget:{pages|bullets}} → 202
POST /api/v1/documents/cover-letter  {job_id, profile_id, tone?, length?}                   → 202
POST /api/v1/documents/interview-prep {job_id, profile_id}                                  → 202
GET  /api/v1/documents?kind=&job_id=&profile_id=
GET  /api/v1/documents/:id                       source + selection + coverage
GET  /api/v1/documents/:id/render.pdf
GET  /api/v1/documents/:id/diff/:other_id
POST /api/v1/documents/:id/regenerate            {overrides} → new version
DELETE /api/v1/documents/:id
GET  /api/v1/templates                           available resume templates
```

### Applications

```
GET/POST/PATCH/DELETE  /api/v1/applications[/:id]
POST /api/v1/applications/:id/events   {kind, occurred_at, title, body_md}
GET  /api/v1/applications/:id/events
GET  /api/v1/applications/board                  kanban: statuses → items
GET  /api/v1/applications/next-actions?window=7d overdue + upcoming
GET/POST/PATCH/DELETE  /api/v1/contacts[/:id]
```

### Dashboard & analytics

```
GET  /api/v1/stats/dashboard   → {pipeline_counts, next_actions, closing_soon,
                                  recent_jobs, top_gaps, queue_health, weekly_activity}
GET  /api/v1/stats/market      ?group_by=title|seniority|region|skill  (M4)
GET  /api/v1/stats/funnel      conversion by source/title/seniority    (M4)
```

### Admin & data

```
POST /api/v1/admin/reconcile   {direction: "to_files"|"from_files"|"rebuild_derived"} → 202
POST /api/v1/admin/backup                                                            → 202
GET  /api/v1/admin/export      ?format=json|csv|parquet&entity=
GET  /api/v1/admin/llm-calls   ?purpose=  cost + cache audit
DELETE /api/v1/admin/llm-calls ?before=   prune cache/log
GET  /api/v1/admin/settings    PUT /api/v1/admin/settings
```

## Design rules

1. **DTOs ≠ domain types.** `jobseeker-api::dto` owns wire types with `From` conversions.
   The domain model must be refactorable without breaking clients.
2. **No endpoint does unbounded work synchronously.** Anything touching the network or an
   LLM returns a task. `?sync=true` exists only for pure-CPU operations (match recompute)
   and only for a single entity.
3. **Filters are composed in SQL, never in Rust.** A filtered list must not load rows to
   discard them.
4. **`PATCH` is field-level and provenance-aware.** Setting a field by hand marks it
   `manual` so automation cannot clobber it.
5. **No endpoint returns raw HTML unsanitized.** Captures are served sanitized with a
   restrictive CSP (NFR-S-08).
6. **Additive evolution.** New optional fields and new endpoints only; breaking changes
   require `/api/v2`.
7. **Scopes are minimal.** The extension token can reach `POST /ingest/capture` and nothing
   else — a leaked device token cannot read your resume or your applications.

## Rate limits & sizes

| Endpoint class | Limit |
|---|---|
| `POST /auth/login` | 10 / 15 min per IP, exponential lockout |
| `POST /ingest/*` | 60 / min |
| `POST /documents/*` | 20 / min (LLM cost control) |
| everything else | 600 / min |
| body: HTML capture | 8 MB |
| body: screenshot | 12 MB |
| body: resume upload | 20 MB |
| body: default | 1 MB |
