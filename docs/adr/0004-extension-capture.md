# ADR-0004 — A browser extension is the primary path for authenticated sites

**Status:** accepted · **Date:** 2026-09-05

## Context

The stated goal is to "use my login on sites like Indeed and LinkedIn" to convert postings
into structured data. Those sites require authentication, render via JavaScript, employ bot
detection, and prohibit automated collection in their terms of service. The naive
implementations are (a) store the user's credentials and log in server-side, or (b) drive a
headless browser with the user's session cookies.

## Decision

A Manifest V3 browser extension is the primary acquisition path for authenticated sites. It
runs in the browser the user is already logged into, captures the rendered DOM of the page
they are currently viewing **on an explicit click**, and POSTs it to the local server with a
scoped device token. The server holds no third-party credentials and initiates no
authenticated requests.

An optional local headless-browser path exists (`acquire.browser.enabled`), is **off by
default**, and is documented as ToS-sensitive.

## Rationale

- **It satisfies the actual requirement.** The user's login *is* used — inside their own
  browser, as themselves. The data lands structured on their server.
- **No credential risk.** Storing a LinkedIn password on a home server is a high-value,
  low-reward target. CON-05 forbids it, and this design makes the prohibition free rather
  than a sacrifice.
- **It solves JS rendering for free.** LinkedIn and Workday are SPAs; their content only
  exists post-hydration. The browser has already done that work.
- **It sidesteps bot detection entirely.** No new request reaches the site, so there is
  nothing to fingerprint, no CAPTCHA to solve, and no arms race to lose. This is the
  difference between a feature that works for years and one that breaks every few months.
- **It is a materially different act from crawling.** One page, one user, one click, no
  additional load, no bulk collection, no redistribution. See
  [13-security-privacy-legal.md](../13-security-privacy-legal.md).
- **It keeps the intelligence server-side.** The extension is deliberately dumb — it ships
  HTML and does not parse. Improving an adapter never requires shipping a new extension
  version, and every historical capture can be re-extracted with better logic.

## Alternatives considered

**Server-side login with stored credentials.** Rejected: violates CON-05, creates a
high-value secret, breaks on MFA, and is unambiguously the automation those terms of service
prohibit.

**Headless browser with the user's cookie jar.** Technically effective and considered
seriously. Rejected as default because it is automation against an authenticated session
(the thing most likely to get an account flagged), because cookie extraction from a browser
profile is fragile and invasive, and because it needs a ~150 MB browser on the server.
Retained as an opt-in for users who accept the tradeoff.

**Manual copy-paste only.** Zero risk and it is kept as a permanent fallback (A3), but the
friction is high enough that it would not be used daily, and pasting loses the DOM structure
and embedded JSON that adapters rely on.

**Third-party job-data APIs.** Cost money, cover a fraction of postings, and route your
search through someone else's server — contrary to the local-first principle.

## Consequences

- **Users must install an extension**, which is friction and, on Chrome, requires developer
  mode or a store listing. Mitigated by a Firefox build (easier self-signing), a clear
  pairing flow (`jobseeker pair`), and the fact that the URL and paste paths still work
  without it.
- **Capture is manual per posting.** This is a real limitation: no bulk save of a search
  results page in M1. Mitigated later by a "capture all visible job cards" action on results
  pages, which still requires the user's click.
- **Two clients to maintain** (extension + web UI). Kept small: the extension is a content
  script, a background worker, and an options page — no framework, no build step beyond
  bundling.
- **CORS and token handling** are extra surface. Handled with a dedicated `ingest`-scoped
  token and an explicit origin allowlist, so a leaked device token cannot read the user's
  data.
- **The extension breaks if a site changes its DOM container heuristics.** Mitigated by
  capturing the whole `documentElement` rather than a selected subtree, so a container
  heuristic failure degrades the extraction rather than losing the page.
