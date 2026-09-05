# Matching, gap analysis & scoring

Goal: replace "78% match" with "meets 11 of 13 required; blocked by a clearance you do not
have; missing Kubernetes by roughly 3 years."

## 1. Inputs

| Side | Records |
|---|---|
| Job | `requirement` rows (kind, necessity, skill_id, min_years, level, is_blocker), `seniority`, salary, locations, `work_mode`, `description_text` embedding |
| Profile | `profile_skill` (years, level, last_used_year), `accomplishment` (+ `accomplishment_skill`, embeddings), `experience_item` (dates → tenure), targets (comp, locations, work auth, relocation) |

Derived once per profile and cached:

- `skill_evidence: Map<SkillId, Evidence>` where `Evidence { years, level, last_used_year,
  accomplishment_ids, direct: bool }`. Years come from the max of the self-asserted value and
  the summed tenure of experience items whose accomplishments touch that skill —
  self-assertion and demonstrated evidence are reconciled, not blindly trusted.
- Hierarchy closure: evidence propagates *up* the taxonomy with a decay of 0.7 per hop
  (React evidence partially satisfies "JavaScript"), never down (JavaScript does not imply
  React).
- Total years of relevant experience, per-domain tenure, current seniority estimate.

## 2. Per-requirement verdict

For each requirement, dispatch on `kind`:

```
skill / tool
  ├─ direct evidence?           → years_have = evidence.years
  ├─ hierarchy evidence?        → years_have = evidence.years * 0.7^hops
  ├─ semantic-only match?       → cosine(req, best accomplishment) ≥ 0.72 → partial
  └─ none                       → gap
     verdict:
       no min_years:            met if any evidence, else gap
       years_have ≥ min_years:  met
       ≥ 0.6 × min_years:       partial   (score = years_have / min_years)
       else:                    gap
     recency penalty: last_used_year older than 3 yrs → score ×= 0.85^(age-3), floor 0.5
                      (a skill last used in 2016 is not the same as one used last year)

experience (non-skill, e.g. "led a team of 5+")
  → semantic match against accomplishments; ≥0.78 met, ≥0.65 partial, else gap.
    Scope-aware: accomplishment.scope_json.team_size ≥ required → met outright.

education      → compare education_level ordinal; met/partial(adjacent)/gap. is_blocker when
                 the posting says "required" and no equivalent-experience clause is present.
certification  → exact/alias match against profile certifications; expiry checked.
clearance      → boolean; gap ⇒ hard blocker.
language       → level comparison from profile_skill(kind=language_human).
soft_skill     → semantic match, capped low weight (they are unfalsifiable).
logistics      → rule per subtype: travel % vs tolerance, work auth vs sponsorship,
                 on-site location vs acceptable locations. Categorical mismatch ⇒ blocker.
responsibility → not scored for fit; used for resume targeting and interview prep.
unknown        → verdict `unknown`, excluded from denominators (never silently counted as a
                 pass or a fail).
```

Each verdict stores its evidence (`[{kind:"accomplishment", id, similarity}, …]`) and a
one-line rationale, so the UI can show *why* (FR-M-01, AS-07).

## 3. Subscores

All in `[0,1]`, all reported individually.

**`required_coverage`** — weighted coverage of `necessity = required`:

```
weight(r) = base_weight(r.kind) × (1 + 0.15 × min(r.min_years, 10) / 10)
required_coverage = Σ weight(r) × score(r) / Σ weight(r)      over required, verdict ≠ unknown

base_weight: skill 1.0, tool 0.9, experience 1.0, education 0.8,
             certification 0.8, clearance 1.0, language 0.9, soft_skill 0.3, logistics 0.7
```

**`preferred_coverage`** — same over `preferred` + `nice_to_have`.

**`seniority_fit`** — ordinal distance between job seniority and profile estimate, combined
with the years band. Deliberately **asymmetric**: being one level under is penalized more
(0.75) than one level over (0.9), because under-qualification screens you out while
over-qualification is usually a negotiation problem. Two-plus levels either way → ≤0.4.

**`comp_fit`** — job band vs `profile.target_comp_min_cents`, after normalizing period
(hourly × 2080). `job.max ≥ target` → 1.0; `job.max ≥ 0.9 × target` → 0.7; linear decay to
0 at 0.6 × target. Missing salary → 0.5 with an `unknown_comp` flag (neutral, and flagged,
rather than silently penalizing the ~50% of postings with no band).

**`location_fit`** — remote & you accept remote → 1.0; on-site in an acceptable metro → 1.0;
hybrid in an acceptable metro → 0.9; on-site elsewhere and willing to relocate → 0.5;
otherwise 0.0 + blocker.

**`semantic_similarity`** — cosine between the job description embedding and a profile
embedding (mean of the top-k accomplishment vectors, k = 20, weighted by `strength`).
Catches domain fit that requirement matching misses ("fintech", "developer tools",
"real-time systems"). Absent an embedding provider → excluded and weights renormalized, with
an explicit `semantic_unavailable` flag (FR-M-06).

**Overall:**

```
overall = Σ wᵢ · subscoreᵢ / Σ wᵢ           (over available subscores)
if blocker_count > 0: overall = min(overall, 0.45)   and blockers are listed first
```

Default weights (user-configurable, FR-M-04):

```
required_coverage 0.45 · preferred_coverage 0.15 · semantic 0.15
seniority 0.10 · comp 0.10 · location 0.05
```

The blocker cap is a hard product decision: a job requiring a clearance you cannot get is
not an 85% match no matter how well the skills line up. The cap is visible in the
explanation, never hidden.

