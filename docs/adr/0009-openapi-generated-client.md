# ADR-0009 — OpenAPI generated from handlers, TypeScript client generated from it

**Status:** accepted · **Date:** 2026-09-05

## Context

The API is the only interface (see [ADR-0007](0007-single-binary-spa.md)), consumed by the
web UI, the CLI, and the extension. Hand-written TypeScript interfaces mirroring Rust structs
drift the moment someone renames a field, and the failure shows up as a runtime `undefined`
in the browser rather than a build error.

## Decision

`utoipa` derives an OpenAPI 3.1 document from the actual handlers and DTO types, served at
`/openapi.json`. `openapi-typescript` generates `apps/web/src/api/generated.ts` from that
document. The generated file is committed, and CI fails if it differs from a fresh
generation.

## Rationale

- **Single source of truth is the Rust code**, which is where the behavior lives. A
  spec-first workflow would create a second artifact to keep in sync, and in practice the
  code wins those disagreements silently.
- **Drift becomes a build failure.** Renaming `salary_min` to `salary_min_cents` breaks
  `tsc` immediately, in CI, instead of producing a blank cell in the UI three weeks later.
- **The spec is useful beyond the SPA**: `curl` recipes, a Scalar/Swagger browser for
  exploration, and future clients get it for free.
- **Type-only generation, not a generated SDK.** `openapi-typescript` emits types; the
  fetch wrapper stays hand-written (~50 lines) so error envelopes, auth, tracing headers, and
  TanStack Query integration behave exactly as we want. Full SDK generators produce a large
  layer of code that fights the query library.
- **Committing the generated file** keeps `pnpm install && pnpm build` working without a
  running server, which matters for CI and for a fresh clone.

## Alternatives considered

| Option | Why not |
|---|---|
| Hand-written TS types | Free until it isn't. Drift is silent and the resulting bugs are the hardest kind to trace. |
| Spec-first (write OpenAPI, generate both sides) | Better for multi-team contracts. Overhead is not justified for one author, and Rust server generation from OpenAPI is weak. |
| `ts-rs` (emit TS directly from Rust types) | Simpler, no OpenAPI layer — genuinely tempting. Rejected because it only covers types, not routes, params, or status codes, and produces no artifact usable by non-TS clients or by an API browser. |
| gRPC / protobuf | Strong contract, poor browser story without a proxy, and overkill for a single-user JSON API. |
| tRPC | Requires TypeScript on both ends. Backend is Rust (CON-01). |

## Consequences

- **DTOs need `#[derive(ToSchema)]` and route annotations.** Mild boilerplate, and it doubles
  as inline documentation of status codes and examples.
- **The generated file is committed**, so PRs touching the API include a generated diff. Noisy
  but reviewable, and the diff is itself a useful summary of the contract change.
- **`just gen-client` requires a running server** (it fetches `/openapi.json`). Acceptable;
  the recipe boots one on an ephemeral port and shuts it down.
- **utoipa's annotations must be kept truthful.** They are not verified against runtime
  behavior. Mitigated by integration tests asserting real status codes and by treating a
  mismatch as a bug of the same severity as a wrong response.
