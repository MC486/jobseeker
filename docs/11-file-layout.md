# Data directory & file format

The database is the index; the files are the artifact. `jobseeker reconcile --from-files`
must be able to rebuild the database completely (FR-S-04), which is what makes the file tree
a real backup rather than a decorative export.

## Layout

```
$DATA_DIR/
├── jobseeker.db                        SQLite (+ -wal, -shm)
├── jobs/
│   └── acme-robotics/
│       └── 2026-09-02-senior-platform-engineer-4f3a9c21/
│           ├── job.md                  human-readable: YAML frontmatter + Markdown
│           ├── job.json                complete structured record
│           ├── requirements.json       atomized requirements
│           ├── match.json              latest score per profile (regenerable)
│           └── raw/
│               ├── 2026-09-02T18-14-02Z-linkedin.html.zst
│               ├── 2026-09-02T18-14-02Z-linkedin.meta.json
│               ├── 2026-09-02T18-14-05Z-linkedin.png
│               └── 2026-09-03T09-01-11Z-greenhouse.json.zst
├── profiles/
│   └── default/
│       ├── profile.json
│       ├── experience.json             items + accomplishments (one file: edited as a unit)
│       └── skills.json
├── documents/
│   └── 2026-09-05-acme-robotics-senior-platform-engineer-resume-v3/
│       ├── document.json               metadata, selection, coverage
│       ├── resume.typ
│       └── resume.pdf
├── applications/
│   └── acme-robotics-senior-platform-engineer-4f3a9c21.json    record + event timeline
├── media/
│   └── 7c/7c9f…e1.png                  content-addressed screenshots/attachments
├── taxonomy/
│   ├── skills.json                     including your promoted additions
│   └── aliases.json
├── backups/
│   └── 2026-09-05T03-00-00Z/           jobseeker.db snapshot + files.tar.zst
└── logs/
```

Path rule: `jobs/<company-slug>/<posted-date>-<job-slug>-<id8>/`. Date first sorts
chronologically; the 8-char id suffix guarantees uniqueness without being ugly. Slugs are
ASCII-folded, lowercased, non-alphanumerics collapsed to `-`, truncated to 60 chars.
`job.file_path` stores this so renames are detectable.

## `job.md`

The file you would actually read, or grep, or open on your phone.

```markdown
---
id: 4f3a9c21-8e2d-7b1a-9c44-0d1e2f3a4b5c
title: Senior Platform Engineer
company: Acme Robotics
status: open
work_mode: remote
work_mode_detail: Remote within the US
employment_type: full_time
seniority: senior
locations:
  - { city: San Francisco, region: CA, country: US, primary: true }
  - { remote_scope: true, country: US }
salary: { min: 185000, max: 225000, currency: USD, period: year, is_estimate: false }
posted_at: 2026-09-02          # precision: day
closes_at: 2026-10-15
apply_url: https://boards.greenhouse.io/acmerobotics/jobs/5512034
sources:
  - { site: greenhouse, canonical: true, url: "https://boards.greenhouse.io/…" }
  - { site: linkedin,  canonical: false, url: "https://www.linkedin.com/jobs/view/4123456789" }
requirements_summary: { required: 13, preferred: 7 }
match: { default: 0.81 }
tags: [rust, infra, remote]
user_rating: 4
extraction: { model: "qwen2.5:14b-instruct", confidence: 0.91, partial: false }
first_seen_at: 2026-09-02T18:14:02Z
content_hash: b3:9f2c…
---

# Senior Platform Engineer — Acme Robotics

## Summary
Own the event ingestion and deployment platform for a fleet of warehouse robots…

## Required
- 5+ years building distributed systems in a systems language  *(skill: distributed-systems, 5y)*
- Production Rust or C++  *(skill: rust)*
- 3+ years operating Kubernetes  *(skill: kubernetes, 3y)*

## Preferred
- Experience with robotics or real-time systems  *(domain: robotics)*

## Responsibilities
- …

## Benefits
- …

## Notes (yours)
Referred by Sam. Team seems small — ask about on-call rotation.

---
<!-- source: greenhouse api · captured 2026-09-02T18:14:02Z -->
```

Frontmatter is the queryable projection; the body is readable prose. Both are generated from
the database, and both are parseable back (the `*(skill: …)*` annotations round-trip
requirement links).

## `job.json`

The lossless record: every column, every provenance entry, every location, listing,
capture reference, revision, and conflict. Serialization rules that keep git diffs meaningful
(FR-S-03):

- Keys sorted lexicographically at every level.
- Two-space indent, LF endings, trailing newline.
- `null` fields omitted; empty arrays omitted.
- No generation timestamp inside the file — a re-export of unchanged data produces a
  byte-identical file, so `git status` stays clean and a spurious diff means something
  actually changed.

## `requirements.json`

```json
[
  { "id": "…", "ordinal": 1, "kind": "skill", "necessity": "required",
    "text": "5+ years building distributed systems in a systems language",
    "normalized_text": "years building distributed systems systems language",
    "skill": {"id": "…", "slug": "distributed-systems", "name": "Distributed Systems"},
    "min_years": 5.0, "level": "proficient", "is_blocker": false,
    "source_span": [412, 471], "confidence": 0.93, "provenance": "llm" }
]
```

## Raw captures

`raw/<iso8601-compact>-<source>.{html,json}.zst` with a sibling `.meta.json`
(url, canonical url, method, status, headers subset, content hash, user agent, client
version). Zstd level 10 gives roughly 8–12× on job-page HTML.

Bodies are also hard-linked into `media/<hh>/<hash>` so identical captures across jobs cost
one copy on disk. If the filesystem does not support hard links, the copy is made — never a
symlink, because a symlinked backup that loses its target is worse than a duplicated byte.

## Atomicity & concurrency (FR-S-05)

Every write is `write to <file>.tmp.<pid>` → `fsync` → `rename` over the target, then
`fsync` the directory. Rename is atomic on POSIX, so a reader never sees a partial file and a
crash leaves either the old or the new version. A per-job advisory lock serializes writers
for the same directory. The DB transaction commits *before* the file write; the file write is
idempotent and retried by a `materialize_job` task, so a crash between the two self-heals on
the next reconcile.

## Reconcile

```
jobseeker reconcile --to-files          # DB → files (also automatic on every job write)
jobseeker reconcile --from-files        # files → DB (rebuild; DB may be absent)
jobseeker reconcile --check             # report drift, change nothing
jobseeker reconcile --rebuild-derived   # recompute denormalized columns and FTS
```

`--from-files` walks `jobs/**/job.json`, upserts companies/jobs/locations/listings/
requirements by id, re-registers captures from `raw/`, then rebuilds FTS and derived columns.
`match.json` and embeddings are *not* restored — they are regenerable, and rebuilding them is
cheaper than trusting a stale copy.

`--check` compares `content_hash` per job and reports `db_only`, `file_only`, and
`divergent`, which is exactly the drill you want before trusting a backup.

## Git-friendliness (D5)

The data directory can itself be a git repo. Recommended `.gitignore` inside `$DATA_DIR`:

```
jobseeker.db*
backups/
logs/
media/
jobs/**/raw/          # large; omit unless you want full provenance in history
```

Committing `jobs/**/job.md`, `job.json`, `requirements.json`, `profiles/`, and `documents/`
gives a readable, diffable history of your entire search: salary changes on a posting show up
as a one-line diff.

## Portability

Nothing in the tree depends on absolute paths, the machine, or the SQLite version. Copy
`$DATA_DIR` to another host, run `jobseeker reconcile --from-files`, and you are whole. That
property is the acceptance test for the whole storage design (AS-06).
