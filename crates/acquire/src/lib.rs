//! Polite, guarded acquisition of job postings.
//!
//! A URL becomes durable bytes here. Semantics come later, in `jobseeker-extract`. The
//! contract is: persist the raw body first, then think. An extraction bug is then a
//! re-run over history, not a lost posting (`docs/05-ingestion.md`).
//!
//! Guardrails run in a fixed order so a failure always has one cause:
//!
//! 1. scheme allowlist
//! 2. SSRF (private, loopback, link-local, CGNAT, metadata)
//! 3. `robots.txt`
//! 4. per-host rate limit
//! 5. size and time caps
//!
//! Authenticated sites are not fetched server-side. LinkedIn and Indeed require a login,
//! and the browser extension is the path for those (`docs/adr/0004-extension-capture.md`).

pub mod fetch;
pub mod rate;
pub mod robots;
pub mod ssrf;

use jobseeker_core::config::AcquireConfig;
use jobseeker_core::domain::capture::CaptureMethod;
use jobseeker_core::domain::enums::SourceKind;
use jobseeker_core::{Error, Result};
use jobseeker_normalize::url::{ats_api_url, canonicalize, CanonicalUrl};

pub use fetch::{FetchOutcome, Fetcher};
pub use rate::HostLimiter;
pub use robots::RobotsCache;
pub use ssrf::{is_blocked_ip, Guard};

/// What acquisition decided to do with a URL, before any bytes move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub canonical: CanonicalUrl,
    /// The URL that will actually be requested. For a known ATS this is the JSON API,
    /// which is higher fidelity and cheaper than the HTML (`docs/05-ingestion.md` §3).
    pub fetch_url: String,
    pub method: CaptureMethod,
    pub source: SourceKind,
}

/// Decide how to acquire a URL. Does not touch the network.
pub fn plan(input: &str) -> Result<Plan> {
    let canonical = canonicalize(input)?;
    if canonical.source.requires_authentication() {
        return Err(Error::NeedsBrowser);
    }
    let (fetch_url, method) = match ats_api_url(&canonical) {
        Some(api) => (api, CaptureMethod::Api),
        None => (canonical.canonical.clone(), CaptureMethod::Http),
    };
    Ok(Plan {
        source: canonical.source,
        method,
        fetch_url,
        canonical,
    })
}

/// The runtime an ingest task holds: HTTP client, robots cache, rate limiter, SSRF policy.
pub struct Acquire {
    pub fetcher: Fetcher,
    pub robots: RobotsCache,
    pub limiter: HostLimiter,
    pub guard: Guard,
}

impl Acquire {
    pub fn new(cfg: &AcquireConfig) -> Result<Self> {
        Ok(Self {
            fetcher: Fetcher::new(cfg)?,
            robots: RobotsCache::new(cfg.respect_robots),
            limiter: HostLimiter::new(cfg.per_host_delay_ms),
            guard: Guard::new(cfg.allow_private_networks),
        })
    }

    /// Fetch a public URL, honouring every guardrail. The caller persists the body.
    pub async fn fetch_url(&self, input: &str) -> Result<FetchOutcome> {
        let planned = plan(input)?;
        self.guard.check(&planned.fetch_url)?;
        // Workday's CXS lives under `/wday/`, which many tenants disallow in robots.txt.
        // The user asked for the public job page; evaluate robots against that page and
        // then fetch the same JSON the careers SPA loads (`docs/05-ingestion.md` §3).
        let robots_url = if planned.source == SourceKind::Workday {
            planned.canonical.original.as_str()
        } else {
            planned.fetch_url.as_str()
        };
        if !self.robots.allows(&self.fetcher, robots_url).await? {
            return Err(Error::RobotsDisallowed(planned.canonical.canonical.clone()));
        }
        self.limiter.wait(&planned.fetch_url).await;
        self.fetcher.get(&planned.fetch_url, None, None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_public_company_page_is_fetched_as_html() {
        let p = plan("https://careers.acme.dev/jobs/platform?utm_source=x").unwrap();
        assert_eq!(p.method, CaptureMethod::Http);
        assert_eq!(p.source, SourceKind::CompanySite);
        assert_eq!(p.fetch_url, "https://careers.acme.dev/jobs/platform");
    }

    #[test]
    fn a_greenhouse_url_is_fetched_from_the_published_api() {
        let p = plan("https://boards.greenhouse.io/acme/jobs/5512034?gh_src=abc").unwrap();
        assert_eq!(p.method, CaptureMethod::Api);
        assert_eq!(p.source, SourceKind::Greenhouse);
        assert_eq!(
            p.fetch_url,
            "https://boards-api.greenhouse.io/v1/boards/acme/jobs/5512034?questions=false"
        );
        assert_eq!(p.canonical.source_job_id.as_deref(), Some("5512034"));
    }

    #[test]
    fn authenticated_aggregators_are_refused_in_favour_of_the_extension() {
        for url in [
            "https://www.linkedin.com/jobs/view/4123456789",
            "https://www.indeed.com/viewjob?jk=abc123",
            "https://www.glassdoor.com/job-listing/foo.htm",
        ] {
            let err = plan(url).unwrap_err();
            assert_eq!(err.code(), "needs_browser", "{url}");
        }
    }

    #[test]
    fn a_workday_url_is_fetched_from_the_cxs_api() {
        let p = plan(
            "https://zillow.wd5.myworkdayjobs.com/en-US/Zillow_Group_External/job/Remote-USA/Data-Scientist_P751219-2",
        )
        .unwrap();
        assert_eq!(p.method, CaptureMethod::Api);
        assert_eq!(p.source, SourceKind::Workday);
        assert_eq!(
            p.fetch_url,
            "https://zillow.wd5.myworkdayjobs.com/wday/cxs/zillow/Zillow_Group_External/job/Remote-USA/Data-Scientist_P751219-2"
        );
        assert_eq!(p.canonical.source_job_id.as_deref(), Some("P751219-2"));
    }

    #[test]
    fn a_non_http_url_is_rejected_before_any_guardrail() {
        assert_eq!(
            plan("ftp://example.com/job").unwrap_err().code(),
            "invalid_url"
        );
        assert_eq!(
            plan("javascript:alert(1)").unwrap_err().code(),
            "invalid_url"
        );
    }
}
