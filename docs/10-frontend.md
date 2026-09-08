# Frontend

## Stack

| Concern | Choice | Why |
|---|---|---|
| Framework | React 19 + TypeScript (strict) | ecosystem depth for tables/virtualization/editors |
| Build | Vite 7 | fast dev, good code splitting, brotli output |
| Routing | TanStack Router (code-based tree; file-based later) | typed params/search — filters live in the URL safely |
| Server state | TanStack Query | caching, invalidation, optimistic updates |
| Styling | Tailwind CSS v4 | no naming overhead, dark mode, tiny output |
| Components | Radix primitives + local components | accessible, unstyled, no design-system lock-in |
| Forms | React Hook Form + Zod | Zod schemas derived from the OpenAPI types |
| Tables | TanStack Table + Virtual | 10k rows without pagination jank |
| Charts | Recharts | only the dashboard needs it; lazy-loaded |
| API types | `openapi-typescript` from `/openapi.json` | never hand-write a response type |

## State discipline

Exactly three kinds of state, in priority order:

1. **URL** — filters, sort, selection, open panel, active tab. Views are shareable, the back
   button works, and a reload lands you where you were.
2. **TanStack Query** — everything from the server. No duplication into a store. Query keys
   mirror the URL shape: `['jobs', {q, status, work_mode, cursor}]`.
3. **`useState` / `useReducer`** — ephemeral UI only (dropdown open, draft text).

No Redux, no Zustand, no context-as-store. The SSE stream drives precise invalidation:

```ts
// one subscription for the whole app
onEvent(e => {
  switch (e.kind) {
    case 'task.updated':   qc.setQueryData(['task', e.entity_id], e.payload); break;
    case 'job.created':
    case 'job.updated':    qc.invalidateQueries({queryKey: ['jobs']});
                           qc.invalidateQueries({queryKey: ['job', e.entity_id]}); break;
    case 'match.updated':  qc.invalidateQueries({queryKey: ['match', e.entity_id]}); break;
    case 'document.ready': qc.invalidateQueries({queryKey: ['documents']}); break;
  }
});
```

The practical effect: you paste a URL, and the row appears immediately as "extracting…",
then fills in with title, salary, and match score as the pipeline progresses — without a
single poll.

## Routes

```
/                          Dashboard
/jobs                      Job list (filters in URL, virtualized, bulk actions)
/jobs/$jobId               Job detail
  ├─ overview              structured summary + confidence badges
  ├─ requirements          typed requirement table with met/gap chips, inline edit
  ├─ match                 subscore breakdown, evidence, gaps
  ├─ description           normalized Markdown, requirement spans highlighted
  ├─ sources               listings, captures, revisions, conflicts
  └─ documents             resumes/cover letters for this job
/jobs/compare?ids=a,b,c    side-by-side
/pipeline                  Kanban board of applications
/applications/$id          timeline, contacts, documents, next action
/profile                   personas
/profile/$profileId
  ├─ experience            roles/projects tree + accomplishment editor
  ├─ skills                skills with years/level/recency + evidence counts
  ├─ coverage              demanded-by-your-jobs vs your evidence
  └─ answers               question/answer bank
/gaps                      learning plan (aggregate, ranked by leverage)
/documents                 all generated docs, versions, diffs
/companies, /companies/$id
/settings                  LLM provider, weights, sources, extension pairing, backups
/tasks                     queue inspector (kind, status, retries, errors)
```

## Key screens

**Compare** (`/jobs/compare?ids=`) is a shareable side-by-side of 2–4 jobs: the
same Overall / Skills / Years split plus Required / Preferred / Seniority / Comp /
Location, then each posting's requirement verdicts. Highest value in a score row
is highlighted. Numbers are not recomputed. When two columns share a company,
merge actions absorb the thinner capture into the keeper (same-company API
guard). The job detail page lists attached source URLs and which listing is
canonical, and offers **Split off** when there are two or more listings.
Same-company title matches appear as **Possible duplicates** with Compare and
a recommended Merge — nothing is folded automatically. After a merge whose
salary strings disagreed, **Extraction conflicts** offers Use A / Use B /
Keep current; the chosen wording becomes provenance `manual`.

