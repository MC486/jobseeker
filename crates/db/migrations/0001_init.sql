-- Jobseeker initial schema.
--
-- Conventions (see docs/04-data-model.md):
--   * ids            TEXT, UUIDv7, lowercase-dashed
--   * timestamps     TEXT, RFC3339 UTC ("2026-09-05T20:21:00Z") — sortable as text
--   * money          INTEGER minor units + ISO-4217 code; never floats
--   * enums          TEXT + CHECK; readable in sqlite3 and in the exported files
--   * booleans       INTEGER 0/1 + CHECK
--   * json           TEXT columns named *_json, guarded by json_valid()
--   * soft delete    deleted_at, so `reconcile --from-files` cannot resurrect a deletion
--
-- Migrations are forward-only and never edited after release.

-- ======================================================================================
-- Sources and companies
-- ======================================================================================

CREATE TABLE source (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL UNIQUE CHECK (kind IN (
                    'linkedin','indeed','glassdoor','ziprecruiter','dice','greenhouse',
                    'lever','ashby','workday','smartrecruiters','workable','recruitee',
                    'company_site','manual','email','other')),
    name        TEXT NOT NULL,
    base_url    TEXT,
    -- Higher wins when two listings describe the same job: an employer's own ATS is more
    -- trustworthy than an aggregator's reconstruction of it.
    fidelity    INTEGER NOT NULL DEFAULT 50 CHECK (fidelity BETWEEN 0 AND 100),
    created_at  TEXT NOT NULL
) STRICT;

CREATE TABLE company (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    slug            TEXT NOT NULL UNIQUE,
    -- Legal suffixes stripped + lowercased: the key that folds "Acme, Inc." into "Acme".
    name_normalized TEXT NOT NULL,
    website         TEXT,
    careers_url     TEXT,
    linkedin_url    TEXT,
    industry        TEXT,
    size_bucket     TEXT CHECK (size_bucket IS NULL OR size_bucket IN (
                        '1-10','11-50','51-200','201-500','501-1000','1001-5000',
                        '5001-10000','10001+','unknown')),
    hq_location     TEXT,
    funding_stage   TEXT,
    is_public       INTEGER CHECK (is_public IS NULL OR is_public IN (0,1)),
    notes_md        TEXT,
    rating          INTEGER CHECK (rating IS NULL OR rating BETWEEN 0 AND 5),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    deleted_at      TEXT
) STRICT;

CREATE INDEX idx_company_normalized ON company(name_normalized) WHERE deleted_at IS NULL;

-- ======================================================================================
-- Jobs
-- ======================================================================================

CREATE TABLE job (
    id                      TEXT PRIMARY KEY,
    company_id              TEXT NOT NULL REFERENCES company(id) ON DELETE RESTRICT,
    slug                    TEXT NOT NULL,
    title                   TEXT NOT NULL,
    -- Seniority/level tokens factored out; the cross-post detection key.
    title_normalized        TEXT NOT NULL,

    seniority               TEXT NOT NULL DEFAULT 'unknown' CHECK (seniority IN (
                                'intern','entry','junior','mid','senior','staff','principal',
                                'lead','manager','director','vp','exec','unknown')),
    employment_type         TEXT NOT NULL DEFAULT 'unknown' CHECK (employment_type IN (
                                'full_time','part_time','contract','contract_to_hire',
                                'internship','temporary','volunteer','unknown')),
    work_mode               TEXT NOT NULL DEFAULT 'unknown' CHECK (work_mode IN (
                                'remote','hybrid','onsite','unknown')),
    work_mode_detail        TEXT,
    department              TEXT,
    team                    TEXT,

    description_md          TEXT NOT NULL DEFAULT '',
    -- Plain-text projection; feeds FTS and embeddings.
    description_text        TEXT NOT NULL DEFAULT '',
    summary                 TEXT,
    responsibilities_md     TEXT,
    benefits_md             TEXT,
    about_company_md        TEXT,

    salary_min_cents        INTEGER,
    salary_max_cents        INTEGER,
    salary_currency         TEXT,
    salary_period           TEXT NOT NULL DEFAULT 'unknown' CHECK (salary_period IN (
                                'year','month','week','day','hour','project','unknown')),
    -- True when the figure is the *site's* estimate, not the employer's posted band.
    salary_is_estimate      INTEGER NOT NULL DEFAULT 0 CHECK (salary_is_estimate IN (0,1)),
    salary_raw              TEXT,
    equity_offered          INTEGER NOT NULL DEFAULT 0 CHECK (equity_offered IN (0,1)),
    comp_notes              TEXT,

    posted_at               TEXT,
    posted_at_precision     TEXT CHECK (posted_at_precision IS NULL OR posted_at_precision IN (
                                'exact','hour','day','month','at_least','unknown')),
    updated_at_source       TEXT,
    closes_at               TEXT,
    closes_at_precision     TEXT CHECK (closes_at_precision IS NULL OR closes_at_precision IN (
                                'exact','hour','day','month','at_least','unknown')),
    first_seen_at           TEXT NOT NULL,
    last_seen_at            TEXT NOT NULL,
    closed_at               TEXT,
    status                  TEXT NOT NULL DEFAULT 'open' CHECK (status IN (
                                'open','closed','filled','expired','removed','unknown')),

    apply_url               TEXT,
    apply_kind              TEXT NOT NULL DEFAULT 'unknown' CHECK (apply_kind IN (
                                'ats','external','email','easy_apply','unknown')),
    canonical_listing_id    TEXT,

    requires_clearance      TEXT,
    visa_sponsorship        TEXT NOT NULL DEFAULT 'unspecified' CHECK (visa_sponsorship IN (
                                'yes','no','unspecified')),
    travel_pct              INTEGER CHECK (travel_pct IS NULL OR travel_pct BETWEEN 0 AND 100),
    education_min           TEXT NOT NULL DEFAULT 'unknown' CHECK (education_min IN (
                                'none','hs','associate','bachelor','master','doctorate','unknown')),
    years_experience_min    REAL,
    years_experience_max    REAL,
    headcount               INTEGER,

    -- BLAKE3 over the normalized record: change detection and score staleness.
    content_hash            TEXT NOT NULL,
    extraction_model        TEXT,
    extracted_at            TEXT,
    extraction_confidence   REAL,
    -- True when the model stage was skipped or failed: deterministic stages only.
    extraction_partial      INTEGER NOT NULL DEFAULT 0 CHECK (extraction_partial IN (0,1)),

    file_path               TEXT,
    user_rating             INTEGER CHECK (user_rating IS NULL OR user_rating BETWEEN 0 AND 5),
    user_notes_md           TEXT,
    is_archived             INTEGER NOT NULL DEFAULT 0 CHECK (is_archived IN (0,1)),

    -- Integer surrogate for FTS5, which requires an INTEGER rowid while our PKs are TEXT.
    fts_rowid               INTEGER,

    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    deleted_at              TEXT,

    CHECK (salary_min_cents IS NULL OR salary_max_cents IS NULL
           OR salary_min_cents <= salary_max_cents)
) STRICT;

