# ADR-0006 — SQLite-backed task queue, in-process workers

**Status:** accepted · **Date:** 2026-09-05

## Context

Ingestion must return in under 100 ms (NFR-P-01) while fetching, parsing, LLM calls,
embedding, scoring, and file materialization take seconds. That needs a queue with retries,
backoff, progress reporting, and crash recovery. CON-02 forbids running another service.

## Decision

A `task` table in the same SQLite database, claimed with a single atomic
`UPDATE … WHERE id = (SELECT … LIMIT 1) RETURNING *`, worked by a pool of tokio tasks inside
the server process. Leases (default 120 s) provide crash recovery; `dedupe_key` coalesces
duplicate work; retries use exponential backoff with jitter; handlers must be idempotent.

## Rationale

- **Transactional enqueue.** Enqueueing a task in the same transaction as the data it
  operates on removes the classic dual-write bug: a task can never reference a row that was
  rolled back, and a committed row can never be missing its task.
- **No extra moving parts.** A Redis or RabbitMQ dependency for a queue whose steady-state
  depth is single digits would be the largest operational cost in the system.
- **Free durability and inspection.** Tasks survive restarts, and `SELECT * FROM task WHERE
  status='failed'` is the entire debugging story. The UI's task inspector is a list query.
- **Leases beat acknowledgements for this workload.** A `kill -9` mid-extraction leaves a row
  with an expired lease that the scheduler returns to `queued`. No dead-letter plumbing, no
  visibility-timeout service.
- **`dedupe_key` matters in practice.** Refreshing a job five times, or a profile edit
  marking 5,000 scores stale, should not enqueue 5,000 duplicate rows. A unique key with
  `ON CONFLICT DO NOTHING` handles it in one statement.
- **Idle cost.** Workers wait on a `Notify` from the enqueuer with a 1 s fallback poll, so
  there is no busy loop and idle CPU is genuinely ~0 (NFR-P-05).

## Alternatives considered

| Option | Why not |
|---|---|
| Redis + a worker process | Standard and battle-tested. Rejected on CON-02: another daemon, another config, another failure mode, another backup concern — for a queue that peaks at tens of items. |
| In-memory channel only | Simplest, but loses queued work on restart and gives no visibility into failures or retries. Unacceptable when ingestion is asynchronous and the user expects the job to eventually appear. |
| Do the work synchronously in the request | Blows the 100 ms budget by orders of magnitude and makes a slow LLM call an HTTP timeout. |
| A separate worker binary against the same DB | Possible later, and the design does not preclude it (leases and atomic claims are already multi-process-safe). Rejected now: one process is simpler to deploy and there is no capacity reason to split. |
| `SKIP LOCKED`-style claiming | Not available in SQLite; the single-writer model makes the atomic-update claim equivalent and simpler. |

## Consequences

- **At-least-once, not exactly-once.** Every handler must be idempotent, keyed on content
  hashes and natural unique keys. This is enforced by a test per handler that runs it twice
  and asserts identical state.
- **Queue throughput is bounded by the single writer.** Fine at this scale (claims are tiny
  single-row updates); documented in the scaling-limits table.
- **Long handlers must renew their lease.** Handlers get a `LeaseGuard` that renews in the
  background; forgetting it means duplicate execution, so it is part of the handler trait's
  contract rather than an optional call.
- **Polling adds a small latency floor** when the notify path is missed. The 1 s fallback is
  invisible for work that already takes seconds.
- **No cross-machine scaling.** Explicitly out of scope (CON-03).
