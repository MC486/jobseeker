# Jobseeker documentation

Read in this order if you are new to the project. If you are the next agent picking up
implementation, start with [17-next-steps.md](17-next-steps.md).

## Product

| Doc | What it answers |
|---|---|
| [00-vision.md](00-vision.md) | What this is, the principles it is built on, and what it explicitly is not |
| [01-features.md](01-features.md) | The full feature catalog, tagged by milestone |
| [02-requirements.md](02-requirements.md) | Numbered functional/non-functional requirements, constraints, and the 10 acceptance scenarios |
| [16-roadmap.md](16-roadmap.md) | Milestones M0–M4 with exit criteria and sequencing rules |

## Design

| Doc | What it answers |
|---|---|
| [03-architecture.md](03-architecture.md) | System context, crate topology, runtime structure, extension points, cross-cutting concerns |
| [04-data-model.md](04-data-model.md) | Every table, why it exists, indexes, FTS, derived fields |
| [05-ingestion.md](05-ingestion.md) | URL canonicalization, fetch guardrails, extension capture, refresh, dedupe |
| [06-extraction.md](06-extraction.md) | The extraction stage order, adapters, requirement atomization, skill taxonomy, normalizers |
| [07-matching.md](07-matching.md) | Per-requirement verdicts, subscores, explainability, gap leverage, embeddings |
| [08-resume.md](08-resume.md) | Experience bank, accomplishment selection, constrained generation, versioning |
| [09-api.md](09-api.md) | The HTTP surface and the rules it follows |
| [10-frontend.md](10-frontend.md) | Stack, state discipline, routes, key screens, UX principles |
| [11-file-layout.md](11-file-layout.md) | Data directory, `job.md`/`job.json` formats, atomicity, reconcile |

## Operations

| Doc | What it answers |
|---|---|
| [12-deployment.md](12-deployment.md) | Building, systemd, network exposure, backups, upgrades, monitoring, sizing |
| [13-security-privacy-legal.md](13-security-privacy-legal.md) | Threat model, auth, privacy defaults, the acquisition posture and its rules |
| [14-performance.md](14-performance.md) | Budgets, SQLite tuning, indexes, async discipline, benchmarking |
| [15-testing.md](15-testing.md) | Test layers, fixtures, mock LLM, eval harness, CI gates |

## Decisions

Architecture decision records — the reasoning behind choices that would be expensive to
reverse. Read these before proposing a change to the corresponding area.

| ADR | Decision |
|---|---|
| [0001](adr/0001-rust-axum.md) | Rust + axum for the backend |
| [0002](adr/0002-sqlite-primary.md) | SQLite as the primary datastore (and why not MongoDB or DuckDB) |
| [0003](adr/0003-files-and-db.md) | Files *and* database, with files as the durable artifact |
| [0004](adr/0004-extension-capture.md) | Browser extension as the primary path for authenticated sites |
| [0005](adr/0005-llm-local-first.md) | LLM behind a trait, local-first, deterministic stages first |
| [0006](adr/0006-sqlite-queue.md) | SQLite-backed task queue with in-process workers |
| [0007](adr/0007-single-binary-spa.md) | Single binary with an embedded SPA behind a JSON API |
| [0008](adr/0008-typst-rendering.md) | Typst for resume rendering |
| [0009](adr/0009-openapi-generated-client.md) | OpenAPI generated from handlers, TS client generated from it |

## Handoff

| Doc | What it answers |
|---|---|
| [17-next-steps.md](17-next-steps.md) | Exactly what to build next, in order, with file paths and acceptance checks |
| [../AGENTS.md](../AGENTS.md) | Conventions, invariants, and rules for anyone (human or agent) writing code here |
