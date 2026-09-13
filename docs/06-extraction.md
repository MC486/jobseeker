# Extraction & normalization

Raw bytes → a structured, provenance-tagged `Job` + `Vec<Requirement>`.

## 1. Target structure

```rust
pub struct Sourced<T> { pub value: T, pub provenance: Provenance, pub confidence: f32 }

pub struct ExtractedJob {
    pub title:            Option<Sourced<String>>,
    pub company_name:     Option<Sourced<String>>,
    pub description_html: Option<Sourced<String>>,
    pub description_md:   Option<Sourced<String>>,
    pub locations:        Vec<Sourced<RawLocation>>,
    pub work_mode:        Option<Sourced<WorkMode>>,
    pub employment_type:  Option<Sourced<EmploymentType>>,
    pub seniority:        Option<Sourced<Seniority>>,
    pub salary:           Option<Sourced<RawSalary>>,
    pub posted_at:        Option<Sourced<PartialDate>>,
    pub closes_at:        Option<Sourced<PartialDate>>,
    pub apply_url:        Option<Sourced<String>>,
    pub source_job_id:    Option<Sourced<String>>,
    pub department:       Option<Sourced<String>>,
    pub benefits:         Vec<Sourced<String>>,
    pub requires_clearance: Option<Sourced<String>>,
    pub visa_sponsorship: Option<Sourced<Tristate>>,
    pub education_min:    Option<Sourced<EducationLevel>>,
    pub raw_fields:       serde_json::Map<String, Value>,  // never discard what we saw
}
```

Every field is optional and carries its own provenance and confidence. **Nothing is ever
guessed into a non-null value** (FR-E-04) — `null` plus retained raw text beats a confident
lie, because a wrong salary silently poisons every downstream filter and score.

## 2. Stage order

```
 stage           provenance  typical confidence   cost
 ────────────────────────────────────────────────────────────
 1 ATS JSON API   api         0.95–1.00           1 request
 2 JSON-LD        jsonld      0.90                free
 3 microdata/RDFa microdata   0.80                free
 4 site adapter   adapter     0.85                free
 5 heuristics     rules       0.50–0.75           free
 6 LLM            llm         0.70–0.90           tokens
 7 inference      inferred    0.40–0.60           free
```

Each stage **only fills fields that are still `None`**, and a stage never lowers an existing
field's provenance (see the precedence order in [Architecture §4](03-architecture.md)). In
`llm_first` debug mode all stages run independently and disagreements are reported — that is
how the eval harness measures whether an adapter is actually better than the model.

### Stage 2: JSON-LD (`schema.org/JobPosting`)

The single best free signal; present on most ATS pages and many company sites.

```json
{ "@type": "JobPosting", "title": "Senior Platform Engineer",
  "hiringOrganization": {"name": "Acme Robotics"},
  "datePosted": "2026-09-02", "validThrough": "2026-10-15",
  "employmentType": "FULL_TIME",
  "jobLocationType": "TELECOMMUTE",
  "applicantLocationRequirements": {"@type":"Country","name":"US"},
  "baseSalary": {"@type":"MonetaryAmount","currency":"USD",
                 "value":{"@type":"QuantitativeValue","minValue":185000,
                          "maxValue":225000,"unitText":"YEAR"}},
  "description": "<p>…</p>" }
```

Handled edge cases, all of which occur in the wild: `@graph` wrappers; arrays of postings on
one page; `hiringOrganization` as a bare string; `baseSalary` as a number, a string, or a
`QuantitativeValue`; `jobLocation` as object or array; HTML-escaped `description`;
`identifier` as object or string; stale `datePosted` on reposted jobs. Each gets a fixture.

### Stage 4: Site adapters

```rust
pub trait SiteAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn fidelity(&self) -> u8;
    fn matches(&self, url: &Url) -> bool;
    fn extract(&self, cap: &CaptureView) -> Result<ExtractedJob>;
    fn closure_markers(&self) -> &[&'static str] { &[] }
}
```

