# jobseeker

Self-hosted job-search workbench. You ingest postings you are already looking
at; the server turns them into structured, queryable, durable data. Matching,
resume writing and gap analysis sit on top of that record — they are not the
first thing that has to work.

The server never stores LinkedIn or Indeed credentials. Those sites are
captured by a browser extension from a page you are already viewing.

## Status

Walking skeleton (M0). One posting can travel paste/URL → capture → extract →
SQLite + `job.md` / `job.json` / `requirements.json` → job list.

See [docs/](docs/README.md) for the architecture and [AGENTS.md](AGENTS.md) for
invariants.

## Quick start

```bash
# defaults: 127.0.0.1:8787, ./data/jobseeker.db, no LLM
cargo run -p jobseeker-cli -- serve
```

In another terminal:

```bash
# Public ATS page (Greenhouse, Lever, …)
cargo run -p jobseeker-cli -- add 'https://boards.greenhouse.io/acme/jobs/123'

# Or paste saved HTML (the hermetic path, and the one tests use)
cargo run -p jobseeker-cli -- add --paste ./fixture.html
cargo run -p jobseeker-cli -- list
```

LinkedIn / Indeed URLs are refused with `needs_browser`. Load `extension/` as
an unpacked MV3 extension and capture the tab you have open.

Copy [jobseeker.example.toml](jobseeker.example.toml) to `jobseeker.toml` to
override defaults, or set `JOBSEEKER__SERVER__BIND`, `JOBSEEKER__DATA__DIR`,
`JOBSEEKER__LLM__PROVIDER`, …

## Layout

```
crates/     Rust workspace (core → db → pipeline → api → cli)
web/        React + Vite + Tailwind UI
extension/  Manifest V3 capture extension
docs/       Product and design docs
```

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cd web && npm install && npm run build
just gen-client   # dump OpenAPI → web/src/api/generated.ts
```