CREATE UNIQUE INDEX idx_job_fts_rowid ON job(fts_rowid) WHERE fts_rowid IS NOT NULL;
CREATE INDEX idx_job_status_posted   ON job(status, posted_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX idx_job_company         ON job(company_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_job_closing         ON job(closes_at) WHERE status = 'open' AND deleted_at IS NULL;
CREATE INDEX idx_job_dedupe          ON job(company_id, title_normalized) WHERE deleted_at IS NULL;
CREATE INDEX idx_job_salary          ON job(salary_max_cents) WHERE salary_max_cents IS NOT NULL;
CREATE INDEX idx_job_updated         ON job(is_archived, updated_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX idx_job_work_mode       ON job(work_mode) WHERE deleted_at IS NULL;

CREATE TABLE job_location (
    id                      TEXT PRIMARY KEY,
    job_id                  TEXT NOT NULL REFERENCES job(id) ON DELETE CASCADE,
    -- Verbatim source string; never discarded, even when parsing succeeded.
    raw                     TEXT NOT NULL,
    city                    TEXT,
    region                  TEXT,
    country                 TEXT,
    postal_code             TEXT,
    lat                     REAL,
    lon                     REAL,
    is_primary              INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0,1)),
    -- 1 = "where you may live while remote", not "where an office is".
    is_remote_scope         INTEGER NOT NULL DEFAULT 0 CHECK (is_remote_scope IN (0,1)),
    timezone_requirement    TEXT,
    ordinal                 INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX idx_job_location_job     ON job_location(job_id);
CREATE INDEX idx_job_location_country ON job_location(country, region, city);

-- One posting as seen at one URL. The dedupe join point: many listings -> one job.
CREATE TABLE job_source_listing (
    id                      TEXT PRIMARY KEY,
    job_id                  TEXT REFERENCES job(id) ON DELETE CASCADE,
    source_id               TEXT NOT NULL REFERENCES source(id) ON DELETE RESTRICT,
    source_job_id           TEXT,
    url                     TEXT NOT NULL,
    url_canonical           TEXT NOT NULL,
    -- BLAKE3 of the canonical URL: the natural idempotency key for re-ingestion.
    url_hash                TEXT NOT NULL UNIQUE,
    title_at_source         TEXT,
    company_name_at_source  TEXT,
    posted_at               TEXT,
    salary_raw              TEXT,
    status                  TEXT NOT NULL DEFAULT 'open' CHECK (status IN (
                                'open','closed','filled','expired','removed','unknown')),
    is_canonical            INTEGER NOT NULL DEFAULT 0 CHECK (is_canonical IN (0,1)),
    first_seen_at           TEXT NOT NULL,
    last_seen_at            TEXT NOT NULL,
    last_checked_at         TEXT,
    check_failures          INTEGER NOT NULL DEFAULT 0,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
) STRICT;

CREATE INDEX idx_listing_job     ON job_source_listing(job_id);
CREATE INDEX idx_listing_source  ON job_source_listing(source_id, source_job_id);
CREATE INDEX idx_listing_recheck ON job_source_listing(status, last_checked_at);

-- Immutable raw acquisition. Never mutated, only added (FR-A-03).
CREATE TABLE capture (
    id              TEXT PRIMARY KEY,
    listing_id      TEXT REFERENCES job_source_listing(id) ON DELETE SET NULL,
    url             TEXT,
    method          TEXT NOT NULL CHECK (method IN ('http','extension','browser','api','paste','file')),
    http_status     INTEGER,
    content_type    TEXT,
    content_encoding TEXT,
    byte_len        INTEGER NOT NULL DEFAULT 0,
    -- Content-addressed: five captures of an unchanged page cost one blob on disk.
    content_hash    TEXT NOT NULL,
    storage_path    TEXT NOT NULL,
    screenshot_path TEXT,
    captured_at     TEXT NOT NULL,
    user_agent      TEXT,
    client_version  TEXT,
    notes           TEXT,
    extract_status  TEXT NOT NULL DEFAULT 'pending' CHECK (extract_status IN ('pending','ok','failed')),
    extract_error   TEXT
) STRICT;

CREATE INDEX idx_capture_listing ON capture(listing_id, captured_at DESC);
CREATE INDEX idx_capture_hash    ON capture(content_hash);
CREATE INDEX idx_capture_pending ON capture(extract_status) WHERE extract_status = 'pending';

-- What changed when a refresh found an edit.
CREATE TABLE job_revision (
    id          TEXT PRIMARY KEY,
    job_id      TEXT NOT NULL REFERENCES job(id) ON DELETE CASCADE,
    capture_id  TEXT REFERENCES capture(id) ON DELETE SET NULL,
    changed_at  TEXT NOT NULL,
    diff_json   TEXT NOT NULL CHECK (json_valid(diff_json)),
    -- Salary/title/status/closes_at moved: worth telling the user about.
    is_material INTEGER NOT NULL DEFAULT 0 CHECK (is_material IN (0,1)),
    summary     TEXT
) STRICT;

CREATE INDEX idx_job_revision_job ON job_revision(job_id, changed_at DESC);

-- One row per (entity, field): where the value came from, so a human edit is never
-- clobbered by automation (FR-E-03) and the UI can show confidence.
CREATE TABLE field_provenance (
    id          TEXT PRIMARY KEY,
    entity_kind TEXT NOT NULL CHECK (entity_kind IN ('job','requirement','company','job_location')),
    entity_id   TEXT NOT NULL,
    field       TEXT NOT NULL,
    provenance  TEXT NOT NULL CHECK (provenance IN (
                    'manual','api','jsonld','microdata','adapter','llm','rules','inferred')),
    confidence  REAL NOT NULL DEFAULT 0.5 CHECK (confidence BETWEEN 0 AND 1),
    model       TEXT,
    capture_id  TEXT REFERENCES capture(id) ON DELETE SET NULL,
    updated_at  TEXT NOT NULL,
    UNIQUE (entity_kind, entity_id, field)
) STRICT;

CREATE INDEX idx_provenance_manual ON field_provenance(entity_kind, entity_id)
    WHERE provenance = 'manual';

-- Two sources disagreed. Surfaced in the UI rather than silently resolved, because the gap
-- between an aggregator's estimate and an ATS's posted band is useful information.
CREATE TABLE extraction_conflict (
    id              TEXT PRIMARY KEY,
    job_id          TEXT NOT NULL REFERENCES job(id) ON DELETE CASCADE,
    field           TEXT NOT NULL,
    value_a         TEXT,
    provenance_a    TEXT,
    listing_a_id    TEXT REFERENCES job_source_listing(id) ON DELETE SET NULL,
    value_b         TEXT,
    provenance_b    TEXT,
    listing_b_id    TEXT REFERENCES job_source_listing(id) ON DELETE SET NULL,
    resolved_value  TEXT,
    resolution      TEXT NOT NULL DEFAULT 'unresolved' CHECK (resolution IN (
                        'auto_precedence','manual','unresolved')),
    created_at      TEXT NOT NULL
) STRICT;

CREATE INDEX idx_conflict_job ON extraction_conflict(job_id);

-- ======================================================================================
-- Skills taxonomy
-- ======================================================================================

CREATE TABLE skill (
    id                  TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    slug                TEXT NOT NULL UNIQUE,
    kind                TEXT NOT NULL CHECK (kind IN (
                            'language','framework','library','tool','platform','database',
                            'concept','domain','methodology','soft','certification',
                            'language_human')),
    -- Hierarchy: React -> JavaScript -> Programming Language. Evidence propagates up only.
    parent_id           TEXT REFERENCES skill(id) ON DELETE SET NULL,
    description         TEXT,
    external_ids_json   TEXT CHECK (external_ids_json IS NULL OR json_valid(external_ids_json)),
    usage_count         INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT NOT NULL
) STRICT;

CREATE INDEX idx_skill_parent ON skill(parent_id);
CREATE INDEX idx_skill_usage  ON skill(usage_count DESC);

CREATE TABLE skill_alias (
    id                  TEXT PRIMARY KEY,
    skill_id            TEXT NOT NULL REFERENCES skill(id) ON DELETE CASCADE,
    alias               TEXT NOT NULL,
    alias_normalized    TEXT NOT NULL UNIQUE,
    kind                TEXT NOT NULL DEFAULT 'synonym' CHECK (kind IN (
                            'abbrev','synonym','misspelling','vendor')),
    -- "Go" and "R" are words as well as languages: ambiguous aliases need corroboration
    -- before they are linked.
    is_ambiguous        INTEGER NOT NULL DEFAULT 0 CHECK (is_ambiguous IN (0,1))
) STRICT;

CREATE INDEX idx_alias_skill ON skill_alias(skill_id);

-- Unrecognized skill phrases, queued for promotion rather than dropped, so the taxonomy
-- grows to fit the user's market instead of a generic one.
CREATE TABLE skill_candidate (
    normalized_text         TEXT PRIMARY KEY,
    display_text            TEXT NOT NULL,
    occurrences             INTEGER NOT NULL DEFAULT 1,
    example_requirement_id  TEXT,
    status                  TEXT NOT NULL DEFAULT 'pending' CHECK (status IN (
                                'pending','promoted','dismissed')),
    first_seen_at           TEXT NOT NULL,
    last_seen_at            TEXT NOT NULL
) STRICT;

CREATE INDEX idx_skill_candidate_pending ON skill_candidate(occurrences DESC)
    WHERE status = 'pending';

-- ======================================================================================
-- Requirements — the atomized demands the whole product is built on
-- ======================================================================================

CREATE TABLE requirement (
    id              TEXT PRIMARY KEY,
    job_id          TEXT NOT NULL REFERENCES job(id) ON DELETE CASCADE,
    ordinal         INTEGER NOT NULL DEFAULT 0,

    -- Verbatim, so the UI can quote it and the user can judge it.
    text            TEXT NOT NULL,
    -- Lowercased/stopworded comparison key; also the in-job dedup key.
    normalized_text TEXT NOT NULL,

    kind            TEXT NOT NULL CHECK (kind IN (
                        'skill','tool','experience','education','certification','clearance',
                        'language','soft_skill','domain','responsibility','logistics','other')),
    necessity       TEXT NOT NULL CHECK (necessity IN (
                        'required','preferred','nice_to_have','implied')),
    skill_id        TEXT REFERENCES skill(id) ON DELETE SET NULL,

    min_years       REAL,
    max_years       REAL,
    level           TEXT CHECK (level IS NULL OR level IN ('exposure','working','proficient','expert')),
    education_level TEXT CHECK (education_level IS NULL OR education_level IN (
                        'none','hs','associate','bachelor','master','doctorate','unknown')),
    field_of_study  TEXT,

    -- Categorical disqualifier (clearance, licence, work authorization) rather than a
    -- graded gap. Caps the overall match score.
    is_blocker      INTEGER NOT NULL DEFAULT 0 CHECK (is_blocker IN (0,1)),
    quantity_raw    TEXT,
    -- Char offsets into job.description_md, for highlighting the origin sentence.
    span_start      INTEGER,
    span_end        INTEGER,

    confidence      REAL NOT NULL DEFAULT 0.5 CHECK (confidence BETWEEN 0 AND 1),
    provenance      TEXT NOT NULL DEFAULT 'rules' CHECK (provenance IN (
                        'manual','api','jsonld','microdata','adapter','llm','rules','inferred')),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,

    -- Postings repeat the same demand in the summary and again in the bullet list.
    UNIQUE (job_id, normalized_text)
) STRICT;

CREATE INDEX idx_req_job             ON requirement(job_id, ordinal);
CREATE INDEX idx_req_skill_necessity ON requirement(skill_id, necessity);
CREATE INDEX idx_req_kind            ON requirement(kind, necessity);
CREATE INDEX idx_req_blocker         ON requirement(job_id) WHERE is_blocker = 1;

-- ======================================================================================
-- Profiles and the experience bank
-- ======================================================================================

CREATE TABLE profile (
    id                      TEXT PRIMARY KEY,
    name                    TEXT NOT NULL,
    full_name               TEXT,
    headline                TEXT,
    email                   TEXT,
    phone                   TEXT,
    location                TEXT,
    links_json              TEXT CHECK (links_json IS NULL OR json_valid(links_json)),
    summary_md              TEXT,
    target_titles_json      TEXT CHECK (target_titles_json IS NULL OR json_valid(target_titles_json)),
    target_comp_min_cents   INTEGER,
    target_locations_json   TEXT CHECK (target_locations_json IS NULL OR json_valid(target_locations_json)),
    work_auth               TEXT,
    willing_to_relocate     INTEGER NOT NULL DEFAULT 0 CHECK (willing_to_relocate IN (0,1)),
    accepts_remote          INTEGER NOT NULL DEFAULT 1 CHECK (accepts_remote IN (0,1)),
    is_default              INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0,1)),
    -- Bumped on any edit to the profile or its children; part of a score's inputs_hash, so
    -- an edit invalidates exactly the affected scores.
    revision                INTEGER NOT NULL DEFAULT 1,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    deleted_at              TEXT
) STRICT;

