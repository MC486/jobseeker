# Feature catalog

Features are grouped by capability area and tagged with the milestone that delivers them
(see [Roadmap](16-roadmap.md)). `M0` = walking skeleton, `M1` = usable daily driver,
`M2` = matching, `M3` = generation, `M4` = polish/automation.

Legend: **[M0]** foundation · **[M1]** core · **[M2]** matching · **[M3]** generation ·
**[M4]** later

---

## A. Acquisition — getting a posting into the system

| # | Feature | Milestone | Notes |
|---|---|---|---|
| A1 | Ingest by URL (public pages) | M0 | Direct HTTP fetch, robots-aware, for company sites and public ATS pages. |
| A2 | Ingest via browser extension capture | M1 | User clicks "Save to Jobseeker" on a page they are logged into; extension POSTs the rendered DOM. Primary path for LinkedIn/Indeed. |
| A3 | Ingest by paste | M0 | Paste raw text/HTML plus optional URL. Universal fallback, also the test fixture path. |
| A4 | Native ATS API ingestion | M1 | Greenhouse, Lever, Ashby, SmartRecruiters, Workable, Recruitee expose public JSON per-board. Highest-fidelity source; no scraping. |
| A5 | Re-capture / refresh a listing | M1 | Detects edits (content hash), records a new revision, flags material changes (salary changed, closed). |
| A6 | Closed/removed detection | M1 | Refresh returning 404/410/"no longer accepting" transitions status to `closed`. |
| A7 | Cross-post deduplication | M1 | Many `job_source_listing` rows → one canonical `job`. Match on ATS id, then normalized (company, title, location) + description similarity. |
| A8 | Public board watch (opt-in) | M4 | Poll named public ATS boards on a cadence for new postings. Never LinkedIn/Indeed. |
| A9 | Bulk import from file | M4 | CSV/JSON of URLs, or a directory of saved HTML. |
| A10 | Email forward ingestion | M4 | Local IMAP mailbox scrape of job alert emails → extract links → queue. |
| A11 | Screenshot capture | M2 | Extension attaches a full-page PNG stored alongside the raw HTML, so a removed posting is still visually verifiable. |
| A12 | Headless browser fetch with persistent profile | M4 | Opt-in, local-only; reuses a browser profile you log into once. Off by default, documented as ToS-sensitive. |

## B. Extraction & normalization

| # | Feature | Milestone | Notes |
|---|---|---|---|
| B1 | schema.org `JobPosting` JSON-LD / microdata extraction | M0 | Deterministic, present on a surprising share of postings. |
| B2 | Site adapters | M1 | Per-source CSS/JSON extractors: LinkedIn, Indeed, Greenhouse, Lever, Ashby, Workday, SmartRecruiters, Dice, ZipRecruiter, Glassdoor. |
| B3 | Boilerplate stripping → clean Markdown | M0 | Nav/footer/cookie-banner removal; description stored as normalized Markdown. |
| B4 | LLM structured extraction | M1 | JSON-schema-constrained; fills only fields deterministic passes left empty (or all fields, in `llm_first` mode, for comparison). |
| B5 | Requirement atomization | M1 | Description → list of typed requirement records (see C). The single most valuable transform in the product. |
| B6 | Salary normalization | M0 | "$120K–$150K DOE", "£70,000 per annum", "$60/hr", "120-150k" → min/max cents + currency + period + `is_estimate`. |
| B7 | Location & work-mode normalization | M0 | Multi-location postings, "Remote (US)", "Hybrid — 3 days in SF", timezone requirements, relocation. |
| B8 | Date normalization | M0 | "Posted 3 days ago", "Reposted", ISO dates, `validThrough` → absolute UTC instants with a `precision` marker. |
| B9 | Seniority & employment-type inference | M1 | From title + requirement years + explicit fields. |
| B10 | Company resolution & enrichment | M1 | Fold "Acme, Inc." / "Acme Robotics" / "acme" into one `company`; capture careers URL, size, industry. |
| B11 | Extraction provenance & confidence | M0 | Every field records source (`jsonld`/`adapter`/`rules`/`llm`/`manual`) + confidence. Rendered in the UI as a subtle badge. |
| B12 | Manual override & re-extract safety | M1 | Human edits are sticky across refresh/re-extract. |
| B13 | Extraction eval harness | M2 | Fixture corpus of saved HTML + hand-labeled expected output; scores precision/recall per field per adapter. Prevents prompt/adapter regressions. |
| B14 | Benefits, visa/clearance, travel flags | M1 | Structured booleans/enums for the things that silently disqualify you. |

