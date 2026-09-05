# ADR-0003 — Files *and* database, with files as the durable artifact

**Status:** accepted · **Date:** 2026-09-05

## Context

The requirement was explicit: postings should become "individual files as well as a
machine-parsable database." Those pull in different directions — files are portable,
readable, and diffable; a database is queryable and normalized. Picking one loses something
real.

## Decision

Both, with a defined authority split:

- **SQLite is the authority for querying** and the working state (indexes, FTS, embeddings,
  task queue, scores).
- **The file tree is the authority for durability and portability.** Every job is
  materialized as a directory of files, and `jobseeker reconcile --from-files` must be able
  to rebuild the entire database from files alone (FR-S-04).

Regenerable data (match scores, embeddings, FTS index, derived counters) lives only in the
database and is recomputed after a rebuild.

## Rationale

- **The files are the backup you can actually verify.** A `.db` snapshot is opaque; a
  directory of Markdown and JSON can be read, grepped, diffed, and inspected on any machine
  without this software. Making rebuild-from-files an acceptance test (AS-06) means the
  backup provably works, rather than being assumed to.
- **Longevity.** This data has a decade-plus useful life (your career history). Betting it on
  one binary format is a worse bet than betting it on UTF-8 Markdown and JSON. If the project
  is abandoned, the data is still usable.
- **Git.** Because serialization is deterministic (sorted keys, LF, no embedded timestamps),
  the data directory can be a git repo. A posting's salary change becomes a one-line diff,
  and you get a free audit trail of your entire search.
- **`job.md` serves a human purpose the database cannot.** Reading a posting in an editor, or
  on a phone, or piping it into another tool, is a real daily activity.
- **Queries the file tree cannot serve.** "Open remote jobs above $180k where Kubernetes is a
  required gap for my default profile, sorted by match" is a five-way join. It needs SQL.

## Alternatives considered

| Option | Why not |
|---|---|
| Database only, with an export command | Export becomes an afterthought that silently rots. Making files the primary durable artifact forces them to stay complete. |
| Files only, grep as the query engine | Fails every performance budget and cannot express the joins the product is built on. |
| Files as source of truth, DB as a pure cache rebuilt on boot | Considered seriously. Rejected because boot-time rebuild of 10⁵ rows plus FTS is slow, and because live state (task queue, leases, sessions) genuinely belongs in a transactional store. |
| Git as the storage engine (commit per change) | Neat, but couples writes to git performance and makes concurrent writes awkward. Instead the data dir is *git-compatible* without git being required. |

## Consequences

- **Two write paths to keep consistent.** Mitigated by: the DB transaction commits first,
  then a `materialize_job` task writes files idempotently; a crash between them self-heals on
  the next reconcile; `reconcile --check` reports drift explicitly.
- **Serialization must be deterministic.** Sorted keys, LF endings, omitted nulls, no
  generation timestamp inside files. Enforced by a property test (round-trip byte equality) —
  otherwise every export churns the whole tree and diffs become useless.
- **Disk usage is higher** (raw captures dominate). Mitigated by zstd compression and
  content-addressed dedup via hard links.
- **The file format becomes a public contract at M1** (CON-07). Changing `job.json` later
  requires a versioned migration path, so the shape needs to be right before real data
  accumulates.
- **Deletion needs care.** Soft deletes are recorded so `reconcile --from-files` cannot
  resurrect something you deleted.
