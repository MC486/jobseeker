# Data model

The authoritative definition is `crates/db/migrations/*.sql`. This document explains the
*shape and reasoning*; when they disagree, the migrations win and this file is a bug.

## 0. Conventions

- **Ids**: `TEXT PRIMARY KEY`, UUIDv7 lowercase dashed. Time-ordered, so B-tree inserts stay
  at the right edge and `ORDER BY id` approximates creation order.
- **Timestamps**: `TEXT NOT NULL` RFC3339 UTC (`2026-09-05T20:21:00Z`). Lexicographically
  sortable, readable in the exported files, comparable in SQL. Where the source is vague, a
  sibling `*_precision` column holds `exact|hour|day|month|unknown`.
- **Money**: integer minor units (`_cents`) plus an ISO-4217 `currency`. Never floats.
- **Enums**: `TEXT` with a `CHECK` constraint. Readable in the files and in `sqlite3`, and
  the check catches typos. Rust side is a real enum with `FromStr`/`Display`.
- **Booleans**: `INTEGER NOT NULL DEFAULT 0` with `CHECK (x IN (0,1))`.
- **JSON**: `TEXT` columns named `*_json`, validated with `CHECK (json_valid(...))`. Used
  only for genuinely open-ended data (raw provider payloads, explanation blobs, link lists).
- **Provenance**: any field that extraction can fill has a companion row in
  `field_provenance` (see §2.4) rather than doubling every column. Cheap and general.
- **Soft delete**: `deleted_at` on user-facing entities so `reconcile` cannot resurrect
  something you deleted; hard delete available via CLI.
- **Foreign keys**: always declared, `ON DELETE CASCADE` for owned children,
  `ON DELETE SET NULL` for references.

## 1. Entity map

```
                    ┌───────────┐
                    │  company  │◄─────────────┐
                    └─────┬─────┘              │
                          │ 1:N               │ N:1
                    ┌─────▼─────┐        ┌────┴──────┐
        ┌──────────►│    job    │◄───────│  contact  │
        │           └─────┬─────┘        └───────────┘
        │      1:N ┌──────┼──────┬───────────┐
        │          │      │      │           │
        │  ┌───────▼──┐ ┌─▼──────────┐ ┌─────▼──────────────┐
        │  │requirement│ │job_location│ │job_source_listing  │──1:N──► capture
        │  └───┬──────┘ └────────────┘ └────────────────────┘
        │      │ N:1
        │  ┌───▼───┐   N:M    ┌──────────────┐
        │  │ skill │◄─────────│ skill_alias  │
        │  └───┬───┘          └──────────────┘
        │      │ N:M
        │  ┌───▼──────────────┐        ┌──────────┐
        │  │accomplishment_skill│──N:1─│profile   │
        │  │  profile_skill     │      └────┬─────┘
        │  └───┬────────────────┘           │ 1:N
        │      │ N:1                  ┌─────▼──────────┐
        │  ┌───▼──────────┐    1:N    │experience_item │
        │  │accomplishment│◄──────────┴────────────────┘
        │  └──────────────┘
        │
        │   ┌──────────────┐  1:N   ┌──────────────────┐
        └───│ match_score  │───────►│ requirement_match│
            └──────────────┘        └──────────────────┘

  ┌────────────┐ 1:N ┌───────────────────┐     ┌───────────┐   ┌──────────┐
  │application │────►│ application_event │     │ document  │   │   task   │
  └────────────┘     └───────────────────┘     └───────────┘   └──────────┘

  support: user, session, device_token, setting, embedding, llm_call,
           event_log, tag, job_tag, saved_view, skill_gap, question_answer,
           board_watch, field_provenance, extraction_conflict, job_revision
```

## 2. Table-by-table

### 2.1 Companies and sources

**`company`** — one row per real-world employer.

| column | type | notes |
|---|---|---|
| `id` | TEXT PK | |
| `name` | TEXT NOT NULL | display name as most commonly seen |
| `slug` | TEXT NOT NULL UNIQUE | `acme-robotics`; used in file paths |
| `name_normalized` | TEXT NOT NULL | lowercased, suffixes (`inc`, `llc`, `gmbh`) stripped; **indexed**, used for resolution |
| `website`, `careers_url`, `linkedin_url` | TEXT | |
| `industry` | TEXT | |
| `size_bucket` | TEXT CHECK | `1-10 … 10001+`, `unknown` |
| `hq_location` | TEXT | |
| `funding_stage`, `is_public` | TEXT / INTEGER | |
| `notes_md` | TEXT | your own research |
| `rating` | INTEGER | your interest 0–5 |
| `created_at`, `updated_at`, `deleted_at` | TEXT | |

