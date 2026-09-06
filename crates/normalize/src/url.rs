//! URL canonicalization and source detection.
//!
//! The same posting reaches you through a LinkedIn feed link, an email tracking redirect,
//! and the company's own board. Reducing all of those to one identity string is what makes
//! `job_source_listing.url_hash` a usable unique key, and therefore what stops the job list
//! filling with duplicates (`docs/05-ingestion.md` §2).

use jobseeker_core::domain::enums::SourceKind;
use jobseeker_core::hash::content_hash;
use jobseeker_core::{Error, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use url::Url;

/// Query parameters that identify the *reader*, not the posting. Dropping them is what makes
/// two links to the same job compare equal.
const TRACKING_PARAMS: &[&str] = &[
    "gclid",
    "fbclid",
    "msclkid",
    "trk",
    "trkinfo",
    "refid",
    "originalsubdomain",
    "position",
    "pagenum",
    "ebp",
    "origin",
    "from",
    "source",
    "vjk",
    "tk",
    "sk",
    "ref",
    "referer",
    "referrer",
    "savedsearchid",
    "alid",
    "eid",
    "cid",
    "mid",
    "iis",
    "iisn",
    "advn",
    "adid",
    "sponsored",
    "hidesmartapply",
    "applied",
    "campaignid",
    "gh_src",
    "lever-source",
    "lever-origin",
    "source_id",
    "recruiter",
    "share",
    "shared",
    "spa",
    "seen",
];

/// A canonicalized URL plus the identity we deduplicate on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalUrl {
    /// The URL as given, untouched, so the UI can link where the user actually was.
    pub original: String,
    /// Identity form: what `url_hash` is computed over.
    pub canonical: String,
    pub source: SourceKind,
    /// The site's own id for this posting, when the URL encodes one. This is a far stronger
    /// dedup key than the URL, and it is how an aggregator's listing is matched to the ATS
    /// posting behind it.
    pub source_job_id: Option<String>,
    /// ATS board / company slug, when present.
    pub board: Option<String>,
}

impl CanonicalUrl {
    pub fn url_hash(&self) -> String {
        content_hash(self.canonical.as_bytes())
    }
}

/// Reduce a URL to its identity form.
///
/// Unknown hosts get the generic treatment only (scheme/host lowercasing, tracking
/// parameter removal). That is safe because `jobseeker reconcile` recomputes
/// `url_canonical`, so adding a host rule later re-deduplicates history.
pub fn canonicalize(input: &str) -> Result<CanonicalUrl> {
    let trimmed = input.trim();
    let mut url = Url::parse(trimmed).map_err(|e| Error::InvalidUrl(format!("{trimmed}: {e}")))?;

    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::InvalidUrl(format!(
            "only http and https are supported, got {:?}",
            url.scheme()
        )));
    }
    if url.host_str().is_none() {
        return Err(Error::InvalidUrl(format!("{trimmed} has no host")));
    }

    url.set_fragment(None);
    // Unwrap common redirect wrappers before anything else, so an emailed tracking link
    // canonicalizes to the same identity as the posting it points at.
    if let Some(inner) = unwrap_redirect(&url) {
        return canonicalize(&inner);
    }

    let host = normalize_host(url.host_str().unwrap_or_default());
    let _ = url.set_host(Some(&host));
    let _ = url.set_scheme("https");
    if url.port() == Some(443) || url.port() == Some(80) {
        let _ = url.set_port(None);
    }
    strip_tracking_params(&mut url);

    let path = url.path().trim_end_matches('/').to_string();
    let source = detect_source(&host);
    let identity = site_identity(&host, &path, &url);

    let canonical = match &identity {
        Some(id) => id.canonical.clone(),
        None => {
            // Generic form: no query unless it survived tracking-param stripping.
            let mut u = url.clone();
            u.set_path(if path.is_empty() { "/" } else { &path });
            u.to_string().trim_end_matches('/').to_string()
        }
    };

    Ok(CanonicalUrl {
        original: trimmed.to_string(),
        canonical,
        source: identity.as_ref().map(|i| i.source).unwrap_or(source),
        source_job_id: identity.as_ref().and_then(|i| i.job_id.clone()),
        board: identity.and_then(|i| i.board),
    })
}