`CaptureView` gives an adapter the parsed DOM, the raw HTML, the extension's `text`, and any
embedded JSON. Adapters are registered by host pattern in `AdapterRegistry`; adding a site
touches exactly one directory (NFR-M-02).

Adapter notes worth writing down:

- **LinkedIn** — the useful content is in the hydrated DOM (`.jobs-description__content`,
  `.job-details-jobs-unified-top-card__*`) and in embedded JSON blobs. Salary often only
  appears in a "compensation" module and is frequently LinkedIn's own *estimate* — set
  `salary_is_estimate = true`. `data-job-id` attribute gives `source_job_id`. Class names
  churn: adapters must degrade to the generic path instead of failing hard, and every
  selector list is a `&[&str]` of alternatives tried in order.
- **Indeed** — `window._initialData` / `mosaic-provider-jobcards` JSON is far more stable
  than the DOM. Salary is often "Estimated $X–$Y a year" → `is_estimate`. `jk` is the id.
- **Greenhouse / Lever / Ashby** — prefer the API (stage 1). The HTML fallback is simple and
  stable (`#content`, `.posting-description`).
- **Workday** — prefer the CXS JSON the careers SPA loads
  (`/wday/cxs/<tenant>/<site>/job/…`). `jobPostingInfo` carries `title`,
  `jobDescription`, `location`, `additionalLocations`, `jobReqId`, `remoteType`,
  `timeType`. The HTML fallback reads `data-automation-id`. Requisition ids are
  not only `R-` / `REQ` / `JR` — Zillow-style `P751219-2` is an identity.
  Multiple locations on one req become multiple `job_location` rows. LinkedIn
  still returns `needs_browser`; paste or the extension is the path for that
  copy of the same posting.
- **Glassdoor / ZipRecruiter / Dice** — aggregators; fidelity 40. Treat salary as estimate
  unless explicitly labeled by the employer.

### Stage 5: Generic heuristics

For unknown hosts: strip `script/style/nav/header/footer/aside/form`, drop nodes matching a
boilerplate class/id denylist (cookie, consent, subscribe, related-jobs, breadcrumb, social),
score remaining blocks by text density and job-vocabulary keyword hits ("responsibilities",
"qualifications", "you will", "we offer"), take the best subtree, convert to Markdown.
Title from `<h1>` / `og:title` / `<title>` minus company suffix; company from
`og:site_name` / apply-URL host / copyright line.

### Stage 6: LLM extraction

Runs only if required fields are still missing, or if requirement atomization is needed
(almost always). Strict JSON-schema-constrained output; validated before any application
(FR-E-09).

```
system: You extract structured data from job postings. Output only JSON matching the
        schema. Use null for anything not stated in the text. Never infer or invent
        salary, dates, or company names. Copy requirement text verbatim.
user:   URL: {canonical_url}
        Captured: {captured_at}
        Known so far: {json of already-extracted fields}   ← anchors the model
        --- POSTING (markdown, truncated to {max_tokens}) ---
        {description_md}
```

Rules that keep this honest and cheap:

- Ask only for missing fields; a mostly-extracted posting costs a fraction of a full call.
- Chunk long descriptions by heading, atomize requirements per chunk, then merge — avoids
  mid-list truncation, which is the main failure mode on 4,000-word postings.
- `temperature = 0`, seed pinned where supported, so results are reproducible.
- Cache by `blake3(provider|model|schema|prompt|params)` in `llm_call` (FR-E-10).
- Validation failure → one retry with the validation error appended; then give up and mark
  `extraction_partial`.
- Provider unreachable → the job still lands via stages 1–5 (FR-E-11, AS-10).

## 3. Requirement atomization

The transform that makes everything else possible. Input: description Markdown. Output:
typed `Requirement` rows.

**Segmentation.** Split on headings and list items. Recognize section intent from headings:

| Heading matches | Default necessity |
|---|---|
| "requirements", "qualifications", "must have", "what you'll need", "basic qualifications" | `required` |
| "preferred", "nice to have", "bonus", "plus", "desired", "preferred qualifications" | `preferred` |
| "responsibilities", "what you'll do", "the role", "day to day" | `responsibility` kind |
| "benefits", "perks", "what we offer", "compensation" | not a requirement — routed to `benefits_md` |
| "about us", "our mission", "EEO", "equal opportunity" | discarded |

**Per-item classification.** Rules first (they are precise and free):

- Years: `/(\d+)\s*\+?\s*(?:to|-|–)?\s*(\d+)?\s*(?:\+)?\s*years?/i` → `min_years`/`max_years`;
  "at least three years" → word-number parsing.
- Necessity overrides inside the text: "must", "required", "essential" → `required`;
  "preferred", "ideally", "bonus points", "a plus", "nice to have" → `preferred`.
- Education: degree words + field → `kind=education`, `education_level`, `field_of_study`.
- Certification: `/\b(AWS|Azure|GCP|CISSP|PMP|CPA|Security\+|CKA|RHCE)\b/` and
  "certified|certification".
- Clearance: type (`secret`, `ts_sci`, …) plus start vs obtain. `CLEARANCE REQUIRED
  FOR START: No` / "ability to obtain" → type is stored, `clearance_required_to_start
  = 0`, requirement `is_blocker = false`. "Must hold an active TS/SCI" →
  `is_blocker = true`. A type without start language is unspecified, not a silent
  hold-at-start.
- Citizenship / US-person: `kind=logistics`, not a skill. Matched against
  `profile.citizenship`.
- Human language: "fluent in", "native", named languages → `kind=language`.
- Skills/tools: taxonomy alias matching over the item text (§4).
- Soft skills: a curated phrase list ("communication", "self-starter", "cross-functional",
  "ownership") → `kind=soft_skill`, deliberately low weight in matching.
- Logistics: travel %, on-call, shift work, relocation, work authorization →
  `kind=logistics`, `is_blocker` when categorical.

Anything the rules cannot classify goes to the LLM in one batched call — a JSON array of
`{text, kind, necessity, skills[], min_years, level, is_blocker}` — rather than one call per
bullet.

**A single bullet often contains several requirements.** "5+ years of Python and experience
with Kubernetes and Terraform in production" becomes four records (Python with 5 years,
Kubernetes, Terraform, production experience), all pointing at the same `source_span` so the
UI can highlight the origin sentence.

**Dedup within a job** by `normalized_text` (lowercased, stopworded, stemmed) and by
`(skill_id, necessity)` — postings routinely repeat the same demand in the summary and again
in the bullet list. Keep the highest-necessity, longest-text instance.

## 4. Skill taxonomy

- **Seed** (checked in at `crates/normalize/data/skills.toml`, ~600 entries): languages,
  frameworks, databases, clouds, infra/DevOps, data/ML, mobile, frontend, security, testing,
  methodologies, certifications, human languages, plus a curated soft-skill list.
- **Aliases** cover abbreviations (`k8s`, `tf`, `pg`, `js`, `ts`), vendor spellings
  (`Postgres`/`PostgreSQL`, `GCP`/`Google Cloud`), and common misspellings (`Javascript`,
  `Kubernets`).
- **Hierarchy** enables partial credit: React experience partially satisfies "JavaScript",
  and PostgreSQL partially satisfies "relational databases" — with a documented decay factor
  per hop (default 0.7).
- **Matching** is longest-alias-first over normalized tokens, with word-boundary anchoring so
  "Rust" does not match "trust" and "Go" does not match "Google". Ambiguous aliases
  (`is_ambiguous`) require a nearby corroborating token or LLM confirmation.
- **Unknown skills** are not silently dropped: `requirement.skill_id` stays null and the
  normalized text lands in a `skill_candidate` review queue. You promote a candidate to the
  taxonomy in one click, which is how the taxonomy grows to fit *your* market instead of a
  generic one.