CREATE UNIQUE INDEX idx_profile_default ON profile(is_default) WHERE is_default = 1;

CREATE TABLE experience_item (
    id              TEXT PRIMARY KEY,
    profile_id      TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL CHECK (kind IN (
                        'role','project','education','certification','award','publication',
                        'oss','volunteer','course')),
    org             TEXT NOT NULL,
    org_normalized  TEXT NOT NULL,
    title           TEXT,
    location        TEXT,
    work_mode       TEXT CHECK (work_mode IS NULL OR work_mode IN ('remote','hybrid','onsite','unknown')),
    employment_type TEXT CHECK (employment_type IS NULL OR employment_type IN (
                        'full_time','part_time','contract','contract_to_hire','internship',
                        'temporary','volunteer','unknown')),
    -- 'YYYY-MM' or 'YYYY-MM-DD': resumes rarely have day precision.
    start_date      TEXT,
    end_date        TEXT,
    is_current      INTEGER NOT NULL DEFAULT 0 CHECK (is_current IN (0,1)),
    description_md  TEXT,
    url             TEXT,
    ordinal         INTEGER NOT NULL DEFAULT 0,
    visibility      TEXT NOT NULL DEFAULT 'public' CHECK (visibility IN ('public','private')),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
) STRICT;

CREATE INDEX idx_experience_profile ON experience_item(profile_id, ordinal);

