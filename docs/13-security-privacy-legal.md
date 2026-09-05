# Security, privacy & legal posture

This document records deliberate positions, not legal advice. Read it before changing the
acquisition layer, because several constraints here exist to keep the project defensible and
are not merely technical preferences.

## 1. Threat model

Single-user, self-hosted, on a home LAN. What we defend against, in priority order:

| Threat | Mitigation |
|---|---|
| Accidental exposure to the internet | binds `127.0.0.1` by default; explicit config required to widen (NFR-S-06); startup logs a warning when bound broadly with `auth.mode = "none"` |
| Credential theft from the app | no third-party site passwords are ever stored (CON-05); own password is Argon2id; tokens stored hashed |
| Leaked device token | scoped to `ingest` only; cannot read jobs, resumes, or applications; revocable per device |
| SSRF via user-supplied URL | IP-range denylist re-checked after every redirect, connection pinned to the validated IP (NFR-S-05) |
| Stored XSS from captured HTML | raw captures are never rendered in the app origin; sanitized + restrictive CSP + `sandbox` (NFR-S-08) |
| Sensitive data leaking to a third party | local-first LLM default; a prominent, dismissible-once warning when a cloud provider is selected (NFR-S-07) |
| Disk theft | out of scope for the app; use full-disk encryption |
| Malicious file upload | size caps, content-type checks, no execution path, resumes parsed in-process without shelling out |

Explicitly **not** in the threat model: multi-tenant isolation, hostile authenticated users,
side-channel resistance. This is a personal tool.

## 2. Authentication

Three modes, chosen by deployment context:

- `none` — no auth. Only sane on a Tailscale-only bind. Logged as a warning at startup.
- `token` — static bearer tokens. Good for tailnet + CLI + extension.
- `password` — Argon2id (m=19 MiB, t=2, p=1), session cookie (`HttpOnly`, `SameSite=Lax`,
  `Secure` behind TLS), 30-day expiry, revocable. Login is rate-limited (10 / 15 min per IP)
  with exponential lockout, and responses are constant-time to avoid username enumeration.

Session tokens are 256-bit random, stored as BLAKE3 hashes — a database dump does not yield
usable sessions. Device tokens are prefixed `jst_` so they are recognizable in logs and
scannable by secret-detection tooling.

## 3. Secrets

- Loaded from env or a `0600` file; never from the data directory.
- Never logged: the config's `Debug` impl redacts, and API responses expose only
  `{provider, model, available}` — never the key.
- `/api/v1/meta` deliberately omits anything sensitive so it can stay unauthenticated.
- Rotating an LLM key requires no data migration; rotating the session key invalidates
  sessions by design.

## 4. Privacy

Your job search reveals that you are looking, where, for how much, and what you are bad at.
Treat it as sensitive.

- **Default is zero egress** beyond the job URLs you point it at. No telemetry, no analytics,
  no crash reporting, no update checks. There is no phone-home code path to disable.
- **LLM egress is explicit.** With `llm.provider = "ollama"` (default) nothing leaves the
  machine. Selecting a cloud provider prints what will be sent (job descriptions, and for
  resume generation your accomplishments) and requires a config change, not a UI toggle.
- **Per-purpose provider selection** so you can, for example, use a cloud model for job
  extraction but keep resume content local.
- **Deletion is real.** `DELETE /api/v1/jobs/:id?purge_files=true` removes rows and files;
  `DELETE /api/v1/admin/llm-calls` purges prompt/response logs (which contain job text).
- **No third-party fonts, CDNs, or trackers** in the web UI. Everything is served from the
  binary.

## 5. Legal posture on acquisition

Reasonable people can hold different views here. This project takes a conservative one and
encodes it in code, not just prose.

### What the project does

1. **Extension capture is the primary path for authenticated sites.** The extension reads a
   page *you* have already loaded in *your* browser under *your* session, at *your* explicit
   click, and stores a copy locally for your personal use. This is materially different from
   automated crawling: there is no additional request to the site, no circumvention of access
   controls, no rate impact, and no bulk collection.
2. **`robots.txt` is honored for all server-side fetches** (FR-A-05), and a disallow produces
   a clear "use the extension instead" message rather than a silent workaround.
3. **Rate limiting and a truthful User-Agent** on every server-side request. We do not
   pretend to be a browser we are not.
4. **Public ATS APIs are preferred** where they exist — those endpoints are published for
   programmatic consumption.
5. **No background crawling of sites requiring login** (FR-A-06). There is no code path that
   logs into LinkedIn or Indeed, and adding one would violate CON-05.
6. **No credential storage** for third-party sites, ever.
7. **No redistribution.** Data stays in your data directory. There is no sharing, publishing,
   or syndication feature, and no plan for one.

### What you should know

- LinkedIn's and Indeed's terms of service prohibit scraping and automated data collection.
  Saving a page you are viewing for personal reference is a different act from crawling, but
  **their ToS govern your account**, and a sufficiently annoyed platform can suspend it.
  That risk is yours; the design minimizes it (no extra traffic, no automation) but cannot
  eliminate it.
- Case law in this area (*hiQ v. LinkedIn*, *Van Buren v. United States*) has generally
  distinguished access to publicly available data from CFAA-relevant unauthorized access, but
  it does not immunize a ToS violation, and it is jurisdiction- and fact-specific.
- Job postings are typically factual content with thin copyright protection, but the text is
  the employer's. Personal, non-redistributed storage is the low-risk use; republishing an
  aggregated database is not, and this project does not enable it.
- The optional headless-browser path (`acquire.browser.enabled`, default **off**) is more
  ToS-sensitive because it is automation against a logged-in session. It is off by default,
  documented as such, never enabled by a migration or an update, and the config key requires
  acknowledging a warning.

### Rules for contributors

Any change to `jobseeker-acquire` must preserve all of the following. A PR that weakens one
should be rejected on principle, not debated case-by-case:

- no storage or transmission of third-party credentials;
- `robots.txt` respected for server-initiated fetches;
- per-host rate limiting cannot be disabled below a floor;
- no default-on automation against authenticated sessions;
- no feature that redistributes captured content.

## 6. Data retention

Configurable, with sane defaults:

| Data | Default | Rationale |
|---|---|---|
| Job records + files | forever | the point of the tool |
| Raw captures | forever (content-addressed, deduped) | evidence after a posting is pulled |
| Screenshots | 180 days | large, rarely needed after the posting closes |
| `llm_call` cache | 90 days | cache value decays; contains job text |
| `event_log` | 30 days | SSE backfill only |
| Sessions | 30 days | |
| Failed tasks | 30 days after terminal | keeps the error for diagnosis |
| Backups | 14 snapshots | |

## 7. Dependency hygiene

- `cargo deny` in CI: advisories, license allowlist, duplicate-version checks.
- `pnpm audit` for the web app; `--frozen-lockfile` everywhere.
- No `build.rs` that fetches from the network.
- rustls, not OpenSSL — no system TLS dependency to keep patched.
- Dependency review is part of PR review; a new transitive dependency tree for a small
  convenience is a reason to say no.

## 8. Responsible use

The generation features (`jobseeker-resume`) are constrained to *select and rephrase what is
in your experience bank* (FR-G-01), with automated post-generation validation that every
number and technology in a generated bullet appears in the source record. This is a product
decision as much as an ethical one: a resume you cannot defend in an interview is worse than
no resume. The system will not invent experience, and there is no configuration flag to make
it do so.

Similarly, there is no bulk auto-apply. Getting *ready* to apply is automated; deciding to
apply is not.
