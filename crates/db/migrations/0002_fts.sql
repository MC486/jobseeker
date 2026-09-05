-- Full-text search.
--
-- FTS5 contentless tables: only the inverted index is stored, so there is exactly one copy
-- of the text. Contentless (rather than external-content) is required because our primary
-- keys are TEXT while FTS5 needs an INTEGER rowid; `job.fts_rowid` is the surrogate.
--
-- `requirements_text` is a materialized concatenation of the job's atomized requirements, so
-- searching "Kubernetes" still hits a posting where the word only survives in a requirement
-- row.

CREATE VIRTUAL TABLE job_fts USING fts5(
    title,
    company_name,
    description_text,
    requirements_text,
    content = '',
    tokenize = "porter unicode61 remove_diacritics 2"
);

CREATE VIRTUAL TABLE accomplishment_fts USING fts5(
    text,
    org,
    title,
    content = '',
    tokenize = "porter unicode61 remove_diacritics 2"
);

-- Monotonic source for the integer surrogates. A plain table rather than AUTOINCREMENT so
-- the same counter can serve several FTS tables and be inspected.
CREATE TABLE fts_rowid_seq (
    name  TEXT PRIMARY KEY,
    value INTEGER NOT NULL
) STRICT;

INSERT INTO fts_rowid_seq (name, value) VALUES ('job', 0), ('accomplishment', 0);

-- Contentless FTS5 rows cannot be updated in place: a change is delete-then-insert, and the
-- delete must repeat the *old* column values. Rather than encode that in triggers (which
-- cannot see the old requirements aggregate), the repository layer owns FTS synchronization
-- in the same transaction as the write. `reconcile --rebuild-derived` reindexes from scratch
-- if the two ever diverge.
--
-- Ranking uses bm25 with title and company weighted up:
--   ORDER BY bm25(job_fts, 10.0, 6.0, 1.0, 3.0)