-- The atom resume generation selects from, and the evidence matching cites.
CREATE TABLE accomplishment (
    id                      TEXT PRIMARY KEY,
    experience_item_id      TEXT NOT NULL REFERENCES experience_item(id) ON DELETE CASCADE,
    text                    TEXT NOT NULL,
    -- {"short": "...", "leadership": "...", "ic": "..."} so tailoring is selection, not
    -- invention.
    variants_json           TEXT CHECK (variants_json IS NULL OR json_valid(variants_json)),
    situation               TEXT,
    action                  TEXT,
    result                  TEXT,
    impact_metric           TEXT,
    impact_value            REAL,
    impact_unit             TEXT,
    impact_direction        TEXT CHECK (impact_direction IS NULL OR impact_direction IN (
                                'increase','decrease','maintain')),
    strength                INTEGER NOT NULL DEFAULT 3 CHECK (strength BETWEEN 1 AND 5),
    verified                INTEGER NOT NULL DEFAULT 0 CHECK (verified IN (0,1)),
    evidence_url            TEXT,
    evidence_note           TEXT,
    scope_json              TEXT CHECK (scope_json IS NULL OR json_valid(scope_json)),
    ordinal                 INTEGER NOT NULL DEFAULT 0,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
) STRICT;

CREATE INDEX idx_accomplishment_item ON accomplishment(experience_item_id, ordinal);