The job page **Your call** block sets pipeline status (applied /
interviewing / …), posting status, a 0–5 rating, notes, and archive.
Stars and pipeline label also show on the list. Kanban is not wired.

**Job detail** is the center of the product. Left: structured facts (comp, location, mode,
dates, seniority, blockers) each with a subtle provenance dot — hover shows "from Greenhouse
API, 0.98" or "edited by you". Center: requirement table, one row per requirement, with
kind, necessity, years, your verdict chip, and the evidence that produced it; clicking a row
highlights the originating sentence in the description. Right: match summary with per-subscore
bars, top strengths, top gaps, and the actions (Tailor resume · Draft cover letter · Track
application · Refresh · Archive).

**Job list** is a dense virtualized table: title, company, comp, location/mode, posted age,
close-date urgency, match overall plus the diagnostic Skills / Years split, status. Facet
rail on the left with counts. Column set and saved views are user-configurable. Bulk select
for tag/archive/rescore. Skills and Years do not change overall; they exist so a 5-year
wishlist can be compared across rows without opening each job. Search (`?q=`) and sort
(`?sort=overall|skills|years`) live in the URL; sort reorders the loaded page only.
Checkboxes select 2–4 rows for `/jobs/compare?ids=`.

**Experience bank editor** is a two-pane outline: roles on the left, accomplishments on the
right with inline metric fields, skill chips with autocomplete, strength stars, and a
"generate variants" action. A persistent nudge bar shows the finite backlog: "12
accomplishments missing a metric · 4 demanded skills with no evidence."

**Gaps** ranks skills by leverage, each expanding to the jobs it would unlock and a suggested
action with an effort estimate.

## UX principles

1. **Never block on the pipeline.** Ingestion returns instantly; rows render in a
   "processing" state and fill in live.
2. **Show confidence, quietly.** A dot, not a paragraph. Low-confidence fields get a subtle
   amber tint and a one-click "correct this".
3. **Every number is drillable.** A match score expands to subscores, which expand to
   requirements, which expand to evidence, which link to the accomplishment.
4. **Keyboard-first.** `⌘K` palette, `j/k` list navigation, `g j`/`g p` route jumps, `e`
   edit, `/` search, `a` add-by-URL. This is a tool used dozens of times a day.
5. **Optimistic where it is safe** (rating, tags, kanban drag) with rollback on error.
6. **Empty states teach.** The empty job list explains how to install the extension; the
   empty experience bank offers resume import.
7. **Destructive actions are reversible** — soft delete with undo, not confirmation dialogs.

## Performance

- Route-level code splitting; the initial bundle is shell + dashboard only (<200 KB gz,
  NFR-P-06). Recharts, the Markdown editor, and the PDF viewer are lazy chunks.
- Virtualized job list and requirement tables.
- `staleTime` tuned per resource (jobs 30 s, taxonomy 1 h, meta 1 h); SSE handles freshness,
  so aggressive caching costs nothing.
- Cursor pagination with infinite scroll and a prefetch of the next page on idle.
- Assets are content-hashed and served with `Cache-Control: immutable`, precompressed
  brotli; `index.html` is `no-cache`.
- No web fonts by default (system font stack) — one fewer render-blocking round trip.

## Build & integration

```
apps/web/
  src/
    routes/            file-based routes
    components/        ui/ (primitives), job/, match/, experience/, layout/
    api/               generated.ts (from OpenAPI), client.ts, queries/, events.ts
    lib/               format.ts (money/date/relative), keys.ts, shortcuts.ts
    styles/
```

- `just gen-client` runs the server, fetches `/openapi.json`, writes
  `src/api/generated.ts`. CI fails if the committed file is stale — the contract cannot
  silently drift.
- Dev: `vite dev` on 5173 proxying `/api` → `127.0.0.1:8787`. Same-origin in production, so
  no CORS in the normal path.
- Prod: `vite build` → `apps/web/dist`, embedded into the binary via `rust-embed` under the
  `embed-web` feature. `cargo build --release --features embed-web` yields one artifact
  containing the whole app.