/// Which site a host belongs to. Drives `source.fidelity`, and therefore which listing wins
/// a merge.
pub fn detect_source(host: &str) -> SourceKind {
    let h = host.trim_start_matches("www.");
    match h {
        _ if h.ends_with("linkedin.com") => SourceKind::LinkedIn,
        _ if h.ends_with("indeed.com") || h.contains("indeed.co") => SourceKind::Indeed,
        _ if h.ends_with("glassdoor.com") || h.contains("glassdoor.co") => SourceKind::Glassdoor,
        _ if h.ends_with("ziprecruiter.com") => SourceKind::ZipRecruiter,
        _ if h.ends_with("dice.com") => SourceKind::Dice,
        "boards.greenhouse.io" | "job-boards.greenhouse.io" | "boards-api.greenhouse.io" => {
            SourceKind::Greenhouse
        }
        _ if h.ends_with("greenhouse.io") => SourceKind::Greenhouse,
        _ if h.ends_with("lever.co") => SourceKind::Lever,
        _ if h.ends_with("ashbyhq.com") => SourceKind::Ashby,
        _ if h.ends_with("myworkdayjobs.com") || h.ends_with("myworkday.com") => {
            SourceKind::Workday
        }
        _ if h.ends_with("smartrecruiters.com") => SourceKind::SmartRecruiters,
        _ if h.ends_with("workable.com") || h.ends_with("applytojob.com") => SourceKind::Workable,
        _ if h.ends_with("recruitee.com") => SourceKind::Recruitee,
        _ => SourceKind::CompanySite,
    }
}

struct Identity {
    canonical: String,
    source: SourceKind,
    job_id: Option<String>,
    board: Option<String>,
}

/// Per-host rewrite rules. Each arm answers one question: *what is the smallest URL that
/// still names this exact posting?*
fn site_identity(host: &str, path: &str, url: &Url) -> Option<Identity> {
    static LINKEDIN_VIEW: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^/jobs/view/(?:[^/]*-)?(\d{6,})").unwrap());
    static GREENHOUSE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^/([^/]+)/jobs/(\d+)").unwrap());
    static LEVER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^/([^/]+)/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})")
            .unwrap()
    });
    static ASHBY: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^/([^/]+)/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})")
            .unwrap()
    });
    static SMARTRECRUITERS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^/([^/]+)/(\d{9,})").unwrap());

    let param = |k: &str| {
        url.query_pairs()
            .find(|(name, _)| name.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.to_string())
    };
    let h = host.trim_start_matches("www.");

    if h.ends_with("linkedin.com") {
        // A feed or collections URL carries the real id in `currentJobId`; a permalink
        // carries it in the path. Both name the same posting.
        let id = LINKEDIN_VIEW
            .captures(path)
            .map(|c| c[1].to_string())
            .or_else(|| param("currentJobId"))
            .or_else(|| param("currentJobid"))?;
        return Some(Identity {
            canonical: format!("https://www.linkedin.com/jobs/view/{id}"),
            source: SourceKind::LinkedIn,
            job_id: Some(id),
            board: None,
        });
    }

    if h.ends_with("indeed.com") || h.contains("indeed.co") {
        // `jk` is Indeed's job key and appears on viewjob, rc/clk and m/ paths alike.
        let jk = param("jk").or_else(|| param("vjk"))?;
        return Some(Identity {
            canonical: format!("https://www.indeed.com/viewjob?jk={jk}"),
            source: SourceKind::Indeed,
            job_id: Some(jk),
            board: None,
        });
    }

    if h.ends_with("greenhouse.io") {
        let c = GREENHOUSE.captures(path)?;
        let (board, id) = (c[1].to_string(), c[2].to_string());
        return Some(Identity {
            canonical: format!("https://boards.greenhouse.io/{board}/jobs/{id}"),
            source: SourceKind::Greenhouse,
            job_id: Some(id),
            board: Some(board),
        });
    }

    if h.ends_with("lever.co") {
        let c = LEVER.captures(path)?;
        let (board, id) = (c[1].to_string(), c[2].to_string());
        return Some(Identity {
            canonical: format!("https://jobs.lever.co/{board}/{id}"),
            source: SourceKind::Lever,
            job_id: Some(id),
            board: Some(board),
        });
    }

    if h.ends_with("ashbyhq.com") {
        let c = ASHBY.captures(path)?;
        let (board, id) = (c[1].to_string(), c[2].to_string());
        return Some(Identity {
            canonical: format!("https://jobs.ashbyhq.com/{board}/{id}"),
            source: SourceKind::Ashby,
            job_id: Some(id),
            board: Some(board),
        });
    }

    if h.ends_with("myworkdayjobs.com") {
        // Workday paths carry a locale segment that varies by visitor:
        // /en-US/<site>/job/<location>/<slug>_P751219-2. The requisition id is the
        // identity; the career-site slug is the board (needed to build the CXS URL).
        let parsed = workday_path(path)?;
        return Some(Identity {
            canonical: format!("https://{h}/{}/job/{}", parsed.site, parsed.req_id),
            source: SourceKind::Workday,
            job_id: Some(parsed.req_id),
            board: Some(parsed.site),
        });
    }

    if h.ends_with("smartrecruiters.com") {
        let c = SMARTRECRUITERS.captures(path)?;
        let (company, id) = (c[1].to_string(), c[2].to_string());
        return Some(Identity {
            canonical: format!("https://jobs.smartrecruiters.com/{company}/{id}"),
            source: SourceKind::SmartRecruiters,
            job_id: Some(id),
            board: Some(company),
        });
    }

    None
}