**`source`** — a channel a posting came from. Seeded, not user-created.

`id`, `kind` (`linkedin|indeed|glassdoor|ziprecruiter|dice|greenhouse|lever|ashby|workday|smartrecruiters|workable|recruitee|company_site|manual|email|other`),
`name`, `base_url`, `fidelity` INTEGER (higher = more trustworthy; used to pick the canonical
listing during dedupe: ATS APIs 90, ATS HTML 70, aggregators 40, manual 100).

### 2.2 Jobs

**`job`** — the canonical posting. One row no matter how many sites cross-posted it.

| column | type | notes |
|---|---|---|
| `id` | TEXT PK | |
| `company_id` | TEXT FK → company | |
| `slug` | TEXT NOT NULL | `senior-platform-engineer` |
| `title` | TEXT NOT NULL | as posted |
| `title_normalized` | TEXT NOT NULL | lowercase, seniority/level tokens factored out; used for dedupe + grouping |
| `seniority` | TEXT CHECK | `intern|entry|junior|mid|senior|staff|principal|lead|manager|director|vp|exec|unknown` |
| `employment_type` | TEXT CHECK | `full_time|part_time|contract|contract_to_hire|internship|temporary|volunteer|unknown` |
| `work_mode` | TEXT CHECK | `remote|hybrid|onsite|unknown` |
| `work_mode_detail` | TEXT | "3 days/week in SF", "remote within EU" |
| `department`, `team` | TEXT | |
| `description_md` | TEXT | normalized Markdown, boilerplate stripped |
| `description_text` | TEXT | plain text projection, used for FTS + embeddings |
| `summary` | TEXT | 2–3 sentence generated summary |
| `responsibilities_md`, `benefits_md`, `about_company_md` | TEXT | split when the source distinguishes them |
| `salary_min_cents`, `salary_max_cents` | INTEGER | |
| `salary_currency` | TEXT | ISO 4217 |
| `salary_period` | TEXT CHECK | `year|month|week|day|hour|project|unknown` |
| `salary_is_estimate` | INTEGER | true when the aggregator guessed it (Indeed/Glassdoor estimates) |
| `salary_raw` | TEXT | verbatim, always kept |
| `equity_offered`, `bonus_note`, `comp_notes` | INTEGER / TEXT | |
| `posted_at`, `posted_at_precision` | TEXT | |
| `updated_at_source` | TEXT | source's own last-modified |
| `closes_at`, `closes_at_precision` | TEXT | `validThrough` or "applications close" |
| `first_seen_at`, `last_seen_at` | TEXT NOT NULL | ours |
| `closed_at` | TEXT | when we observed closure |
| `status` | TEXT CHECK | `open|closed|filled|expired|removed|unknown` |
| `apply_url` | TEXT | the real application entry point |
| `apply_kind` | TEXT CHECK | `ats|external|email|easy_apply|unknown` |
| `canonical_listing_id` | TEXT FK → job_source_listing | the highest-fidelity source |
| `requires_clearance` | TEXT | e.g. `secret`, `ts_sci`, null |
| `visa_sponsorship` | TEXT CHECK | `yes|no|unspecified` |
| `travel_pct` | INTEGER | |
| `education_min` | TEXT CHECK | `none|hs|associate|bachelor|master|doctorate|unknown` |
| `years_experience_min`, `years_experience_max` | REAL | headline requirement, if stated |
| `headcount` | INTEGER | "hiring 3" |
| `content_hash` | TEXT NOT NULL | BLAKE3 over the normalized record; change detection |
| `extraction_model`, `extracted_at` | TEXT | |
| `extraction_confidence` | REAL | mean field confidence |
| `extraction_partial` | INTEGER | true when the LLM stage was skipped/failed |
| `file_path` | TEXT | data-dir-relative directory |
| `user_rating` | INTEGER | interest 0–5 |
| `user_notes_md` | TEXT | |
| `is_archived` | INTEGER | |
| `created_at`, `updated_at`, `deleted_at` | TEXT | |

Indexes: `(company_id)`, `(status, posted_at DESC)`, `(work_mode)`, `(seniority)`,
`(salary_max_cents)`, `(closes_at)` partial `WHERE status='open'`,
`(title_normalized, company_id)` for dedupe, `(is_archived, updated_at DESC)`.