## 5. Field normalization

**Salary.** Handles `$120,000 - $150,000`, `120k-150k`, `$60/hr`, `£70,000 per annum`,
`€90.000 – €110.000`, `USD 185000-225000/yr`, `up to $200k`, `from $150k`,
`$8,000 per month`, `competitive` (→ null), and Indeed's `Estimated: $X - $Y a year`
(→ `is_estimate`). Output: min/max in minor units, ISO currency (inferred from symbol,
explicit code, or job country), period, `is_estimate`, and always the verbatim `salary_raw`.
Sanity bounds reject nonsense (annual < 1,000 or > 10,000,000 → null + conflict row).
Hourly↔annual conversion is *computed at query time* (× 2080), never stored, so the stored
value stays faithful to the posting.

**Location.** `"San Francisco, CA (Hybrid)"` → city, region `CA`, country `US`, work mode
`hybrid`. `"Remote - United States"` → `is_remote_scope` row with country `US`.
`"Multiple locations"` + a list → several rows. Region names normalize to codes; a
city→(region, country, lat, lon) gazetteer of ~40k places ships with the app (offline, no
geocoding service). Unresolvable strings are kept in `raw` — never dropped.

**Dates.** `"3 days ago"`, `"Posted 30+ days ago"` (→ day precision, `>=30` flag),
`"Reposted 2 weeks ago"`, `"Sep 2, 2026"`, ISO 8601, epoch millis. Relative dates resolve
against `capture.captured_at`, not "now", so re-extracting an old capture stays correct.
Precision is recorded (FR-E-06); a `30+ days ago` posting is displayed as "≥30d" rather than
a fake exact date.

**Seniority.** Title tokens first (`senior`, `sr.`, `staff`, `principal`, `lead`, `head of`,
`II`/`III`, `junior`, `jr`, `associate`, `intern`, `new grad`), then `years_experience_min`
bands (0–1 entry, 2–4 mid, 5–8 senior, 9+ staff), then the LLM. Company-relative
inflation ("Senior" at a 20-person startup vs at a bank) is explicitly *not* modeled; it is
noted as a known limitation.

**Company name.** Strip legal suffixes and punctuation, collapse whitespace, lowercase →
`name_normalized`. Resolution order: exact `name_normalized`, then known-alias table, then
apply-URL host → company website match, then trigram similarity ≥ 0.9 with confirmation.
Wrong company merges are user-visible and annoying, so the threshold is deliberately high.

## 6. Provenance, conflicts, and manual edits

- `field_provenance` stores `(entity, field) → (provenance, confidence, model, capture_id)`.
- The write path refuses to overwrite `manual` (FR-E-03, AS-05).
- Material disagreements between sources create `extraction_conflict` rows, rendered in the
  UI as "LinkedIn: $150–170k (estimate) · Greenhouse: $185–225k". This is a *feature*: the
  discrepancy between an aggregator's estimate and the ATS's posted band is genuinely useful
  information. Same-company merge writes an unresolved salary row when the raw strings
  differ; `GET /api/v1/jobs/:id/conflicts` and the job-page banner surface it. The user
  picks A, B, or keep-current — nothing is auto-resolved.
- Editing a field in the UI writes provenance `manual`, confidence `1.0`, and appends to the
  eval corpus as a labeled example — your corrections become the regression suite.

## 7. Evaluation harness

`crates/extract/tests/fixtures/<source>/<case>/` holds `input.html` (or `input.json`),
`meta.json` (url, captured_at), and `expected.json` (hand-labeled).

`just eval` reports, per source and per field, exact-match rate for scalars, set F1 for
requirements and skills, and a diff of regressions against the last committed baseline
(`eval/baseline.json`). CI fails on regression beyond a small tolerance (FR-E-12).

Fixtures are scrubbed of personal data before being committed; captures from your real
searches stay in the data directory, never in the repo.