## 4. Explanation payload

```jsonc
{
  "overall": 0.81, "algorithm_version": "1.0.0",
  "verdict": "strong_fit_with_gaps",
  "subscores": { "required_coverage": 0.85, "preferred_coverage": 0.57,
                 "semantic_similarity": 0.79, "seniority_fit": 1.0,
                 "comp_fit": 1.0, "location_fit": 1.0 },
  "weights_used": { "...": 0.45 },
  "blockers": [],
  "flags": ["unknown_comp:false", "semantic_available:true"],
  "counts": { "required": {"met": 11, "partial": 1, "gap": 1, "unknown": 0},
              "preferred": {"met": 4, "partial": 1, "gap": 2, "unknown": 0} },
  "top_strengths": [
    { "requirement_id": "…", "text": "5+ years building distributed systems in Rust",
      "score": 1.0, "evidence": [{"kind":"accomplishment","id":"…","similarity":0.91,
      "text":"Rebuilt the ingestion pipeline in Rust, cutting p95 from 1.2s to 180ms"}] }
  ],
  "top_gaps": [
    { "requirement_id": "…", "text": "3+ years operating Kubernetes in production",
      "status": "gap", "years_have": 0.5, "years_needed": 3,
      "rationale": "No accomplishments reference Kubernetes; closest evidence is Docker Compose (0.61)." }
  ],
  "narrative": "Strong technical fit. Two required gaps: Kubernetes depth and Go. \
Comp band is above your floor and the role is remote-eligible."
}
```

The `narrative` is the only optional LLM-generated part and is regenerated cheaply from the
deterministic structure — the numbers never come from a model.

## 5. Staleness and recomputation

`inputs_hash = blake3(job.content_hash ‖ profile_revision ‖ weights ‖ algorithm_version)`.

- Job updated → `UPDATE match_score SET is_stale = 1 WHERE job_id = ?`.
- Profile/experience/skill changed → bump `profile.revision`, mark that profile's scores
  stale.
- Weights or algorithm version changed → mark all stale.
- The scheduler drains stale scores at low priority; the UI shows the last known value with
  a "recomputing" affordance rather than a spinner or a blank.

Recomputation is pure CPU (embeddings are cached), so a full re-score of 5,000 jobs is a
few seconds — cheap enough that tuning weights interactively is realistic (FR-M-08).

## 6. Aggregate gap analysis / learning plan

```sql
SELECT s.id, s.name,
       COUNT(*)                                                      AS frequency,
       SUM(r.necessity = 'required')                                 AS blocking_count,
       AVG(COALESCE(r.min_years, 0))                                 AS avg_years_wanted,
       AVG(rm.score)                                                 AS avg_current_score
FROM requirement r
JOIN skill s              ON s.id = r.skill_id
JOIN job j                ON j.id = r.job_id AND j.status = 'open'
                                            AND j.is_archived = 0 AND j.deleted_at IS NULL
JOIN match_score ms       ON ms.job_id = j.id AND ms.profile_id = ?1
JOIN requirement_match rm ON rm.match_score_id = ms.id AND rm.requirement_id = r.id
WHERE rm.status IN ('gap', 'partial')
GROUP BY s.id
ORDER BY blocking_count DESC, frequency DESC;
```

Leverage score, which is the number actually worth acting on:

```
leverage(skill) = Σ over jobs where this skill is a gap of
                    (Δoverall if this requirement became `met`) × interest_weight(job)
interest_weight = 0.5 + 0.1 × user_rating   (unrated → 1.0)
```

This answers the real question: *"which single skill, if I learned it, would most improve my
position across the jobs I actually want?"* The answer is often not the most frequent skill —
frequency counts everything equally, leverage weights by how close each job already is to
flipping and how much you want it.

Output per skill: current evidence, years short, the jobs it unlocks, a suggested action
(project / course / certification), and an estimated effort band. Suggested resources are
LLM-generated when a provider exists and otherwise a static curated map for the top ~100
skills — the plan still works offline.

## 7. Embeddings

- What gets embedded: `job.description_text` (chunked at ~500 tokens, mean-pooled),
  each `requirement.normalized_text`, each `accomplishment.text`, each `skill.name` +
  description.
- Storage: `embedding` table, `BLOB` of little-endian f32, keyed by
  `(owner_kind, owner_id, model)` with a `content_hash` so text edits invalidate precisely.
- Search: brute-force cosine in Rust over ≤10⁵ vectors — a few milliseconds with f32 slices,
  and no index to keep consistent. `sqlite-vec` is a drop-in when the corpus grows.
- Default model `nomic-embed-text` (768-dim) via Ollama; `fastembed` is a pure-Rust fallback
  requiring no external service. Dimension is stored per row so switching models is a
  re-embed task, not a migration.

## 8. Honest limitations (documented, not hidden)

1. **Requirement inflation is real.** Postings ask for more than the job needs. A 0.7 is
   frequently worth applying to; the UI says so rather than implying a cutoff.
2. **Semantic similarity is a weak signal.** Capped at 0.15 weight for that reason.
3. **Soft skills are unfalsifiable.** Weighted 0.3 and never a blocker.
4. **Seniority titles are not comparable across companies.** Not modeled.
5. **No prediction of callbacks.** Nothing here estimates your odds of a response — that
   depends on referrals, timing, and the other applicants, none of which is observable here.
   Presenting a fit score as a probability of success would be dishonest.
6. **Your own data quality dominates.** A thin experience bank produces pessimistic scores;
   the UI nudges toward enriching accomplishments when coverage is driven by missing evidence
   rather than genuine gaps.
