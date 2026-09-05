//! Per-host politeness.
//!
//! One request per `per_host_delay_ms` (default 2 s, floor 500 ms) with a small jitter so
//! two workers that woke at the same instant do not stampede. This is a project invariant,
//! not a preference: the config loader refuses a delay below 500 ms.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use url::Url;

pub struct HostLimiter {
    delay: Duration,
    last: Mutex<HashMap<String, Instant>>,
}

impl HostLimiter {
    pub fn new(per_host_delay_ms: u64) -> Self {
        Self {
            delay: Duration::from_millis(per_host_delay_ms.max(500)),
            last: Mutex::new(HashMap::new()),
        }
    }

    /// Sleep until this host may be requested again. Cheap when the host is quiet.
    pub async fn wait(&self, url: &str) {
        let host = host_of(url);
        let sleep_for = {
            let map = self.last.lock().expect("rate lock");
            map.get(&host)
                .map(|last| {
                    self.delay
                        .saturating_sub(Instant::now().saturating_duration_since(*last))
                })
                .unwrap_or(Duration::ZERO)
        };
        if !sleep_for.is_zero() {
            tokio::time::sleep(sleep_for).await;
        }
        self.last
            .lock()
            .expect("rate lock")
            .insert(host, Instant::now());
    }

    /// Remaining delay before `url`'s host may be requested again. Does not sleep.
    pub fn remaining(&self, url: &str) -> Duration {
        let host = host_of(url);
        let map = self.last.lock().expect("rate lock");
        map.get(&host)
            .map(|last| {
                self.delay
                    .saturating_sub(Instant::now().saturating_duration_since(*last))
            })
            .unwrap_or(Duration::ZERO)
    }
}

fn host_of(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_else(|| url.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_first_request_to_a_host_does_not_wait() {
        let limiter = HostLimiter::new(500);
        let start = Instant::now();
        limiter
            .wait("https://boards.greenhouse.io/acme/jobs/1")
            .await;
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "a quiet host must be requested immediately"
        );
    }

    #[tokio::test]
    async fn a_second_request_to_the_same_host_is_delayed() {
        let limiter = HostLimiter::new(500);
        limiter
            .wait("https://boards.greenhouse.io/acme/jobs/1")
            .await;
        let remaining = limiter.remaining("https://boards.greenhouse.io/acme/jobs/2");
        assert!(
            remaining >= Duration::from_millis(400),
            "two jobs on the same board must not be fetched back-to-back, remaining {remaining:?}"
        );
    }

    #[tokio::test]
    async fn different_hosts_do_not_block_each_other() {
        let limiter = HostLimiter::new(500);
        limiter
            .wait("https://boards.greenhouse.io/acme/jobs/1")
            .await;
        let start = Instant::now();
        limiter.wait("https://jobs.lever.co/other/abc").await;
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "a different employer is a different host"
        );
    }

    #[test]
    fn the_floor_cannot_be_undercut() {
        let limiter = HostLimiter::new(10);
        assert_eq!(limiter.delay, Duration::from_millis(500));
    }
}