CREATE TABLE accomplishment_skill (
    accomplishment_id   TEXT NOT NULL REFERENCES accomplishment(id) ON DELETE CASCADE,
    skill_id            TEXT NOT NULL REFERENCES skill(id) ON DELETE CASCADE,
    weight              REAL NOT NULL DEFAULT 1.0 CHECK (weight BETWEEN 0 AND 1),
    is_primary          INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0,1)),
    PRIMARY KEY (accomplishment_id, skill_id)
) STRICT;

CREATE INDEX idx_accomplishment_skill_skill ON accomplishment_skill(skill_id);

CREATE TABLE profile_skill (
    id              TEXT PRIMARY KEY,
    profile_id      TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
    skill_id        TEXT NOT NULL REFERENCES skill(id) ON DELETE CASCADE,
    years           REAL,
    level           TEXT CHECK (level IS NULL OR level IN ('exposure','working','proficient','expert')),
    -- A skill last used in 2016 is not the same as one used last year.
    last_used_year  INTEGER,
    is_primary      INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0,1)),
    self_rating     INTEGER CHECK (self_rating IS NULL OR self_rating BETWEEN 1 AND 5),
    -- Derived count of tagged accomplishments: the counterweight to self-assertion.
    evidence_count  INTEGER NOT NULL DEFAULT 0,
    notes           TEXT,
    UNIQUE (profile_id, skill_id)
) STRICT;

-- ======================================================================================
-- Matching
-- ======================================================================================

CREATE TABLE match_score (
    id                  TEXT PRIMARY KEY,
    job_id              TEXT NOT NULL REFERENCES job(id) ON DELETE CASCADE,
    profile_id          TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
    algorithm_version   TEXT NOT NULL,

    overall             REAL NOT NULL CHECK (overall BETWEEN 0 AND 1),
    required_coverage   REAL,
    preferred_coverage  REAL,
    seniority_fit       REAL,
    comp_fit            REAL,
    location_fit        REAL,
    semantic_similarity REAL,

    blocker_count       INTEGER NOT NULL DEFAULT 0,
    blockers_json       TEXT CHECK (blockers_json IS NULL OR json_valid(blockers_json)),
    weights_json        TEXT NOT NULL CHECK (json_valid(weights_json)),
    explanation_json    TEXT CHECK (explanation_json IS NULL OR json_valid(explanation_json)),
    narrative           TEXT,
    flags_json          TEXT CHECK (flags_json IS NULL OR json_valid(flags_json)),

    -- hash(job.content_hash, profile.revision, weights, algorithm_version). Changing an
    -- input marks the score stale instead of deleting it, so the UI can show the last known
    -- value while recomputing.
    inputs_hash         TEXT NOT NULL,
    is_stale            INTEGER NOT NULL DEFAULT 0 CHECK (is_stale IN (0,1)),
    computed_at         TEXT NOT NULL,
    duration_ms         INTEGER NOT NULL DEFAULT 0,

    UNIQUE (job_id, profile_id, algorithm_version)
) STRICT;

