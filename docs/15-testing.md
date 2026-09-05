# Testing strategy

The architecture exists partly to make testing cheap: `normalize` and `matching` are pure,
so the logic most likely to be wrong needs neither a database nor a network.

## Layers

| Layer | Location | Scope | Speed |
|---|---|---|---|
| Unit | `crates/*/src/**/#[cfg(test)]` | pure functions: salary/date/location parsing, atomization rules, scoring math, URL canonicalization | ms |
| Golden / fixture | `crates/extract/tests/fixtures/` | real captured HTML → expected JSON | ms |
| Repository | `crates/db/tests/` | SQL against a temp-file SQLite with migrations applied | tens of ms |
| Integration | `crates/api/tests/` | the real axum app + real DB + mock LLM, over HTTP | hundreds of ms |
| Eval | `crates/extract/tests/eval.rs` + `eval/baseline.json` | extraction quality metrics vs a committed baseline | seconds |
| E2E | `apps/web/e2e/` (Playwright) | browser against a seeded server | tens of seconds |
| Bench | `crates/*/benches/` (criterion) | performance budgets | seconds |

## What gets tested where

**Pure logic (unit).** Every parser gets a table-driven test with the ugly real-world cases,
not the happy path:

```rust
#[test]
fn parses_salary_variants() {
    let cases = [
        ("$120,000 - $150,000 a year", sal(120_000_00, 150_000_00, "USD", Year, false)),
        ("120k-150k",                  sal(120_000_00, 150_000_00, "USD", Year, false)),
        ("$60/hr",                     sal(60_00, 60_00, "USD", Hour, false)),
        ("£70,000 per annum",          sal(70_000_00, 70_000_00, "GBP", Year, false)),
        ("€90.000 – €110.000",         sal(90_000_00, 110_000_00, "EUR", Year, false)),
        ("Estimated: $95K - $120K a year", sal(95_000_00, 120_000_00, "USD", Year, true)),
        ("up to $200k",                sal_max(200_000_00, "USD", Year)),
        ("Competitive",                None),
        ("DOE",                        None),
        ("$5",                         None),          // below sanity floor
    ];
    for (input, expected) in cases { assert_eq!(parse_salary(input, None), expected, "{input}"); }
}
```

**Extraction (golden).** `crates/extract/tests/fixtures/<source>/<case>/` holds
`input.html`, `meta.json`, `expected.json`. The test runs the full pipeline with a mock LLM
and asserts field equality plus requirement set F1. `UPDATE_FIXTURES=1 cargo test` rewrites
expectations, and the resulting diff is reviewed by a human — never accepted blindly.

Fixture corpus targets: ≥5 cases each for LinkedIn, Indeed, Greenhouse, Lever, Ashby,
Workday, plus ≥10 generic company sites. Every parsing bug fixed adds a fixture
(NFR-R-05) — that is the mechanism that keeps adapters from regressing as sites change.

Fixtures are scrubbed: no recruiter names, no personal emails, no session identifiers.

**Repositories.** Each repo test opens a fresh temp-file SQLite (not `:memory:` — WAL
behavior and file locking differ, and those are exactly what we depend on), runs migrations,
and exercises upsert/conflict/cascade paths. Specifically covered: `ON CONFLICT` upsert
idempotency, cascade deletes leaving no orphans, `url_hash` uniqueness, provenance
protection of `manual` fields, and FTS trigger sync.

**Integration.** `tests/common/harness.rs` boots the real router with a temp data dir and a
`MockLlm` returning canned schema-valid responses. Tests drive real HTTP and then drain the
queue synchronously (`harness.run_queue_to_completion()`), which makes the async pipeline
deterministic in tests without special-casing production code.

The ten acceptance scenarios in [Requirements §4](02-requirements.md) are integration tests,
one per scenario, named `as01_public_posting_no_llm` and so on, so a failing test maps
directly to a violated requirement.