/// Tracking wrappers hide the real URL in a query parameter. Following them here means the
/// SSRF guard and robots check run against the destination, not the redirector.
fn unwrap_redirect(url: &Url) -> Option<String> {
    let host = url.host_str()?.trim_start_matches("www.");
    let key = match host {
        "linkedin.com" if url.path().starts_with("/slink") => "url",
        "google.com" if url.path() == "/url" => "q",
        _ if url.path() == "/safelinks/redirect" => "url",
        _ => return None,
    };
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.to_string())
        .filter(|v| v.starts_with("http"))
}

fn normalize_host(host: &str) -> String {
    let h = host.to_lowercase();
    // Indeed and LinkedIn run per-country subdomains that serve the same posting; folding
    // them to the canonical host stops one job appearing twice because you clicked a
    // `uk.indeed.com` link.
    if let Some(rest) = h.strip_suffix(".indeed.com") {
        if rest.len() <= 3 && rest != "www" {
            return "www.indeed.com".to_string();
        }
    }
    if h.ends_with("linkedin.com") && h != "www.linkedin.com" {
        return "www.linkedin.com".to_string();
    }
    if h == "job-boards.greenhouse.io" {
        return "boards.greenhouse.io".to_string();
    }
    h
}

fn strip_tracking_params(url: &mut Url) {
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| {
            let lower = k.to_lowercase();
            !lower.starts_with("utm_") && !TRACKING_PARAMS.contains(&lower.as_str())
        })
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        let mut q = url.query_pairs_mut();
        q.clear();
        for (k, v) in kept {
            q.append_pair(&k, &v);
        }
    }
}

/// Requisition ids Workday appends to the last path segment: `R-12345`, `JR1234`,
/// `P751219-2`. Zillow and several other tenants use a letter + digits + optional `-N`
/// suffix, not only `R` / `REQ` / `JR`.
static WORKDAY_REQ: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:^|_)((?:[A-Z]{1,6}[-_]?)?\d{3,}(?:-\d+)?)$").unwrap());

static WORKDAY_LOCALE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[A-Za-z]{2}(?:-[A-Za-z]{2})?$").unwrap());

struct WorkdayPath {
    site: String,
    /// Path after `/job/`, including the location slug. Needed to build the CXS URL.
    job_path: String,
    req_id: String,
}

fn workday_path(path: &str) -> Option<WorkdayPath> {
    let mut segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.first().is_some_and(|s| WORKDAY_LOCALE.is_match(s)) {
        segs.remove(0);
    }
    // A CXS URL is `/wday/cxs/<tenant>/<site>/job/...`.
    if segs.first().copied() == Some("wday")
        && segs.get(1).copied() == Some("cxs")
        && segs.len() >= 5
    {
        segs.drain(..3);
    }
    let site = (*segs.first()?).to_string();
    let job_idx = segs.iter().position(|s| *s == "job")?;
    let job_segs = &segs[job_idx + 1..];
    if job_segs.is_empty() {
        return None;
    }
    let last = *job_segs.last()?;
    let req_id = WORKDAY_REQ.captures(last).map(|c| c[1].to_string())?;
    Some(WorkdayPath {
        site,
        job_path: job_segs.join("/"),
        req_id,
    })
}

