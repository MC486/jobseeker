# ADR-0005 — LLM behind a trait, local-first, deterministic stages first

**Status:** accepted · **Date:** 2026-09-05

## Context

Turning prose postings into typed requirements is the core transform, and an LLM does it far
better than rules. But job-search and resume data are sensitive (NFR-S-07), API calls cost
money and add latency, model output is non-deterministic, and the app must remain fully
usable with no model at all (FR-E-11, CON-04).

## Decision

1. All model access goes through two traits, `Completer` and `Embedder`, in
   `jobseeker-llm`. Providers: `ollama` (default), `openai`, `anthropic`, `mock`, `none`.
2. Extraction runs deterministic stages first and invokes the model **only for fields still
   missing** and for requirement atomization.
3. All calls are JSON-schema-constrained, validated before use, and cached by
   `blake3(provider|model|schema|prompt|params)` in the `llm_call` table.
4. `temperature = 0`, pinned seeds where supported.
5. With `llm.provider = "none"` or an unreachable provider, ingestion still succeeds and the
   job is marked `extraction_partial`.
6. Provider is selectable per purpose, so job extraction and resume writing can use different
   models — or different trust boundaries.

## Rationale

- **Privacy default matters more than quality here.** Sending every posting you look at, plus
  your full work history, to a third party is a meaningful disclosure. A 14B local model
  handles "classify this bullet as required vs preferred and extract the year count" well;
  that is not a frontier-model task.
- **Cost and latency.** JSON-LD on a Greenhouse page yields a complete record for zero
  tokens. Only asking about missing fields turns most ingestions into either no call or one
  small call.
- **Determinism is a testability requirement.** The cache plus `temperature = 0` makes
  re-extraction reproducible, which is what allows golden fixtures and an eval baseline to
  exist at all.
- **Schema constraint plus validation is the difference between a parser and a rumor.**
  Rejecting an invalid response wholesale (never partially applying it) prevents a malformed
  generation from writing a garbage salary.
- **The trait boundary is small and stable.** Provider churn in this space is fast; two
  methods is a cheap insurance policy.

## Alternatives considered

| Option | Why not |
|---|---|
| LLM-first for everything | Simpler code, uniform pipeline, better raw accuracy on messy pages. Rejected: it makes every ingestion cost tokens, discards free high-confidence structured data (JSON-LD/ATS APIs are strictly better than a model reading rendered text), and makes the system unusable offline. Retained as a debug mode for eval comparison. |
| Rules only, no LLM | Requirement atomization and necessity classification are where rules plateau badly — the prose is too varied. The product's central value would be weak. |
| Cloud-only (OpenAI/Anthropic) | Best quality, no local GPU needed. Rejected as *default* on privacy; fully supported as opt-in. |
| Fine-tuned local model | Attractive later (backlog), needs a labeled corpus first — which the manual-correction loop is quietly building. |
| Function calling / tool use | Unnecessary indirection; we want one JSON object back, and constrained decoding gives that directly. |

## Consequences

- **Extraction quality depends on the user's local model.** Documented recommendations
  (Qwen2.5 14B instruct or similar) and the eval harness lets a user measure their own setup
  rather than guess.
- **Two code paths** (with and without a model) must both stay working. Covered by AS-10 and
  by a CI run with `llm.provider = "none"`.
- **The cache table grows** and contains job text. Retention default 90 days, purgeable via
  the admin endpoint.
- **Prompt changes invalidate the cache** (the schema and prompt are in the hash key), so a
  prompt tweak triggers re-extraction cost. Intentional: a stale cache keyed on an old prompt
  would be worse.
- **Cost accounting is built in** (`prompt_tokens`, `completion_tokens`, `cost_micros`) so
  cloud usage is visible rather than a surprise.
