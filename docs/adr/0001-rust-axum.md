# ADR-0001 — Rust + axum for the backend

**Status:** accepted · **Date:** 2026-09-05

## Context

The backend must run continuously on a home server with near-zero idle cost, do CPU-bound
work (HTML parsing, text normalization, vector math, set-cover optimization), and stay
correct across a long-lived local dataset. It was also specified as a constraint (CON-01).

## Decision

Rust, stable toolchain, edition 2021, with `axum` for HTTP on `tokio`.

## Rationale

- **Idle cost.** A single native binary sits at a few tens of MB RSS and ~0% CPU. A JVM or
  Node process idles an order of magnitude heavier on a box that is also doing other things.
- **CPU work is real here.** Parsing a 500 KB DOM, cosine over 10⁵ vectors, and scoring
  5,000 jobs are hot paths. Being able to do them in-process without a native-extension
  escape hatch keeps the architecture simple.
- **The type system pays for itself in this domain.** Provenance-tagged optional fields
  (`Option<Sourced<T>>`), typed ids, and exhaustive enum matching over requirement kinds
  catch precisely the errors this project is prone to — silently coercing a missing salary
  into `0`, or transposing a `JobId` and a `ProfileId`.
- **`axum`** is `tower`-based, so compression, tracing, timeouts, body limits, and rate
  limiting are composable middleware rather than bespoke code. `State` extraction gives
  clean dependency injection with no macros or globals.
- **`sqlx`** offers compile-time-checked SQL against SQLite, which keeps the SQL visible —
  important because tuning SQLite indexes and FTS is part of meeting the performance budgets.

## Alternatives considered

| Option | Why not |
|---|---|
| Python (FastAPI) | Fastest to prototype, and the extraction ecosystem is richer. But idle footprint, GIL-bound CPU work, and the runtime-error surface across a large evolving schema are all wrong for a service that runs unattended for months. |
| Go | Very close call: excellent for a single static binary and simpler concurrency story. Rust wins on the type-level modeling of optional provenance-tagged fields and on zero-cost enum exhaustiveness, which is a large share of this codebase's correctness. |
| Node/TypeScript | One language across stack is genuinely attractive. Rejected on idle memory, CPU-bound work, and the fact that heavy numeric/parsing paths would end up in native modules anyway. |
| actix-web | Comparable performance; `tower` middleware ecosystem and the `State` ergonomics of axum are a better fit. |

## Consequences

- Slower iteration on the extraction layer than Python would allow. Mitigated by keeping
  `normalize`/`extract` pure and fixture-driven, so the feedback loop is `cargo test` on a
  small crate rather than a full rebuild.
- Compile times require attention: the workspace is split into many small crates for
  parallelism, and heavy dependencies are feature-gated.
- Some Rust-ecosystem gaps (DOCX parsing, PDF text extraction) need more manual work than
  their Python equivalents. Accepted; both are small, bounded problems.
