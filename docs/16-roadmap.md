# Roadmap

Milestones are ordered by dependency, not by calendar. Each has an exit criterion that is
demonstrable, so "done" is not a judgment call.

---

## M0 — Walking skeleton ✅ *scaffolded in this repo*

**Goal:** the whole spine exists and one job can travel it end to end.

- [x] Workspace, crate boundaries, config loading, tracing, error taxonomy
- [x] SQLite schema + migrations + FTS + pragmas + dual pools
- [x] Task queue with leases, backoff, dedupe keys
- [x] axum server: health/ready/meta, error envelope, OpenAPI, SPA serving
- [x] Ingest endpoints (url/paste/capture) → durable capture → queued task
- [x] Web app shell, generated API client, SSE plumbing
- [x] Extension skeleton with pairing
- [x] CLI skeleton (`serve`, `migrate`, `add`, `list`, `show`)
- [ ] JSON-LD extractor + salary/date/location normalizers, wired end to end
- [ ] File materialization (`job.md` / `job.json` / `requirements.json` / `raw/`)

**Exit:** `jobseeker add <greenhouse-url>` produces a queryable job row, four files on disk,
and a rendered job detail page. AS-01 passes.

---

## M1 — Usable daily driver

**Goal:** replace the spreadsheet. Everything except matching and generation.

**Acquisition & extraction**
- Site adapters: LinkedIn, Indeed, Greenhouse, Lever, Ashby, Workday
- ATS JSON clients for Greenhouse, Lever, Ashby, SmartRecruiters, Workable
- LLM provider abstraction + Ollama and OpenAI providers, response cache
- Requirement atomization (rules + LLM), skill taxonomy seed (~600 skills + aliases)
- Refresh, closure detection, `job_revision` diffs, cross-post dedupe
- Provenance table, conflict surfacing, sticky manual edits

**Storage & interop**
- `reconcile --from-files` / `--to-files` / `--check`
- `backup` + restore drill documented and exercised

**UI**
- Job list with FTS + facets, virtualized
- Job detail: overview / requirements / description / sources tabs
- Application tracking: kanban, timeline, next actions, closing-soon
- Task inspector, settings, extension pairing flow
- Browser extension shipped and working on LinkedIn + Indeed

**Ops**
- Prometheus metrics, systemd unit, container image

**Exit:** 100 real jobs ingested across ≥4 sources; extraction eval ≥0.85 on core fields;
`reconcile --check` clean after a DB rebuild; you stop keeping a separate spreadsheet.
AS-01 through AS-06 and AS-10 pass.

---

## M2 — Matching

**Goal:** the app tells you where you stand and what to fix.

- Experience bank: experience items, accomplishments with metrics/scope/STAR, skills with
  recency, resume import to bootstrap, review queue
- Embeddings: Ollama + fastembed providers, `embedding` table, cosine search
- Requirement-level verdicts with evidence pointers
- Subscores, weighted overall, blocker cap, explanation payload
- Staleness invalidation + background rescore
- Aggregate gap analysis and leverage-ranked learning plan
- Match UI: subscore bars, strengths/gaps, evidence drill-down, `min_match` filter and sort
- Screenshot capture, saved views, keyboard navigation, dark mode
- Extraction eval harness in CI with a committed baseline

**Exit:** a populated experience bank scores every saved job; every `met` verdict cites real
evidence; the gap list names the three skills with the highest leverage across your pipeline.
AS-07 and AS-09 pass.

---

## M3 — Generation

**Goal:** hand it a link, get application-ready.

- Accomplishment selection as weighted set cover under a length budget
- LLM rephrasing with no-new-facts validation (numbers and technologies verified)
- Typst templates (`ats`, `modern`, `academic`) + PDF rendering
- Cover letter generation grounded in job + selected evidence
- Document versioning, bullet-level diff, application pins a version
- Keyword/ATS coverage report
- Answer bank with similarity retrieval
- Job comparison view

**Exit:** for a real posting, a generated one-page resume whose every bullet traces to a bank
record, reporting which required requirements remain uncovered — and it is genuinely the
resume you would send. AS-08 passes.

---

## M4 — Automation & insight

**Goal:** the search runs itself where automation is legitimate.

- Public ATS board watching on a cadence (never authenticated sites)
- Email-alert ingestion via a local IMAP mailbox
- Interview prep packs, outreach drafts, learning-plan generation
- Market analytics: salary distributions, skill co-occurrence, posting dynamics
- Funnel analytics: conversion by source/title/seniority, response-time distributions
- DuckDB analytics attachment, Parquet export
- Optional headless-browser acquisition (opt-in, documented)
- PDF posting ingestion; bulk import
- Mobile-friendly layout; printable job sheets

**Exit:** new postings from your watched boards appear scored each morning without
intervention, and the analytics answer "what is the market actually paying for my profile."

---

## Backlog (unscheduled, deliberately)

Worth doing eventually; not worth designing for now.

- Multi-profile A/B testing of resume variants against response rates
- Commute/geo scoring with real travel times
- Company research aggregation (funding, Glassdoor sentiment, layoff history)
- Salary negotiation support with market comparables
- Referral graph from your contact list
- Calendar integration for interview scheduling
- Local fine-tune of a small model on your own corrections
- Mobile app (the responsive web UI should make this unnecessary)
- Multi-user sharing (explicitly against the vision — listed to record the decision)

## Sequencing rules

1. **M1 before M2.** Matching against badly extracted requirements produces confident
   nonsense. Extraction quality is the foundation everything else stands on.
2. **The experience bank gates M3.** Generation without structured accomplishments becomes an
   LLM writing fiction, which the whole design exists to prevent.
3. **Eval harness before prompt iteration.** Without a baseline, prompt changes are vibes.
4. **The file format is a contract from M1.** Changing it later requires a migration path
   (CON-07), so get `job.json` right before real data accumulates.
5. **Every milestone ships usable.** No milestone leaves the app in a state where you would
   go back to the spreadsheet.
