//! HTTP fetching with the guardrails applied per hop.
//!
//! Redirects are followed by hand so each hop is re-validated for scheme and SSRF. reqwest's
//! built-in policy would follow `http://evil.test` → `http://169.254.169.254/` without us
//! seeing the destination (`docs/05-ingestion.md` §3).

use std::time::Duration;

use jobseeker_core::config::AcquireConfig;
use jobseeker_core::hash::content_hash;
use jobseeker_core::{Error, Result};
use url::Url;

use crate::ssrf::Guard;

const MAX_REDIRECTS: usize = 5;

/// What a fetch produced. The body is the raw bytes as received (decompressed by reqwest);
/// the caller decides how to store them.
#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub url: String,
    pub final_url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub body: Vec<u8>,
}

impl FetchOutcome {
    pub fn content_hash(&self) -> String {
        content_hash(&self.body)
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn is_not_modified(&self) -> bool {
        self.status == 304
    }

    pub fn looks_closed(&self) -> bool {
        matches!(self.status, 404 | 410)
    }
}

pub struct Fetcher {
    http: reqwest::Client,
    user_agent: String,
    max_body_bytes: u64,
    guard: Guard,
}

impl Fetcher {
    pub fn new(cfg: &AcquireConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_seconds))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(&cfg.user_agent)
            .build()
            .map_err(|e| Error::Config(format!("could not build the HTTP client: {e}")))?;
        Ok(Self {
            http,
            user_agent: cfg.user_agent.clone(),
            max_body_bytes: cfg.max_body_bytes,
            guard: Guard::new(cfg.allow_private_networks),
        })
    }

    /// GET `url`, optionally sending validators from the previous capture so an unchanged
    /// posting costs a 304 and nothing else.
    pub async fn get(
        &self,
        url: &str,
        if_none_match: Option<&str>,
        if_modified_since: Option<&str>,
    ) -> Result<FetchOutcome> {
        let mut current = url.to_string();
        for hop in 0..=MAX_REDIRECTS {
            self.guard.check(&current)?;
            let mut request = self.http.get(&current);
            if let Some(etag) = if_none_match {
                request = request.header("if-none-match", etag);
            }
            if let Some(since) = if_modified_since {
                request = request.header("if-modified-since", since);
            }

            let response = request.send().await.map_err(classify)?;
            let status = response.status();

            if status.is_redirection() {
                let next = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| Error::FetchStatus(status.as_u16()))?;
                current = resolve_redirect(&current, next)?;
                if hop == MAX_REDIRECTS {
                    return Err(Error::Network(format!(
                        "too many redirects fetching {url} (last hop {current})"
                    )));
                }
                continue;
            }

            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let etag = header(&response, reqwest::header::ETAG);
            let last_modified = header(&response, reqwest::header::LAST_MODIFIED);

            if status.as_u16() == 304 {
                return Ok(FetchOutcome {
                    url: url.to_string(),
                    final_url: current,
                    status: 304,
                    content_type,
                    etag,
                    last_modified,
                    body: Vec::new(),
                });
            }

            if let Some(len) = response.content_length() {
                if len > self.max_body_bytes {
                    return Err(Error::PayloadTooLarge(len, self.max_body_bytes));
                }
            }

            let body = response.bytes().await.map_err(classify)?;
            if body.len() as u64 > self.max_body_bytes {
                return Err(Error::PayloadTooLarge(
                    body.len() as u64,
                    self.max_body_bytes,
                ));
            }

            let status_code = status.as_u16();
            if status_code == 401 || status_code == 403 {
                return Err(Error::NeedsBrowser);
            }
            if status_code >= 400 && status_code != 404 && status_code != 410 {
                return Err(Error::FetchStatus(status_code));
            }

            return Ok(FetchOutcome {
                url: url.to_string(),
                final_url: current,
                status: status_code,
                content_type,
                etag,
                last_modified,
                body: body.to_vec(),
            });
        }
        Err(Error::Network(format!("too many redirects fetching {url}")))
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }
}

fn header(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn resolve_redirect(current: &str, location: &str) -> Result<String> {
    let base = Url::parse(current).map_err(|e| Error::InvalidUrl(e.to_string()))?;
    base.join(location)
        .map(|u| u.to_string())
        .map_err(|e| Error::InvalidUrl(e.to_string()))
}

fn classify(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::FetchTimeout(0)
    } else if error.is_connect() {
        Error::Network(error.to_string())
    } else {
        Error::Network(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_redirects_resolve_against_the_current_url() {
        assert_eq!(
            resolve_redirect("https://acme.com/jobs/1", "/login").unwrap(),
            "https://acme.com/login"
        );
        assert_eq!(
            resolve_redirect(
                "https://acme.com/jobs/1",
                "https://boards.greenhouse.io/acme/jobs/1"
            )
            .unwrap(),
            "https://boards.greenhouse.io/acme/jobs/1"
        );
    }

    #[test]
    fn a_redirect_to_a_private_address_is_caught_on_the_next_hop() {
        // The guard is applied at the top of each loop iteration, so a public host that
        // 302s to metadata cannot smuggle the request through.
        let guard = Guard::new(false);
        assert!(guard.check("https://evil.example/redirect").is_ok());
        assert_eq!(
            guard.check("http://169.254.169.254/").unwrap_err().code(),
            "blocked_private_network"
        );
    }

    #[test]
    fn closed_and_unchanged_statuses_are_identified() {
        let not_modified = FetchOutcome {
            url: "https://x".into(),
            final_url: "https://x".into(),
            status: 304,
            content_type: None,
            etag: Some("\"abc\"".into()),
            last_modified: None,
            body: vec![],
        };
        assert!(not_modified.is_not_modified());
        assert!(!not_modified.looks_closed());

        let gone = FetchOutcome {
            status: 410,
            ..not_modified.clone()
        };
        assert!(gone.looks_closed());
    }

    #[test]
    fn the_content_hash_is_over_the_raw_bytes() {
        let a = FetchOutcome {
            url: String::new(),
            final_url: String::new(),
            status: 200,
            content_type: None,
            etag: None,
            last_modified: None,
            body: b"hello".to_vec(),
        };
        let mut b = a.clone();
        b.body = b"hellp".to_vec();
        assert_ne!(a.content_hash(), b.content_hash());
        assert_eq!(a.content_hash(), content_hash(b"hello"));
    }
}