/// Workday's own CXS JSON endpoint — the same payload the careers SPA loads.
///
/// `https://{host}/wday/cxs/{tenant}/{site}/job/{location}/{slug}_{reqId}`
///
/// Built from the *original* URL so the location slug is preserved. Identity
/// canonicalization drops locale and location; the API cannot.
pub fn workday_cxs_url(original: &str) -> Option<String> {
    let url = Url::parse(original.trim()).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if !host.ends_with("myworkdayjobs.com") {
        return None;
    }
    let path = url.path();
    if path.contains("/wday/cxs/") {
        return Some(format!("https://{host}{}", path.trim_end_matches('/')));
    }
    let tenant = host.split('.').next()?;
    let parsed = workday_path(path)?;
    Some(format!(
        "https://{host}/wday/cxs/{tenant}/{}/job/{}",
        parsed.site, parsed.job_path
    ))
}

/// The public ATS JSON endpoint for a canonical URL, when one exists. Preferring it over the
/// HTML is both higher fidelity and cheaper (`docs/05-ingestion.md` §3).
pub fn ats_api_url(c: &CanonicalUrl) -> Option<String> {
    if c.source == SourceKind::Workday {
        return workday_cxs_url(&c.original);
    }
    let (board, id) = (c.board.as_deref()?, c.source_job_id.as_deref()?);
    Some(match c.source {
        SourceKind::Greenhouse => {
            format!("https://boards-api.greenhouse.io/v1/boards/{board}/jobs/{id}?questions=false")
        }
        SourceKind::Lever => format!("https://api.lever.co/v0/postings/{board}/{id}"),
        SourceKind::Ashby => {
            format!(
                "https://api.ashbyhq.com/posting-api/job-board/{board}?includeCompensation=true"
            )
        }
        SourceKind::SmartRecruiters => {
            format!("https://api.smartrecruiters.com/v1/companies/{board}/postings/{id}")
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canon(u: &str) -> CanonicalUrl {
        canonicalize(u).unwrap()
    }

    #[test]
    fn rejects_non_http_schemes_and_garbage() {
        assert!(canonicalize("ftp://example.com/job").is_err());
        assert!(canonicalize("javascript:alert(1)").is_err());
        assert!(canonicalize("not a url").is_err());
        assert!(canonicalize("file:///etc/passwd").is_err());
    }

    #[test]
    fn tracking_parameters_and_fragments_are_dropped() {
        let c = canon("https://acme.com/careers/senior-engineer?utm_source=x&gclid=y#apply");
        assert_eq!(c.canonical, "https://acme.com/careers/senior-engineer");
    }

    #[test]
    fn meaningful_query_parameters_survive() {
        let c = canon("https://acme.com/careers?id=42&utm_medium=email");
        assert_eq!(c.canonical, "https://acme.com/careers?id=42");
    }

    #[test]
    fn every_linkedin_url_shape_reduces_to_one_identity() {
        let expected = "https://www.linkedin.com/jobs/view/4123456789";
        for input in [
            "https://www.linkedin.com/jobs/view/4123456789/",
            "https://www.linkedin.com/jobs/view/senior-platform-engineer-at-acme-4123456789?refId=abc&trk=feed",
            "https://uk.linkedin.com/jobs/view/4123456789",
            "https://www.linkedin.com/jobs/collections/recommended/?currentJobId=4123456789&origin=JOB_SEARCH",
        ] {
            let c = canon(input);
            assert_eq!(c.canonical, expected, "input: {input}");
            assert_eq!(c.source, SourceKind::LinkedIn);
            assert_eq!(c.source_job_id.as_deref(), Some("4123456789"));
        }
    }

    #[test]
    fn indeed_reduces_to_its_job_key_across_country_domains() {
        let expected = "https://www.indeed.com/viewjob?jk=a1b2c3d4e5f6";
        for input in [
            "https://www.indeed.com/viewjob?jk=a1b2c3d4e5f6&tk=xyz&from=serp",
            "https://www.indeed.com/rc/clk?jk=a1b2c3d4e5f6&fccid=123",
            "https://uk.indeed.com/viewjob?jk=a1b2c3d4e5f6",
        ] {
            assert_eq!(canon(input).canonical, expected, "input: {input}");
        }
    }

    #[test]
    fn ats_urls_expose_board_and_id_for_cross_post_matching() {
        let gh = canon("https://job-boards.greenhouse.io/acmerobotics/jobs/5512034?gh_src=abc");
        assert_eq!(
            gh.canonical,
            "https://boards.greenhouse.io/acmerobotics/jobs/5512034"
        );
        assert_eq!(gh.source, SourceKind::Greenhouse);
        assert_eq!(gh.board.as_deref(), Some("acmerobotics"));
        assert_eq!(gh.source_job_id.as_deref(), Some("5512034"));

        let lever = canon("https://jobs.lever.co/acme/1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed/apply");
        assert_eq!(
            lever.canonical,
            "https://jobs.lever.co/acme/1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed"
        );
        assert_eq!(lever.source, SourceKind::Lever);
    }

    #[test]
    fn workday_identity_survives_the_locale_segment() {
        let a = canon("https://acme.wd1.myworkdayjobs.com/en-US/acme_careers/job/San-Francisco/Senior-Platform-Engineer_R-12345");
        let b = canon("https://acme.wd1.myworkdayjobs.com/de-DE/acme_careers/job/Berlin/Senior-Platform-Engineer_R-12345");
        assert_eq!(a.canonical, b.canonical, "locale must not fork identity");
        assert_eq!(
            a.canonical,
            "https://acme.wd1.myworkdayjobs.com/acme_careers/job/R-12345"
        );
        assert_eq!(a.source_job_id.as_deref(), Some("R-12345"));
        assert_eq!(a.board.as_deref(), Some("acme_careers"));
        assert_eq!(a.source, SourceKind::Workday);
    }

    #[test]
    fn workday_accepts_product_style_req_ids_and_builds_cxs() {
        let url = "https://zillow.wd5.myworkdayjobs.com/en-US/Zillow_Group_External/job/Remote-USA/Data-Scientist_P751219-2";
        let c = canon(url);
        assert_eq!(c.source, SourceKind::Workday);
        assert_eq!(c.source_job_id.as_deref(), Some("P751219-2"));
        assert_eq!(c.board.as_deref(), Some("Zillow_Group_External"));
        assert_eq!(
            c.canonical,
            "https://zillow.wd5.myworkdayjobs.com/Zillow_Group_External/job/P751219-2"
        );
        assert_eq!(
            ats_api_url(&c).as_deref(),
            Some("https://zillow.wd5.myworkdayjobs.com/wday/cxs/zillow/Zillow_Group_External/job/Remote-USA/Data-Scientist_P751219-2")
        );
        let seattle = canon(
            "https://zillow.wd5.myworkdayjobs.com/en-US/Zillow_Group_External/job/Seattle-WA/Data-Scientist_P751219-2",
        );
        assert_eq!(
            seattle.canonical, c.canonical,
            "location options of the same req must share identity"
        );
    }

    #[test]
    fn redirect_wrappers_resolve_to_the_destination() {
        let c = canon("https://www.linkedin.com/slink?code=abc&url=https%3A%2F%2Fboards.greenhouse.io%2Facme%2Fjobs%2F999");
        assert_eq!(c.canonical, "https://boards.greenhouse.io/acme/jobs/999");
        assert_eq!(c.source, SourceKind::Greenhouse);
    }

    #[test]
    fn scheme_and_case_are_normalized_so_http_and_https_agree() {
        assert_eq!(
            canon("http://ACME.com/Careers/Job").canonical,
            canon("https://acme.com/Careers/Job").canonical
        );
    }

    #[test]
    fn unknown_hosts_are_treated_as_company_sites() {
        let c = canon("https://careers.acme-robotics.dev/openings/platform-eng");
        assert_eq!(c.source, SourceKind::CompanySite);
        assert_eq!(c.source_job_id, None);
    }

    #[test]
    fn identical_identities_hash_identically() {
        let a = canon("https://www.linkedin.com/jobs/view/4123456789/?trk=feed");
        let b = canon("https://www.linkedin.com/jobs/collections/?currentJobId=4123456789");
        assert_eq!(a.url_hash(), b.url_hash());
    }

    #[test]
    fn ats_api_endpoints_are_derived_only_where_they_exist() {
        let gh = canon("https://boards.greenhouse.io/acme/jobs/1");
        assert_eq!(
            ats_api_url(&gh).as_deref(),
            Some("https://boards-api.greenhouse.io/v1/boards/acme/jobs/1?questions=false")
        );
        let li = canon("https://www.linkedin.com/jobs/view/4123456789");
        assert_eq!(
            ats_api_url(&li),
            None,
            "aggregators have no public posting API"
        );
    }

    #[test]
    fn trailing_slashes_do_not_fork_identity() {
        assert_eq!(
            canon("https://acme.com/careers/eng/").canonical,
            canon("https://acme.com/careers/eng").canonical
        );
    }
}
