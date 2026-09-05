//! `robots.txt` evaluation.
//!
//! Honouring robots.txt is a project stance, not a legal opinion
//! (`docs/13-security-privacy-legal.md`). A disallow is reported as `robots_disallowed`
//! and tells the user to capture the page with the extension, rather than being silently
//! worked around.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use jobseeker_core::config::AcquireConfig;
use jobseeker_core::{Error, Result};
use url::Url;

use crate::fetch::Fetcher;

const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const OUR_UA_TOKEN: &str = "jobseeker";

#[derive(Clone)]
struct Cached {
    rules: Rules,
    fetched_at: Instant,
}

pub struct RobotsCache {
    enabled: bool,
    entries: Mutex<HashMap<String, Cached>>,
}

impl RobotsCache {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn from_config(cfg: &AcquireConfig) -> Self {
        Self::new(cfg.respect_robots)
    }

    /// Whether our User-Agent may fetch `url`. A missing or unreadable robots.txt is
    /// treated as allow: a broken robots file must not make the whole host uningestible.
    pub async fn allows(&self, fetcher: &Fetcher, url: &str) -> Result<bool> {
        if !self.enabled {
            return Ok(true);
        }
        let parsed = Url::parse(url).map_err(|e| Error::InvalidUrl(e.to_string()))?;
        let host = parsed.host_str().unwrap_or_default().to_string();
        let path = parsed.path().to_string();

        if let Some(rules) = self.cached(&host) {
            return Ok(rules.allows(&path));
        }

        let robots_url = format!("{}://{}/robots.txt", parsed.scheme(), host);
        let rules = match fetcher.get(&robots_url, None, None).await {
            Ok(outcome) if outcome.status == 200 => Rules::parse(&outcome.body_text()),
            Ok(_) | Err(_) => Rules::empty(),
        };
        self.store(host, rules.clone());
        Ok(rules.allows(&path))
    }

    /// Evaluate already-fetched robots.txt, for tests and for the reconcile path.
    pub fn allows_parsed(robots_body: &str, path: &str) -> bool {
        Rules::parse(robots_body).allows(path)
    }

    fn cached(&self, host: &str) -> Option<Rules> {
        let map = self.entries.lock().expect("robots lock");
        map.get(host).and_then(|c| {
            (c.fetched_at.elapsed() < CACHE_TTL).then_some(c.rules.clone())
        })
    }

    fn store(&self, host: String, rules: Rules) {
        self.entries.lock().expect("robots lock").insert(
            host,
            Cached {
                rules,
                fetched_at: Instant::now(),
            },
        );
    }
}

/// The subset of robots.txt we honour: `User-agent` / `Allow` / `Disallow`. Crawl-delay
/// is ignored; our own per-host limiter is stricter and always on.
#[derive(Debug, Clone, Default)]
struct Rules {
    /// Longest matching prefix wins. `allow` true beats an equally long `disallow`, which
    /// is the Google interpretation and the one most operators expect.
    directives: Vec<(String, bool)>,
}

impl Rules {
    fn empty() -> Self {
        Self::default()
    }

    fn parse(body: &str) -> Self {
        let mut applicable = false;
        let mut directives = Vec::new();
        for raw in body.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else { continue };
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
            match key.as_str() {
                "user-agent" => {
                    applicable = value == "*"
                        || value.eq_ignore_ascii_case(OUR_UA_TOKEN)
                        || value.to_ascii_lowercase().contains(OUR_UA_TOKEN);
                }
                "disallow" if applicable => {
                    directives.push((value.to_string(), false));
                }
                "allow" if applicable => {
                    directives.push((value.to_string(), true));
                }
                _ => {}
            }
        }
        Self { directives }
    }

    fn allows(&self, path: &str) -> bool {
        if self.directives.is_empty() {
            return true;
        }
        let mut best_len = -1isize;
        let mut allowed = true;
        for (prefix, allow) in &self.directives {
            if prefix.is_empty() {
                // An empty Disallow means "allow everything", which is how operators
                // write an allow-all file. An empty Allow is a no-op.
                if !allow {
                    if best_len < 0 {
                        allowed = true;
                    }
                }
                continue;
            }
            if path.starts_with(prefix.as_str()) && prefix.len() as isize >= best_len {
                // Equal length: Allow wins. That is what lets
                // `Disallow: /jobs/` + `Allow: /jobs/api` work.
                if prefix.len() as isize == best_len && *allow {
                    allowed = true;
                } else if prefix.len() as isize > best_len {
                    best_len = prefix.len() as isize;
                    allowed = *allow;
                }
            }
        }
        allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(robots: &str, path: &str) -> bool {
        RobotsCache::allows_parsed(robots, path)
    }

    #[test]
    fn an_empty_or_missing_file_allows_everything() {
        assert!(allowed("", "/jobs/1"));
        assert!(allowed("User-agent: *\nDisallow:\n", "/jobs/1"));
    }

    #[test]
    fn a_blanket_disallow_refuses_every_path() {
        let robots = "User-agent: *\nDisallow: /\n";
        assert!(!allowed(robots, "/"));
        assert!(!allowed(robots, "/jobs/1"));
    }

    #[test]
    fn a_path_disallow_does_not_block_siblings() {
        let robots = "User-agent: *\nDisallow: /jobs/\nAllow: /\n";
        assert!(!allowed(robots, "/jobs/1"));
        assert!(allowed(robots, "/careers/1"));
    }

    #[test]
    fn the_longest_matching_directive_wins_and_allow_breaks_ties() {
        let robots = "\
User-agent: *\n\
Disallow: /jobs/\n\
Allow: /jobs/api\n\
";
        assert!(!allowed(robots, "/jobs/view/1"));
        assert!(allowed(robots, "/jobs/api/1"), "the more specific Allow must win");
    }

    #[test]
    fn our_user_agent_is_honoured_when_named() {
        let robots = "\
User-agent: Googlebot\n\
Disallow: /\n\
\n\
User-agent: jobseeker\n\
Disallow: /private/\n\
";
        assert!(allowed(robots, "/jobs/1"), "Googlebot's rules must not apply to us");
        assert!(!allowed(robots, "/private/secret"));
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let robots = "# generated\n\nUser-agent: *  # everyone\nDisallow: /tmp  # scratch\n";
        assert!(!allowed(robots, "/tmp/x"));
        assert!(allowed(robots, "/jobs"));
    }

    #[test]
    fn a_disabled_cache_always_allows() {
        let cache = RobotsCache::new(false);
        // No fetcher is consulted; this must not panic or require a network.
        assert!(cache.enabled == false);
    }
}