CREATE INDEX idx_match_profile ON match_score(profile_id, overall DESC) WHERE is_stale = 0;
CREATE INDEX idx_match_stale   ON match_score(profile_id) WHERE is_stale = 1;
CREATE INDEX idx_match_job     ON match_score(job_id);

CREATE TABLE requirement_match (
    id              TEXT PRIMARY KEY,
    match_score_id  TEXT NOT NULL REFERENCES match_score(id) ON DELETE CASCADE,
    requirement_id  TEXT NOT NULL REFERENCES requirement(id) ON DELETE CASCADE,
    status          TEXT NOT NULL CHECK (status IN ('met','partial','gap','unknown')),
    score           REAL NOT NULL DEFAULT 0 CHECK (score BETWEEN 0 AND 1),
    weight          REAL NOT NULL DEFAULT 1,
    -- [{"kind":"accomplishment","id":"...","similarity":0.83}, ...]
    evidence_json   TEXT CHECK (evidence_json IS NULL OR json_valid(evidence_json)),
    years_have      REAL,
    years_needed    REAL,
    rationale       TEXT,
    created_at      TEXT NOT NULL,
    UNIQUE (match_score_id, requirement_id)
) STRICT;

CREATE INDEX idx_reqmatch_status ON requirement_match(match_score_id, status);
CREATE INDEX idx_reqmatch_req    ON requirement_match(requirement_id, status);

