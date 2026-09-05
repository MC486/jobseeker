# Experience bank & document generation

## 1. Why a bank instead of resume files

A resume is a *projection* of your history onto one job. Storing resumes as documents means
re-deriving that projection by hand every time. Storing the history as structured
accomplishments makes tailoring a query.

```
experience_item (role at Acme, 2021-03 → 2024-08)
  └─ accomplishment
       text:      "Rebuilt the event ingestion pipeline in Rust, cutting p95 latency
                   from 1.2s to 180ms at 40k events/sec"
       variants:  { short: "Cut ingestion p95 1.2s → 180ms (Rust, 40k eps)",
                    leadership: "Led a 3-engineer rewrite of event ingestion…" }
       impact:    metric="p95 latency" value=85 unit="%" direction=decrease
       scope:     { team_size: 3, users_affected: 2_000_000 }
       skills:    [rust:1.0, kafka:0.8, distributed-systems:0.9, observability:0.5]
       strength:  5   verified: true
       evidence:  https://github.com/…/pull/812
       STAR:      situation / action / result decomposition
```

That single record serves four purposes: a resume bullet (in three lengths), match evidence,
an interview story, and a skill-years data point.

## 2. Bootstrapping the bank

Nobody hand-enters 60 accomplishments. `POST /api/v1/profiles/:id/import-resume`:

1. Accept PDF / DOCX / Markdown / plain text.
2. Extract text (`pdf-extract` for PDF, DOCX = zip + XML parse — no external converter).
3. Segment into sections (experience / education / skills / projects) using layout and
   heading heuristics.
4. Parse each role: org, title, dates, location; each bullet becomes a draft accomplishment.
5. LLM pass (optional) to extract `impact_metric/value/unit`, propose skill tags, and
   generate STAR decomposition.
6. Land everything as **drafts** in a review queue. You confirm, correct, and rate strength.
   Nothing enters the bank unreviewed — the bank is the source of truth for claims you will
   make to employers, so it must be exactly right.

Follow-up loop: the UI surfaces "accomplishments with no metric" and "skills demanded by
your saved jobs with no supporting accomplishment," which is a concrete, finite backlog for
improving your own data.

## 3. Tailored resume generation

`POST /api/v1/documents/resume { job_id, profile_id, template, budget }`

### 3.1 Selection (deterministic, no LLM)

Selection is an optimization, not a prompt. Maximize coverage of the job's required
requirements subject to a length budget:

```
maximize   Σ_r covered(r) · weight(r)  +  λ · Σ_a strength(a) · relevance(a)
subject to Σ_a length(a) ≤ budget
           per-role bullet count ∈ [1, 6]
           recency: ≥60% of bullets from the two most recent roles
```

Solved greedily by marginal coverage per line (a weighted set-cover heuristic, which is
within a log factor of optimal and takes microseconds), then improved by local swaps.

`relevance(a) = max over job requirements of (skill overlap 0.6 + embedding cosine 0.4)`.

Output records, per selected bullet: the accomplishment id, the requirements it targets, the
variant chosen, and the position. That is `document.selection_json`, and it is what makes
FR-G-05 (provenance per bullet) real.

### 3.2 Phrasing (LLM, tightly constrained)

The model may only **rewrite** a selected accomplishment, never add facts:

```
system: Rewrite the bullet to emphasize the target requirement. Rules:
        - Do not introduce technologies, numbers, dates, or claims not in the input.
        - Keep every number exactly as given.
        - Start with a strong past-tense verb. One sentence. ≤ {max_chars} characters.
        - No adjectives about the author ("passionate", "highly skilled").
user:   bullet: {accomplishment.text}
        metrics: {impact_metric} {impact_value}{impact_unit} {direction}
        scope: {scope_json}
        target requirement: {requirement.text}
```

Post-generation validation, applied automatically:

- Every number in the output must appear in the input (regex extraction + set comparison).
- Every named technology in the output must be in the accomplishment's skill set or its text.
- Length within budget.

A bullet failing validation falls back to the stored variant. The result is that
hallucination is *structurally* prevented rather than requested politely (FR-G-01).

### 3.3 Rendering

Typst is the renderer ([ADR-0008](adr/0008-typst-rendering.md)): fast, single binary,
scriptable, no LaTeX install. Templates in `templates/resume/*.typ` receive a JSON data model
so a user can restyle without touching Rust.

- `ats.typ` — one column, no tables, no icons, no columns-as-layout, standard section
  headings, embedded selectable text. Designed for machine parsing.
- `modern.typ` — two-column sidebar, still text-extractable.
- `academic.typ` — publications, teaching, grants.

Output: PDF plus the Typst source, both stored. Rendering is optional
(`resume.renderer = "none"` yields Markdown/Typst source only), so a missing binary degrades
gracefully instead of breaking generation (FR-G-04).

### 3.4 Response

```jsonc
{ "document_id": "…", "version": 3, "page_count": 1,
  "coverage": { "required_total": 13, "required_covered": 11,
                "uncovered": [{"requirement_id":"…","text":"3+ years Kubernetes"}] },
  "keyword_gaps": ["Kubernetes", "Terraform", "SRE"],   // in posting, absent from resume
  "bullets": [ { "accomplishment_id": "…", "targets": ["req-1","req-7"],
                 "variant": "short", "text": "…" } ] }
```

`keyword_gaps` (F10) is presented as information, not a directive: it tells you which of the
posting's literal terms are absent, and you decide whether that reflects a real gap or just
phrasing.

## 4. Cover letters

Input: job (company, title, mission text, top requirements), profile summary, and the 3–5
highest-relevance accomplishments. Structure: hook tied to something specific about the
company from the posting, two paragraphs each anchored on one accomplishment with its metric,
a short close addressing the largest gap honestly if it is material.

Constraints: same no-new-facts validation as bullets; company facts must be quoted from the
job record; explicitly marked draft; no "I am passionate about" filler (an anti-pattern list
is part of the prompt and checked post-hoc).

## 5. Versioning and diffs

Every generation is a new `document` row with `parent_document_id`, so nothing is
overwritten. `GET /api/v1/documents/:id/diff/:other_id` returns a bullet-level diff (added /
removed / rephrased with the source accomplishment id), which answers "what did I actually
send to this company in March?" — a question that comes up in real interviews.

An `application` pins the exact document version submitted (FR-G-06), so the pinned artifact
is immutable even as you keep iterating on later versions.

## 6. Interview prep pack (M4)

From the job's requirements plus your matched evidence:

- Likely technical topics, ranked by requirement weight.
- For each `met` requirement, your STAR story from `accomplishment.situation/action/result`.
- For each `gap`, a prepared honest framing ("I have not run Kubernetes in production; I
  have done X, and here is how I would approach it") — because the alternative is improvising
  under pressure.
- Questions to ask, derived from the posting's ambiguities (unclear team, no comp band, vague
  scope).
- Your open `question_answer` bank entries relevant to this company.

## 7. Answer bank

Recurring application questions ("Why do you want to work here?", "Salary expectations",
"Describe a conflict") get normalized (lowercase, stopword-stripped, embedded) so a new
application form can retrieve your previous answer by similarity, with the option to adapt it
for the specific company. `use_count` surfaces which answers are load-bearing and worth
polishing.