## C. Structured requirements

| # | Feature | Milestone | Notes |
|---|---|---|---|
| C1 | Typed requirements | M1 | `skill`, `experience`, `education`, `certification`, `clearance`, `language`, `soft_skill`, `domain`, `tool`, `responsibility`, `logistics`. |
| C2 | Necessity classification | M1 | `required` / `preferred` / `nice_to_have` / `implied`. Distinguishes "must have" from wish list. |
| C3 | Quantity extraction | M1 | "5+ years", "3–5 years", "at least two shipped products" → numeric min/max. |
| C4 | Skill taxonomy with aliases | M1 | Canonical skills + alias table ("k8s"→Kubernetes, "postgres"→PostgreSQL, "GCP"→Google Cloud Platform). Hierarchy (React → JavaScript → Programming Language). |
| C5 | Requirement dedup within a job | M1 | The same demand stated in the summary and again in the bullet list collapses to one record. |
| C6 | Cross-job requirement frequency | M2 | "Kubernetes appears in 31 of your 44 saved jobs" — drives the learning plan. |
| C7 | Requirement editing in UI | M1 | Add/remove/retype/reclassify; feeds the eval corpus as labeled data. |

## D. Storage & interoperability

| # | Feature | Milestone | Notes |
|---|---|---|---|
| D1 | Per-job file directory | M0 | `job.md` (YAML frontmatter + Markdown), `job.json` (full structured record), `raw/` (original capture), `requirements.json`. |
| D2 | SQLite index | M0 | Full relational schema + FTS5 full-text search. Single file, WAL. |
| D3 | Rebuild DB from files | M1 | `jobseeker reconcile --from-files`. Guarantees files are a real backup, not a nicety. |
| D4 | Export DB to files | M0 | `jobseeker export`; also automatic on every write. |
| D5 | Git-friendly layout | M1 | Stable paths, deterministic serialization (sorted keys, LF), so the data directory can itself be a git repo. |
| D6 | DuckDB analytics attach | M4 | Read-only analytical queries over the SQLite file (or exported Parquet) for market analysis; no second write path. See [ADR-0002](adr/0002-sqlite-primary.md). |
| D7 | JSON / CSV / Parquet export | M4 | For ad-hoc analysis and portability. |
| D8 | Backup & restore | M1 | `VACUUM INTO` snapshot + files tarball; documented restore drill. |
| D9 | Attachment store | M2 | Screenshots, PDFs of postings, offer letters — content-addressed on disk. |

## E. Profile / experience bank

| # | Feature | Milestone | Notes |
|---|---|---|---|
| E1 | Structured experience items | M2 | Roles, projects, education, certifications, awards, publications, OSS, volunteer. |
| E2 | Accomplishment atoms | M2 | Bullet-level records with impact metric/value/unit, tagged with skills, user-rated strength. The raw material for resume generation. |
| E3 | Bullet variants | M3 | Short/long/leadership-framed/IC-framed phrasings of the same accomplishment, so tailoring is selection not invention. |
| E4 | Self-asserted skills with recency | M2 | Years, level, last-used year — recency matters to matchers and to honest self-assessment. |
| E5 | Resume import to bootstrap the bank | M2 | Parse an existing PDF/DOCX/Markdown resume into experience items + accomplishments for review. |
| E6 | Multiple profiles/personas | M2 | "Staff engineer" vs "eng manager" targeting from one experience bank. |
| E7 | Evidence links | M2 | Accomplishment → repo/PR/publication/metric source, so claims are defensible in interviews. |
| E8 | Question/answer bank | M3 | Reusable answers to "why us", "salary expectations", "describe a conflict" keyed by normalized question. |

## F. Matching & analysis

| # | Feature | Milestone | Notes |
|---|---|---|---|
| F1 | Requirement-level match | M2 | Per requirement: `met` / `partial` / `gap` / `unknown` + evidence pointers. |
| F2 | Explainable overall score | M2 | Weighted components: required coverage, preferred coverage, seniority fit, comp fit, location fit, semantic similarity. Weights are user-configurable. |
| F3 | Semantic similarity via embeddings | M2 | Local embedding model by default (Ollama / fastembed); vectors stored in SQLite. |
| F4 | Gap analysis per job | M2 | Ranked list of what is missing and how far off it is. |
| F5 | Aggregate gap / learning plan | M2 | Across all saved jobs: highest-leverage skills to acquire, with suggested resources. |
| F6 | "Am I qualified?" verdict with caveats | M2 | Blunt readout: hard blockers (clearance, degree, visa) vs soft gaps. |
| F7 | Job comparison view | M3 | Side-by-side of 2–5 jobs on comp, requirements, fit, commute/work-mode. |
| F8 | Market insight over saved corpus | M4 | Salary distribution by title/location/seniority, skill co-occurrence, posting age dynamics. |
| F9 | Stale-score invalidation | M2 | Scores are keyed by an inputs hash; editing your profile or the job marks affected scores stale and re-queues them. |
| F10 | Keyword/ATS coverage check | M3 | Which literal terms from the posting do *not* appear in the generated resume — useful whether or not you believe in ATS keyword filtering. |

