# Vision

## One sentence

Jobseeker is a self-hosted, single-user job-search workbench: you hand it a link to a job
posting and it turns that posting into structured, queryable, durable data — then uses your
own experience bank to tell you how well you fit, what you are missing, and to draft the
application materials.

## The problem

Job searching produces a large volume of unstructured, ephemeral, and deliberately
non-portable information:

- Postings live behind logins (LinkedIn, Indeed), get edited without notice, and disappear
  when filled. Nothing you saved is yours.
- The same role is cross-posted to five sites with five different salary bands, five
  different "posted" dates, and one real ATS application form.
- Requirements are written as prose blobs that mix hard requirements, wish-list items,
  legal boilerplate, and marketing. Comparing 40 postings by hand is not feasible.
- Your own history (accomplishments, metrics, technologies, dates) lives scattered across
  old resume `.docx` files, and gets re-derived from scratch for every application.
- Tracking (applied when, to whom, with which resume version, next follow-up) degrades into
  a spreadsheet that nobody maintains.

## The thesis

All of the above becomes tractable if two things are true:

1. **Every posting is normalized into the same schema**, with requirements broken into
   atomic, typed, individually addressable records.
2. **Your history is normalized into the same vocabulary** — an *experience bank* of
   accomplishments tagged with the same skill taxonomy the postings are tagged with.

Once both sides share a vocabulary, the interesting features stop being AI parlor tricks and
become ordinary joins and set operations: coverage of required skills, gap ranking across
your whole pipeline, bullet selection for a tailored resume, "which 3 skills would unlock
the most postings I have saved."

## Principles

1. **Local-first and self-hosted.** Runs as one binary plus one SQLite file on your home
   server. No cloud dependency is required for core function. Your job search is sensitive
   data; it should not leave your network unless you decide it does.
2. **Files and database, not files or database.** Every job is materialized as a
   human-readable directory of files (Markdown + JSON + raw capture) *and* indexed in
   SQLite. The database is the query engine and can always be rebuilt from files; the files
   are the durable, portable, greppable, git-committable artifact. See
   [ADR-0003](adr/0003-files-and-db.md).
3. **Deterministic first, LLM second.** Structured sources (schema.org `JobPosting`
   JSON-LD, ATS JSON APIs) are used before heuristics, heuristics before an LLM. Every
   extracted field records *how* it was obtained and with what confidence. LLM output is
   schema-validated, cached, and always overridable by hand.
4. **Explainable, never a black-box score.** A match score is a sum of named components,
   each traceable to a specific requirement and a specific piece of your evidence. "78%" is
   useless; "meets 11/13 required, missing Kubernetes and a security clearance" is not.
5. **You are the browser.** Authenticated sites are captured through a browser extension
   acting on pages *you* are already viewing while logged in. Jobseeker never stores your
   LinkedIn or Indeed password, and does not crawl those sites in the background. See
   [ADR-0004](adr/0004-extension-capture.md) and
   [Security, Privacy & Legal](13-security-privacy-legal.md).
6. **Correction is a first-class action.** Extraction will be wrong. Every field is
   editable, an edit is recorded as provenance `manual`, and re-extraction never silently
   clobbers a human correction.
7. **Fast enough to be pleasant.** Saving a job should feel instant (capture is
   acknowledged immediately, enrichment happens in the background). Listing and filtering
   thousands of jobs should be sub-50ms. See [Performance](14-performance.md).

## What success looks like

Concretely, after the roadmap in [16-roadmap.md](16-roadmap.md) is complete:

```
$ jobseeker add https://www.linkedin.com/jobs/view/4123456789
  queued capture 01J...  → extracting → done (1.8s)

  Senior Platform Engineer · Acme Robotics · Remote (US)
  $185k–$225k/yr · posted 3d ago · closes in 25d · apply via Greenhouse

  Match 0.81 for profile "default"
    required   11/13 met   ✗ Kubernetes (3+ yrs)   ✗ Go (production)
    preferred   4/7  met
    seniority   fit        comp  above target   location  fit

  Next: `jobseeker resume --job 01J... --template ats` (est. 12 bullets from 4 roles)
```

...and the same thing in a web UI on your LAN, with a pipeline board, a gap-driven learning
plan aggregated across every posting you have saved, and a diffable version history of every
resume you sent.

## Explicit non-goals

- **Not a job board or aggregator.** Jobseeker does not discover jobs for you by crawling
  the internet at scale. It deepens the jobs you point it at. (Opt-in polling of *public*
  ATS boards you name is in scope; background scraping of LinkedIn is not.)
- **Not multi-tenant SaaS.** Single user, optionally a couple of trusted accounts on a
  home LAN. Data model has `profile` for multiple personas (e.g. "IC engineer" vs
  "eng manager"), not for multiple customers.
- **Not an auto-applier.** No bulk one-click blasting of applications. It gets you *ready*
  to apply; a human presses submit. This is both an ethics stance and a quality stance.
- **Not a resume-lie machine.** Generation is strictly *selection and rephrasing* of
  accomplishments that exist in your experience bank, with a provenance link from every
  generated bullet back to the source accomplishment.