CREATE TABLE skill_gap (
    id                  TEXT PRIMARY KEY,
    profile_id          TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
    skill_id            TEXT NOT NULL REFERENCES skill(id) ON DELETE CASCADE,
    -- NULL means "aggregated across the pipeline" rather than specific to one job.
    job_id              TEXT REFERENCES job(id) ON DELETE CASCADE,
    severity            REAL NOT NULL DEFAULT 0,
    frequency           INTEGER NOT NULL DEFAULT 0,
    blocking_count      INTEGER NOT NULL DEFAULT 0,
    years_short         REAL,
    -- Expected score improvement across the pipeline if closed, weighted by interest.
    -- Usually a better guide to what to learn than raw frequency.
    leverage            REAL NOT NULL DEFAULT 0,
    suggested_action    TEXT,
    resources_json      TEXT CHECK (resources_json IS NULL OR json_valid(resources_json)),
    status              TEXT NOT NULL DEFAULT 'open' CHECK (status IN (
                            'open','learning','done','dismissed')),
    target_date         TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX idx_gap_aggregate ON skill_gap(profile_id, skill_id) WHERE job_id IS NULL;
CREATE INDEX idx_gap_leverage ON skill_gap(profile_id, leverage DESC) WHERE status = 'open';

-- ======================================================================================
-- Applications, contacts, documents
-- ======================================================================================

CREATE TABLE contact (
    id                  TEXT PRIMARY KEY,
    company_id          TEXT REFERENCES company(id) ON DELETE SET NULL,
    name                TEXT NOT NULL,
    title               TEXT,
    email               TEXT,
    phone               TEXT,
    linkedin_url        TEXT,
    relationship        TEXT NOT NULL DEFAULT 'other' CHECK (relationship IN (
                            'recruiter','hiring_manager','referral','peer','interviewer','other')),
    notes_md            TEXT,
    last_contacted_at   TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
) STRICT;

CREATE INDEX idx_contact_company ON contact(company_id);

CREATE TABLE document (
    id                      TEXT PRIMARY KEY,
    profile_id              TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
    job_id                  TEXT REFERENCES job(id) ON DELETE SET NULL,
    application_id          TEXT,
    kind                    TEXT NOT NULL CHECK (kind IN (
                                'resume','cover_letter','outreach','answer','interview_prep',
                                'learning_plan')),
    title                   TEXT NOT NULL,
    template                TEXT,
    format                  TEXT NOT NULL DEFAULT 'markdown' CHECK (format IN (
                                'typst','markdown','html','text')),
    source_content          TEXT NOT NULL,
    render_path             TEXT,
    render_format           TEXT,
    page_count              INTEGER,
    version                 INTEGER NOT NULL DEFAULT 1,
    -- Immutable versions: nothing is overwritten, so "what did I send in March?" is
    -- answerable.
    parent_document_id      TEXT REFERENCES document(id) ON DELETE SET NULL,
    generated_by_model      TEXT,
    prompt_hash             TEXT,
    -- Which accomplishments, in what order, targeting which requirements (FR-G-05).
    selection_json          TEXT CHECK (selection_json IS NULL OR json_valid(selection_json)),
    coverage_json           TEXT CHECK (coverage_json IS NULL OR json_valid(coverage_json)),
    is_sent                 INTEGER NOT NULL DEFAULT 0 CHECK (is_sent IN (0,1)),
    created_at              TEXT NOT NULL
) STRICT;

CREATE INDEX idx_document_job     ON document(job_id, kind, version DESC);
CREATE INDEX idx_document_profile ON document(profile_id, created_at DESC);

CREATE TABLE application (
    id                          TEXT PRIMARY KEY,
    job_id                      TEXT NOT NULL REFERENCES job(id) ON DELETE CASCADE,
    profile_id                  TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
    status                      TEXT NOT NULL DEFAULT 'interested' CHECK (status IN (
                                    'interested','preparing','applied','screening','interviewing',
                                    'offer','accepted','rejected','withdrawn','ghosted')),
    applied_at                  TEXT,
    applied_via                 TEXT,
    resume_document_id          TEXT REFERENCES document(id) ON DELETE SET NULL,
    cover_letter_document_id    TEXT REFERENCES document(id) ON DELETE SET NULL,
    referral_contact_id         TEXT REFERENCES contact(id) ON DELETE SET NULL,
    salary_asked_cents          INTEGER,
    salary_offered_cents        INTEGER,
    offer_details_json          TEXT CHECK (offer_details_json IS NULL OR json_valid(offer_details_json)),
    next_action                 TEXT,
    next_action_due             TEXT,
    priority                    INTEGER NOT NULL DEFAULT 0,
    rejection_reason            TEXT,
    rejection_stage             TEXT,
    notes_md                    TEXT,
    last_activity_at            TEXT NOT NULL,
    created_at                  TEXT NOT NULL,
    updated_at                  TEXT NOT NULL,
    UNIQUE (job_id, profile_id)
) STRICT;

CREATE INDEX idx_application_status      ON application(status, priority DESC);
CREATE INDEX idx_application_next_action ON application(next_action_due) WHERE next_action_due IS NOT NULL;
CREATE INDEX idx_application_stale       ON application(last_activity_at);

-- Append-only timeline.
CREATE TABLE application_event (
    id              TEXT PRIMARY KEY,
    application_id  TEXT NOT NULL REFERENCES application(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL CHECK (kind IN (
                        'status_change','email_sent','email_received','call','interview',
                        'assessment','offer','note','reminder','document_sent')),
    occurred_at     TEXT NOT NULL,
    title           TEXT,
    body_md         TEXT,
    from_status     TEXT,
    to_status       TEXT,
    contact_id      TEXT REFERENCES contact(id) ON DELETE SET NULL,
    metadata_json   TEXT CHECK (metadata_json IS NULL OR json_valid(metadata_json)),
    created_at      TEXT NOT NULL
) STRICT;

CREATE INDEX idx_app_event_app ON application_event(application_id, occurred_at DESC);

CREATE TABLE question_answer (
    id                      TEXT PRIMARY KEY,
    profile_id              TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
    question                TEXT NOT NULL,
    question_normalized     TEXT NOT NULL,
    answer_md               TEXT NOT NULL,
    job_id                  TEXT REFERENCES job(id) ON DELETE SET NULL,
    tags_json               TEXT CHECK (tags_json IS NULL OR json_valid(tags_json)),
    use_count               INTEGER NOT NULL DEFAULT 0,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
) STRICT;

CREATE INDEX idx_qa_profile ON question_answer(profile_id, question_normalized);

-- ======================================================================================
-- Platform: queue, cache, embeddings, auth, events, settings
-- ======================================================================================

CREATE TABLE task (
    id                  TEXT PRIMARY KEY,
    kind                TEXT NOT NULL CHECK (kind IN (
                            'ingest_url','ingest_capture','extract_job','refresh_listing',
                            'dedupe_job','materialize_job','embed','score_match',
                            'aggregate_gaps','render_document','import_resume',
                            'reconcile_files','poll_board','maintenance')),
    payload_json        TEXT NOT NULL CHECK (json_valid(payload_json)),
    status              TEXT NOT NULL DEFAULT 'queued' CHECK (status IN (
                            'queued','running','done','failed','cancelled')),
    priority            INTEGER NOT NULL DEFAULT 0,
    attempts            INTEGER NOT NULL DEFAULT 0,
    max_attempts        INTEGER NOT NULL DEFAULT 5,
    last_error          TEXT,
    progress            REAL,
    progress_message    TEXT,
    -- Coalescing: enqueueing "extract job X" five times must produce one row.
    dedupe_key          TEXT UNIQUE,
    available_at        TEXT NOT NULL,
    -- Leases make crash recovery automatic: an expired lease returns the task to 'queued'.
    lease_expires_at    TEXT,
    started_at          TEXT,
    finished_at         TEXT,
    parent_task_id      TEXT REFERENCES task(id) ON DELETE SET NULL,
    trace_id            TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
) STRICT;

-- The claim query's covering index.
CREATE INDEX idx_task_claim  ON task(status, available_at, priority DESC);
CREATE INDEX idx_task_lease  ON task(lease_expires_at) WHERE status = 'running';
CREATE INDEX idx_task_recent ON task(created_at DESC);

-- Cache + audit + cost accounting. The UNIQUE prompt_hash *is* the cache (FR-E-10).
CREATE TABLE llm_call (
    id                  TEXT PRIMARY KEY,
    provider            TEXT NOT NULL,
    model               TEXT NOT NULL,
    purpose             TEXT NOT NULL,
    prompt_hash         TEXT NOT NULL UNIQUE,
    request_json        TEXT CHECK (request_json IS NULL OR json_valid(request_json)),
    response_json       TEXT CHECK (response_json IS NULL OR json_valid(response_json)),
    response_valid      INTEGER NOT NULL DEFAULT 1 CHECK (response_valid IN (0,1)),
    prompt_tokens       INTEGER,
    completion_tokens   INTEGER,
    cost_micros         INTEGER NOT NULL DEFAULT 0,
    latency_ms          INTEGER,
    error               TEXT,
    trace_id            TEXT,
    created_at          TEXT NOT NULL
) STRICT;

CREATE INDEX idx_llm_purpose ON llm_call(purpose, created_at DESC);

CREATE TABLE embedding (
    id              TEXT PRIMARY KEY,
    owner_kind      TEXT NOT NULL CHECK (owner_kind IN (
                        'job','requirement','accomplishment','skill','document','profile')),
    owner_id        TEXT NOT NULL,
    model           TEXT NOT NULL,
    dim             INTEGER NOT NULL,
    -- Little-endian f32. Brute-force cosine over <=1e5 of these is milliseconds in Rust;
    -- sqlite-vec is a drop-in if the corpus ever outgrows that.
    vector          BLOB NOT NULL,
    -- Text hash, so an edit invalidates precisely this vector.
    content_hash    TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    UNIQUE (owner_kind, owner_id, model)
) STRICT;

CREATE TABLE user (
    id              TEXT PRIMARY KEY,
    username        TEXT NOT NULL UNIQUE,
    -- Argon2id.
    password_hash   TEXT NOT NULL,
    role            TEXT NOT NULL DEFAULT 'owner' CHECK (role IN ('owner','viewer')),
    disabled        INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0,1)),
    created_at      TEXT NOT NULL,
    last_login_at   TEXT
) STRICT;