## G. Generation

| # | Feature | Milestone | Notes |
|---|---|---|---|
| G1 | Tailored resume generation | M3 | Selects and orders accomplishments to maximize covered required requirements under a length budget; renders via Typst → PDF. |
| G2 | Resume templates | M3 | `ats` (single column, no tables/icons), `modern`, `academic`. Typst source is user-editable. |
| G3 | Cover letter drafting | M3 | Grounded in job + selected accomplishments; refuses to assert anything not in the bank. |
| G4 | Document versioning & diff | M3 | Every generation is a `document` row with parent linkage; diff two versions. |
| G5 | Provenance for every bullet | M3 | Generated bullet → source accomplishment id + which requirement it targets. |
| G6 | Outreach / referral message drafts | M4 | To a `contact` at the company, referencing real overlap. |
| G7 | Interview prep pack | M4 | Likely questions from the requirement set, plus your strongest matching stories (STAR-shaped from accomplishments). |
| G8 | Learning plan generation | M4 | For a target job or aggregate gaps: ordered curriculum with time estimates. |

## H. Application tracking

| # | Feature | Milestone | Notes |
|---|---|---|---|
| H1 | Application records with status pipeline | M1 | `interested → preparing → applied → screening → interviewing → offer → accepted/rejected/withdrawn/ghosted`. |
| H2 | Timeline of events | M1 | Status changes, emails, calls, interviews, assessments, offers, notes. |
| H3 | Kanban board | M1 | Drag between statuses; WIP counts. |
| H4 | Next-action + due date + reminders | M1 | The "what do I do today" list. Possibly-ghosted flag after 14 days idle while waiting on the employer — does not auto-set status. |
| H5 | Contacts | M2 | Recruiters, hiring managers, referrals, linked to company/application. |
| H6 | Which resume did I send? | M3 | Application pins the exact `document` version. |
| H7 | Funnel analytics | M4 | Conversion rates by source, title, seniority; response time distributions. |
| H8 | Deadline / close-date alerts | M1 | Jobs closing soon that you have not applied to. |

## I. Platform, UX, operations

| # | Feature | Milestone | Notes |
|---|---|---|---|
| I1 | Single-binary deployment with embedded UI | M0 | One artifact, one config file, one data dir. |
| I2 | Background task queue with progress | M0 | SQLite-backed; SSE progress to the UI. No Redis, no extra services. |
| I3 | Auth suitable for a home LAN | M0 | `none` (trusted network/Tailscale) / `token` / `password+session`. |
| I4 | Config via file + env | M0 | TOML with env overrides; validated at boot with clear errors. |
| I5 | OpenAPI spec + generated TS client | M0 | Backend is the schema authority; frontend types are generated, never hand-written. |
| I6 | Full-text + faceted search UI | M1 | FTS5 across title/company/description/requirements plus structured facets. |
| I7 | Saved views | M2 | Named filter sets ("remote, >$180k, no clearance, open"). |
| I8 | Keyboard-first navigation | M2 | Command palette, `j/k` list nav, quick-add. This is a tool used daily. |
| I9 | Observability | M1 | Structured tracing, `/healthz`, `/readyz`, Prometheus `/metrics`. |
| I10 | Offline/degraded operation | M1 | No LLM configured → deterministic extraction still works and the app stays fully usable. |
| I11 | CLI parity for core flows | M0 | `add`, `list`, `show`, `refresh`, `match`, `resume`, `export`, `reconcile`, `serve`, `migrate`. |
| I12 | Dark mode, responsive, printable job sheets | M2 | |
| I13 | Extension pairing flow | M1 | Server prints a pairing token; extension options page stores server URL + token. |
| I14 | Data wipe / per-entity delete | M1 | Delete a job and its files; delete all LLM call logs; etc. |