**`job_location`** — a posting can be genuinely multi-city.

`id`, `job_id` FK, `raw`, `city`, `region`, `country` (ISO-3166-1 alpha-2), `postal_code`,
`lat`/`lon` REAL, `is_primary` INTEGER, `is_remote_scope` INTEGER (row describes a remote
eligibility region, e.g. "Remote — US"), `timezone_requirement`, `ordinal`.

**`job_source_listing`** — the posting as seen at one URL. This is the dedupe join point.

`id`, `job_id` FK, `source_id` FK, `source_job_id` (site's own id), `url`, `url_canonical`,
`url_hash` UNIQUE (BLAKE3 of canonical URL — the natural idempotency key), `title_at_source`,
`company_name_at_source`, `posted_at`, `salary_raw`, `status`, `is_canonical`,
`first_seen_at`, `last_seen_at`, `last_checked_at`, `check_failures` INTEGER,
`created_at`, `updated_at`.

**`capture`** — immutable raw acquisition. Never mutated, only added.

`id`, `listing_id` FK nullable, `url`, `method` (`http|extension|browser|api|paste|file`),
`http_status`, `content_type`, `content_encoding`, `byte_len`, `content_hash` (BLAKE3),
`storage_path` (relative, e.g. `captures/2f/2fa9…html.zst`), `screenshot_path`,
`captured_at`, `user_agent`, `client_version`, `notes`, `extract_status`
(`pending|ok|failed`), `extract_error`.

Raw bodies are zstd-compressed on disk and content-addressed, so five captures of the same
unchanged page cost one blob.

**`job_revision`** — what changed when a refresh finds an edit.

`id`, `job_id`, `capture_id`, `changed_at`, `diff_json` (field → {before, after}),
`is_material` INTEGER (salary/status/closes_at/title changed), `summary`.

**`field_provenance`** — one row per (entity, field) telling you where the value came from.

`id`, `entity_kind` (`job|requirement|company|job_location`), `entity_id`, `field`,
`provenance` (`manual|api|jsonld|microdata|adapter|rules|llm|inferred`), `confidence` REAL,
`model`, `capture_id`, `updated_at`. UNIQUE `(entity_kind, entity_id, field)`.

This is what makes FR-E-03 ("manual edits are sticky") a one-line check instead of a
sprawling special case, and it powers the confidence badges in the UI.

**`extraction_conflict`** — sources disagreed.

`id`, `job_id`, `field`, `value_a`, `provenance_a`, `listing_a_id`, `value_b`,
`provenance_b`, `listing_b_id`, `resolved_value`, `resolution` (`auto_precedence|manual|unresolved`), `created_at`.
Merge writes `unresolved` when salary raw strings differ (listing source in
`provenance_*`). `POST /conflicts/:id/resolve` sets `manual` and
`resolved_value`; `auto_precedence` is reserved for Stage 1–2 and is not
written today.

### 2.3 Requirements and skills

**`requirement`** — the atomized demand. The heart of the product.

| column | type | notes |
|---|---|---|
| `id` | TEXT PK | |
| `job_id` | TEXT FK CASCADE | |
| `ordinal` | INTEGER | display order |
| `text` | TEXT NOT NULL | verbatim from the posting |
| `normalized_text` | TEXT NOT NULL | cleaned, deduped comparison key |
| `kind` | TEXT CHECK | `skill|tool|experience|education|certification|clearance|language|soft_skill|domain|responsibility|logistics|other` |
| `necessity` | TEXT CHECK | `required|preferred|nice_to_have|implied` |
| `skill_id` | TEXT FK → skill | null when it is not a taxonomy skill |
| `min_years`, `max_years` | REAL | |
| `level` | TEXT CHECK | `exposure|working|proficient|expert` |
| `education_level` | TEXT CHECK | as `job.education_min` |
| `field_of_study` | TEXT | |
| `is_blocker` | INTEGER | hard disqualifier (clearance, license, work authorization) |
| `quantity_raw` | TEXT | "5+ years", "at least two" |
| `source_span` | TEXT | char offsets into `description_md` for UI highlighting |
| `confidence` | REAL | |
| `provenance` | TEXT CHECK | as field_provenance |
| `created_at`, `updated_at` | TEXT | |

Indexes: `(job_id, ordinal)`, `(skill_id)`, `(kind, necessity)`, `(job_id, normalized_text)` UNIQUE (in-job dedupe, C5).

**`skill`** — taxonomy node.

`id`, `name`, `slug` UNIQUE, `kind` (`language|framework|library|tool|platform|database|concept|domain|methodology|soft|certification|language_human`),
`parent_id` FK → skill (hierarchy: React → JavaScript → Programming Language),
`description`, `external_ids_json` (ESCO/O*NET/LinkedIn ids for future interop),
`usage_count` INTEGER (denormalized frequency across jobs, refreshed by a task),
`created_at`.

**`skill_alias`** — `id`, `skill_id` FK, `alias`, `alias_normalized` UNIQUE, `kind`
(`abbrev|synonym|misspelling|vendor`), `is_ambiguous` INTEGER.

Ambiguity matters: "Go" is a language and a word; "R" likewise. Ambiguous aliases require
contextual confirmation before linking.

**`skill_gap`** — actionable gap, per profile.

`id`, `profile_id`, `skill_id`, `job_id` nullable (null = aggregate),
`severity` REAL, `frequency` INTEGER (jobs demanding it), `blocking_count` INTEGER
(jobs where it is `required`), `years_short` REAL, `suggested_action`,
`resources_json`, `status` (`open|learning|done|dismissed`), `target_date`,
`created_at`, `updated_at`.

### 2.4 Profile and experience bank

**`profile`** — a persona. `id`, `name` ("Staff SWE"), `full_name`, `headline`, `email`,
`phone`, `location`, `links_json`, `summary_md`, `target_titles_json`,
`target_comp_min_cents`, `target_locations_json`, `work_auth`, `willing_to_relocate`,
`is_default`, `created_at`, `updated_at`.

**`experience_item`** — `id`, `profile_id` FK, `kind`
(`role|project|education|certification|award|publication|oss|volunteer|course`),
`org`, `org_normalized`, `title`, `location`, `work_mode`, `employment_type`,
`start_date`, `end_date`, `is_current`, `description_md`, `url`,
`ordinal`, `visibility` (`public|private`), `created_at`, `updated_at`.

Dates are `TEXT` in `YYYY-MM` or `YYYY-MM-DD` form — resumes rarely have day precision.

**`accomplishment`** — the atom resume generation selects from.

| column | type | notes |
|---|---|---|
| `id` | TEXT PK | |
| `experience_item_id` | TEXT FK CASCADE | |
| `text` | TEXT NOT NULL | canonical bullet |
| `variants_json` | TEXT | `{"short":"…","leadership":"…","ic":"…"}` |
| `situation`, `action`, `result` | TEXT | STAR decomposition for interview prep |
| `impact_metric` | TEXT | "p95 latency", "ARR", "deploy frequency" |
| `impact_value` | REAL | |
| `impact_unit` | TEXT | `ms`, `%`, `usd`, `x`, `count` |
| `impact_direction` | TEXT CHECK | `increase|decrease|maintain` |
| `strength` | INTEGER | your 1–5 rating of how strong this bullet is |
| `verified` | INTEGER | you can defend it with evidence |
| `evidence_url`, `evidence_note` | TEXT | |
| `scope_json` | TEXT | team size, budget, users affected |
| `ordinal`, `created_at`, `updated_at` | | |

**`accomplishment_skill`** — `accomplishment_id`, `skill_id`, `weight` REAL,
`is_primary`; PK `(accomplishment_id, skill_id)`.

**`profile_skill`** — `id`, `profile_id`, `skill_id`, `years` REAL, `level`,
`last_used_year` INTEGER, `is_primary`, `self_rating` INTEGER, `notes`;
UNIQUE `(profile_id, skill_id)`.

### 2.5 Matching

**`match_score`** — one row per (job, profile, algorithm_version).

`id`, `job_id` FK, `profile_id` FK, `algorithm_version` TEXT,
`overall` REAL, `required_coverage` REAL, `preferred_coverage` REAL,
`seniority_fit` REAL, `comp_fit` REAL, `location_fit` REAL, `semantic_similarity` REAL,
`blocker_count` INTEGER, `blockers_json`, `weights_json` (the weights actually used),
`explanation_json`, `inputs_hash` TEXT (hash of job content_hash + profile revision +
weights + algorithm_version), `is_stale` INTEGER, `computed_at`, `duration_ms`.

UNIQUE `(job_id, profile_id, algorithm_version)`. `is_stale` lets a cheap UPDATE invalidate
thousands of scores instantly (FR-F-09) without deleting the last-known values.

**`requirement_match`** — `id`, `match_score_id` FK CASCADE, `requirement_id` FK,
`status` (`met|partial|gap|unknown`), `score` REAL, `weight` REAL,
`evidence_json` (`[{kind:"accomplishment",id:"…",similarity:0.83}, …]`),
`years_have` REAL, `years_needed` REAL, `rationale`, `created_at`.

### 2.6 Applications and documents

**`application`** — `id`, `job_id` FK, `profile_id` FK, `status` CHECK
(`interested|preparing|applied|screening|interviewing|offer|accepted|rejected|withdrawn|ghosted`),
`applied_at`, `applied_via`, `resume_document_id` FK → document,
`cover_letter_document_id` FK → document, `referral_contact_id` FK → contact,
`salary_asked_cents`, `salary_offered_cents`, `offer_details_json`,
`next_action`, `next_action_due`, `priority` INTEGER, `rejection_reason`, `rejection_stage`,
`notes_md`, `last_activity_at`, `created_at`, `updated_at`.
UNIQUE `(job_id, profile_id)`.

**`application_event`** — append-only. `id`, `application_id` FK CASCADE, `kind`
(`status_change|email_sent|email_received|call|interview|assessment|offer|note|reminder|document_sent`),
`occurred_at`, `title`, `body_md`, `from_status`, `to_status`, `contact_id`,
`metadata_json`, `created_at`.

**`contact`** — `id`, `company_id` FK, `name`, `title`, `email`, `phone`, `linkedin_url`,
`relationship` (`recruiter|hiring_manager|referral|peer|interviewer|other`), `notes_md`,
`last_contacted_at`, `created_at`, `updated_at`.

**`document`** — every generated artifact, immutably versioned.

`id`, `profile_id` FK, `job_id` FK nullable, `application_id` FK nullable,
`kind` (`resume|cover_letter|outreach|answer|interview_prep|learning_plan`),
`title`, `template`, `format` (`typst|markdown|html|text`), `source_content` TEXT,
`render_path` (PDF on disk), `render_format`, `page_count`,
`version` INTEGER, `parent_document_id` FK → document,
`generated_by_model`, `prompt_hash`, `selection_json` (which accomplishments, in what order,
targeting which requirements — FR-G-05), `coverage_json` (which required requirements
remain uncovered), `is_sent` INTEGER, `created_at`.

**`question_answer`** — `id`, `profile_id`, `question`, `question_normalized`,
`answer_md`, `job_id` nullable, `tags_json`, `use_count`, `created_at`, `updated_at`.

### 2.7 Platform tables

**`task`** — the queue. `id`, `kind`, `payload_json`, `status`
(`queued|running|done|failed|cancelled`), `priority` INTEGER DEFAULT 0,
`attempts`, `max_attempts`, `last_error`, `progress` REAL, `progress_message`,
`dedupe_key` TEXT UNIQUE (nullable — coalesces "extract job X" enqueued five times),
`available_at`, `lease_expires_at`, `started_at`, `finished_at`,
`parent_task_id`, `trace_id`, `created_at`, `updated_at`.

Index: `(status, available_at, priority DESC)` — the claim query's covering index.

**`llm_call`** — cache + audit + cost. `id`, `provider`, `model`, `purpose`
(`extract_job|atomize_requirements|summarize|match_rationale|resume_bullets|cover_letter|embed`),
`prompt_hash` (BLAKE3 of provider+model+prompt+schema+params), `request_json`,
`response_json`, `response_valid` INTEGER, `prompt_tokens`, `completion_tokens`,
`cost_micros` INTEGER, `latency_ms`, `error`, `trace_id`, `created_at`.
UNIQUE `(prompt_hash)` — this *is* the cache (FR-E-10).

**`embedding`** — polymorphic vectors. `id`, `owner_kind`
(`job|requirement|accomplishment|skill|document|profile`), `owner_id`, `model`,
`dim` INTEGER, `vector` BLOB (little-endian f32), `content_hash`, `created_at`.
UNIQUE `(owner_kind, owner_id, model)`.

Brute-force cosine over ≤10⁵ vectors in Rust is ~milliseconds, so no ANN index is needed at
this scale; `sqlite-vec` is an optional drop-in later (see [ADR-0002](adr/0002-sqlite-primary.md)).

**`user`** — `id`, `username` UNIQUE, `password_hash` (Argon2id), `role`
(`owner|viewer`), `created_at`, `last_login_at`, `disabled`.

**`session`** — `id` (BLAKE3 of the bearer token — the plaintext is never stored),
`user_id` FK, `created_at`, `expires_at`, `last_seen_at`, `user_agent`, `ip`, `revoked_at`.

**`device_token`** — extension pairing. `id`, `token_hash`, `name` ("Firefox on laptop"),
`scopes_json` (`["ingest"]`), `created_at`, `last_used_at`, `expires_at`, `revoked_at`.

**`event_log`** — durable domain events for SSE backfill. `id`, `kind`, `entity_kind`,
`entity_id`, `payload_json`, `created_at`. Pruned by the scheduler after N days.

**`setting`** — `key` PK, `value_json`, `updated_at`. Runtime-tunable things (weights,
feature flags) that shouldn't require an app restart.

**`tag`** / **`job_tag`** — `tag(id, name UNIQUE, color, kind)`;
`job_tag(job_id, tag_id, created_at)` PK both.

**`saved_view`** — `id`, `name`, `entity` (`job|application`), `query_json`, `is_pinned`,
`ordinal`, `created_at`.

**`board_watch`** — opt-in public board polling. `id`, `source_id`, `name`,
`config_json` (board slug, filters), `cadence_minutes`, `last_run_at`, `last_result_json`,
`enabled`, `created_at`.

## 3. Full-text search

FTS5 external-content tables, kept in sync by triggers so there is exactly one copy of the
data:

```sql
CREATE VIRTUAL TABLE job_fts USING fts5(
  title, company_name, description_text, requirements_text,
  content='',                 -- contentless: we store only the index
  tokenize = "porter unicode61 remove_diacritics 2"
);
```

`job_fts` is contentless with `rowid` mapped to a stable integer surrogate
(`job.fts_rowid INTEGER UNIQUE`), because FTS5 external-content requires an INTEGER rowid
and our PKs are TEXT. `requirements_text` is a materialized concatenation refreshed when
requirements change — search results should hit on "Kubernetes" even when the word only
appears in an atomized requirement.

Ranking uses `bm25(job_fts, 10.0, 6.0, 1.0, 3.0)` (title and company weighted up), and the
API composes FTS with structured filters in one query:

```sql
SELECT j.* FROM job_fts f
JOIN job j ON j.fts_rowid = f.rowid
WHERE job_fts MATCH ?1
  AND j.deleted_at IS NULL AND j.status = 'open'
  AND (?2 IS NULL OR j.work_mode = ?2)
  AND (?3 IS NULL OR j.salary_max_cents >= ?3)
ORDER BY bm25(job_fts, 10.0, 6.0, 1.0, 3.0)
LIMIT ?4;
```

A second FTS table `accomplishment_fts(text, org, title)` powers experience-bank search and
the "find my evidence for X" flow.

## 4. Derived / denormalized fields

Denormalization is used only where it removes a hot join, and every such field is rebuildable
by `jobseeker reconcile --rebuild-derived`:

| field | source of truth | refreshed by |
|---|---|---|
| `job.content_hash` | the job's normalized record | write path |
| `job.description_text` | `description_md` | write path |
| `job.requirements_text` (FTS input) | `requirement` rows | trigger + task |
| `skill.usage_count` | `requirement` | nightly task |
| `application.last_activity_at` | `application_event` | trigger |
| `match_score.is_stale` | job/profile/weights changes | triggers + task |
| `profile_skill.evidence_count` | `accomplishment_skill` | trigger |

## 5. Migration policy

- Forward-only, numbered, checksummed (`sqlx::migrate!`). Never edit a shipped migration.
- Pre-1.0: additive changes preferred; destructive changes must ship a data-moving migration
  and a note in `docs/16-roadmap.md`.
- Startup applies migrations (configurable) and **refuses to start** if the database reports
  a schema version newer than the binary knows (NFR-O-06).
- Every migration gets a test that runs it against both an empty DB and a seeded DB.

## 6. Sizing sanity check

For a heavy 12-month search: 5,000 jobs × ~8 KB normalized + 60,000 requirements ×
~200 B + 15,000 captures × ~120 KB compressed raw.

- SQLite: ~80 MB (jobs, requirements, matches, FTS index).
- Files: ~2 GB dominated by raw captures and screenshots.
- Embeddings: 70,000 × 768 × 4 B ≈ 215 MB if everything is embedded; restricted to jobs +
  accomplishments it is ~20 MB.

Well inside "one file on a home server," which is the whole point of
[ADR-0002](adr/0002-sqlite-primary.md).