**Migrations.** For every migration: apply to empty, apply to the previous release's seeded
snapshot (checked-in `tests/snapshots/schema-vN.sql`), assert no data loss and that
`reconcile --check` reports zero drift afterwards (NFR-R-04).

**Property tests** (`proptest`) where invariants are more valuable than examples:

- salary round-trip: format(parse(x)) parses to the same value;
- URL canonicalization is idempotent: `canon(canon(u)) == canon(u)`;
- match scores stay in `[0,1]` for arbitrary requirement/profile combinations;
- deterministic serialization: `serialize(deserialize(json)) == json` byte-for-byte;
- requirement dedup is idempotent and order-independent.

**E2E** covers the flows a unit test cannot: paste a URL and watch the row populate via SSE;
edit a requirement and see the match update; generate a resume and download the PDF; drag an
application across the kanban. Kept deliberately small — a handful of high-value paths, run
against a seeded database.

## Mock LLM

```rust
pub struct MockLlm { responses: HashMap<String, Value>, pub calls: Arc<Mutex<Vec<LlmCall>>> }
```

Keyed by prompt purpose + a hash of the input. Returns schema-valid canned JSON, records
calls so tests can assert **that the LLM was not called** when deterministic stages
sufficed — a cost regression is a real regression, and it is otherwise invisible.

A `record` mode hits a real local model once and writes the response into the fixture, so
adding a case does not mean hand-writing JSON.

## Eval harness

```
$ just eval
source        cases  title  company  salary  location  posted  req-F1  blockers
linkedin          8   1.00     1.00    0.88      0.88    0.75    0.81      1.00
indeed            6   1.00     0.83    0.83      1.00    0.83    0.74      1.00
greenhouse        7   1.00     1.00    1.00      1.00    1.00    0.89      1.00
generic          11   0.91     0.82    0.73      0.82    0.64    0.68      0.91
─────────────────────────────────────────────────────────────────────────────────
overall          32   0.97     0.91    0.85      0.91    0.79    0.78      0.97
baseline         32   0.97     0.91    0.85      0.88    0.79    0.76      0.97
                                              +0.03            +0.02
```

CI fails on any metric regressing more than 0.02 from `eval/baseline.json`. Updating the
baseline is a deliberate, reviewed commit. This is the only defense against a prompt tweak
that improves one case and quietly breaks six.

## CI

```
1. cargo fmt --check
2. cargo clippy --all-targets --all-features -- -D warnings
3. cargo test --workspace
4. cargo deny check
5. sqlx migration test (empty + upgrade paths)
6. just eval                          (fails on regression)
7. pnpm -C apps/web install --frozen-lockfile
8. pnpm -C apps/web typecheck && lint && build
9. openapi drift check                (generated client must match committed)
10. size-limit                        (initial bundle ≤ 200 KB gz)
11. cargo build --release --features embed-web   (main branch only)
```

Steps 1–8 must pass on every PR. Benches run nightly on a fixed runner; E2E runs on PRs
touching `apps/web` and nightly.

## Coverage expectations

Not a percentage target — a list of things that must have tests:

- every field parser (salary, location, date, seniority, education, clearance);
- every site adapter, with ≥1 "site changed its markup" degradation case;
- every requirement classification rule;
- every scoring component, including the blocker cap and weight renormalization;
- every task handler's idempotency (run twice → same state);
- every migration;
- the provenance precedence table;
- reconcile round-trip (AS-06);
- resume bullet validation (no invented numbers or technologies — FR-G-01).

## Manual test checklist (pre-release)

1. Ingest one real posting from each of: LinkedIn, Indeed, Greenhouse, Workday, a random
   company site.
2. Verify salary, location, dates, and requirement quality by eye; correct anything wrong
   and confirm the correction survives a re-extract.
3. Delete `jobseeker.db`, run `reconcile --from-files`, confirm zero drift.
4. Generate a resume; read it as an employer would; verify every bullet against the bank.
5. Restart mid-pipeline (`kill -9` during extraction) and confirm the task resumes.
6. Run with `llm.provider = "none"` and confirm the app stays fully usable.
