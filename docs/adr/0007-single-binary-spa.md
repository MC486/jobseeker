# ADR-0007 — Single binary with an embedded SPA, behind a JSON API

**Status:** accepted · **Date:** 2026-09-05

## Context

The UI needs to be an interactive workbench: dense virtualized tables, live pipeline
progress, keyboard-driven navigation, inline editing, filter state in the URL. It also needs
to deploy onto a home server without a reverse-proxy-plus-static-host arrangement, and the
CLI needs the same capabilities as the UI.

## Decision

A React SPA built by Vite, embedded into the Rust binary via `rust-embed` under the
`embed-web` feature, served from the same origin as the API. The API is versioned JSON at
`/api/v1` and is the **only** interface — the web UI, the CLI, and the extension are all
clients of it.

## Rationale

- **One artifact to deploy.** `scp jobseeker && systemctl restart` is the entire upgrade.
  No asset directory to keep in sync with the binary, no nginx root to configure, no
  possibility of a version mismatch between UI and API.
- **Same-origin means no CORS** in the normal path, and the session cookie works without
  `SameSite` gymnastics. The only CORS exception is the extension's explicit allowlist.
- **API-only forces a complete API.** If the UI could reach around the API via server-side
  rendering, the CLI and any future client would be second-class. Making the SPA a pure
  client guarantees CLI parity (FR-I-06) is achievable rather than aspirational.
- **The interaction model wants a client-side app.** Optimistic kanban drags, a command
  palette, live SSE-driven row updates, and 10k-row virtualized tables are all client-state
  problems. Server-rendered HTML with partial swaps would fight the grain of every one.
- **Filters in the URL** give shareable views, working back button, and reload-safety for
  free — but only if the client owns routing.

## Alternatives considered

| Option | Why not |
|---|---|
| Server-rendered templates (askama/maud) + HTMX | Genuinely appealing: no build step, no JS bundle, less total code. Rejected because the dense-table + live-progress + palette + inline-edit combination is where HTMX stops being simpler, and because it would tempt the UI to bypass the API. |
| Separate static host for the SPA | Adds a deployment component and a CORS/auth story for no benefit on a single-server deployment. |
| Tauri/desktop app | Would not be reachable from the phone or another laptop, which is a real use (checking the pipeline away from the desk). |
| Next.js / SvelteKit with SSR | Adds a Node runtime to the server, contradicting the single-binary goal, for SEO and first-paint benefits that are irrelevant on a LAN with one user. |
| Serve `dist/` from disk instead of embedding | Simpler builds, and this is exactly what dev mode does. Rejected for production because it reintroduces the two-artifact sync problem. `embed-web` is a feature flag, so the disk path remains available. |

## Consequences

- **Binary size grows** to roughly 15–25 MB. Irrelevant on a home server.
- **A UI change requires a Rust rebuild** for the embedded artifact. Mitigated by dev mode:
  Vite on 5173 proxying `/api` to the Rust server, so the frontend loop never rebuilds Rust.
- **The frontend build must run before the release build.** Enforced by the `just build`
  recipe and by CI ordering; `embed-web` fails loudly if `apps/web/dist` is missing rather
  than embedding an empty directory.
- **JS is required.** Acceptable for a personal workbench; there is no no-JS fallback and no
  plan for one.
- **The OpenAPI contract must stay accurate**, since the client is generated from it. Covered
  by the drift check in CI ([ADR-0009](0009-openapi-generated-client.md)).