CREATE TABLE session (
    -- BLAKE3 of the bearer token: a database dump yields no usable sessions.
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    created_at      TEXT NOT NULL,
    expires_at      TEXT NOT NULL,
    last_seen_at    TEXT NOT NULL,
    user_agent      TEXT,
    ip              TEXT,
    revoked_at      TEXT
) STRICT;

CREATE INDEX idx_session_user ON session(user_id, expires_at);

-- Per-device extension tokens, scoped to ingest only: a leaked device token cannot read
-- your resume or your applications.
CREATE TABLE device_token (
    id              TEXT PRIMARY KEY,
    token_hash      TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    scopes_json     TEXT NOT NULL DEFAULT '["ingest"]' CHECK (json_valid(scopes_json)),
    created_at      TEXT NOT NULL,
    last_used_at    TEXT,
    expires_at      TEXT,
    revoked_at      TEXT
) STRICT;

-- Durable domain events, so an SSE client that reconnects can backfill rather than losing
-- progress. Pruned by the maintenance task.
CREATE TABLE event_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    kind            TEXT NOT NULL,
    entity_kind     TEXT,
    entity_id       TEXT,
    payload_json    TEXT CHECK (payload_json IS NULL OR json_valid(payload_json)),
    created_at      TEXT NOT NULL
) STRICT;

CREATE INDEX idx_event_log_created ON event_log(created_at);

CREATE TABLE setting (
    key         TEXT PRIMARY KEY,
    value_json  TEXT NOT NULL CHECK (json_valid(value_json)),
    updated_at  TEXT NOT NULL
) STRICT;

CREATE TABLE tag (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    color       TEXT,
    kind        TEXT,
    created_at  TEXT NOT NULL
) STRICT;

CREATE TABLE job_tag (
    job_id      TEXT NOT NULL REFERENCES job(id) ON DELETE CASCADE,
    tag_id      TEXT NOT NULL REFERENCES tag(id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (job_id, tag_id)
) STRICT;

CREATE INDEX idx_job_tag_tag ON job_tag(tag_id);

CREATE TABLE saved_view (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    entity      TEXT NOT NULL DEFAULT 'job' CHECK (entity IN ('job','application')),
    query_json  TEXT NOT NULL CHECK (json_valid(query_json)),
    is_pinned   INTEGER NOT NULL DEFAULT 0 CHECK (is_pinned IN (0,1)),
    ordinal     INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
) STRICT;

-- Opt-in polling of *public* ATS boards only. Never an authenticated site.
CREATE TABLE board_watch (
    id                  TEXT PRIMARY KEY,
    source_id           TEXT NOT NULL REFERENCES source(id) ON DELETE CASCADE,
    name                TEXT NOT NULL,
    config_json         TEXT NOT NULL CHECK (json_valid(config_json)),
    cadence_minutes     INTEGER NOT NULL DEFAULT 720,
    last_run_at         TEXT,
    last_result_json    TEXT CHECK (last_result_json IS NULL OR json_valid(last_result_json)),
    enabled             INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
    created_at          TEXT NOT NULL
) STRICT;

-- Tracks deletions so `reconcile --from-files` cannot resurrect something you deleted.
CREATE TABLE tombstone (
    entity_kind TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    deleted_at  TEXT NOT NULL,
    reason      TEXT,
    PRIMARY KEY (entity_kind, entity_id)
) STRICT;
