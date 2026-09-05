//! Server-side request forgery guard.
//!
//! A user-supplied URL is an instruction to make a request. Without this check, pasting
//! `http://169.254.169.254/` would ask the home server to read its own cloud metadata, and
//! `http://127.0.0.1:5432` would probe whatever is listening locally. The ranges below are
//! rejected even when the hostname looks public, because DNS can resolve to a private
//! address (`docs/13-security-privacy-legal.md`).
//!
//! The connection is pinned to the validated IP so a DNS rebinding between the check and
//! the request cannot sneak past.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use jobseeker_core::{Error, Result};
use url::Url;

pub struct Guard {
    allow_private: bool,
}

impl Guard {
    pub fn new(allow_private: bool) -> Self {
        Self { allow_private }
    }

    /// Reject a URL that would target a non-public address. DNS is not resolved here so
    /// the check stays testable and cheap; [`Fetcher`] resolves and re-checks the IP
    /// before connecting.
    pub fn check(&self, url: &str) -> Result<()> {
        let parsed = Url::parse(url).map_err(|e| Error::InvalidUrl(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(Error::InvalidUrl(format!(
                "only http and https are supported, got {:?}",
                parsed.scheme()
            )));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| Error::InvalidUrl("url has no host".into()))?;
        if host.eq_ignore_ascii_case("localhost")
            || host.eq_ignore_ascii_case("metadata.google.internal")
        {
            return self.reject(host);
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_blocked_ip(ip) {
                return self.reject(host);
            }
        }
        Ok(())
    }

    pub fn check_ip(&self, ip: IpAddr) -> Result<()> {
        if is_blocked_ip(ip) {
            self.reject(&ip.to_string())
        } else {
            Ok(())
        }
    }

    fn reject(&self, host: &str) -> Result<()> {
        if self.allow_private {
            tracing::warn!(
                host,
                "allowing a private-network fetch (acquire.allow_private_networks)"
            );
            return Ok(());
        }
        Err(Error::BlockedPrivateNetwork(host.to_string()))
    }
}

/// True when `ip` must not be requested. Covers IPv4, IPv6, and IPv4-mapped IPv6, because
/// `[::ffff:127.0.0.1]` is the same destination as `127.0.0.1`.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_v4(v4);
            }
            is_blocked_v6(v6)
        }
    }
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (o[0] == 100 && o[1] >= 64 && o[1] <= 127) // CGNAT 100.64/10
        || (o[0] == 169 && o[1] == 254) // link-local, including cloud metadata
        || o[0] == 0
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF protocol assignments
        || (o[0] == 192 && o[1] == 0 && o[2] == 2) // TEST-NET-1
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        || o[0] >= 240
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local() // fc00::/7
        || (ip.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(s: &str) -> bool {
        is_blocked_ip(s.parse().unwrap())
    }

    #[test]
    fn loopback_and_unspecified_are_blocked() {
        assert!(blocked("127.0.0.1"));
        assert!(blocked("127.1.2.3"), "the entire 127/8 is loopback");
        assert!(blocked("0.0.0.0"));
        assert!(blocked("::1"));
        assert!(blocked("::"));
    }

    #[test]
    fn rfc1918_and_cgnat_are_blocked() {
        assert!(blocked("10.0.0.1"));
        assert!(blocked("192.168.1.1"));
        assert!(blocked("172.16.0.1"));
        assert!(blocked("172.31.255.255"));
        assert!(!blocked("172.32.0.1"), "172.32/8 is public");
        assert!(blocked("100.64.0.1"), "CGNAT, including Tailscale");
        assert!(blocked("100.127.255.255"));
        assert!(!blocked("100.128.0.1"));
    }

    #[test]
    fn cloud_metadata_and_link_local_are_blocked() {
        assert!(blocked("169.254.169.254"));
        assert!(blocked("169.254.0.1"));
        assert!(blocked("fe80::1"));
    }

    #[test]
    fn ipv4_mapped_ipv6_does_not_bypass_the_v4_rules() {
        // Without this, `[::ffff:127.0.0.1]` would look like a public IPv6 address.
        assert!(blocked("::ffff:127.0.0.1"));
        assert!(blocked("::ffff:192.168.0.1"));
        assert!(blocked("::ffff:169.254.169.254"));
        assert!(!blocked("::ffff:1.1.1.1"));
    }

    #[test]
    fn unique_local_ipv6_is_blocked() {
        assert!(blocked("fc00::1"));
        assert!(blocked("fd12:3456:789a::1"));
    }

    #[test]
    fn public_addresses_are_allowed() {
        assert!(!blocked("1.1.1.1"));
        assert!(!blocked("8.8.8.8"));
        assert!(!blocked("2606:4700:4700::1111"));
    }

    #[test]
    fn a_literal_private_url_is_rejected_without_resolving() {
        let guard = Guard::new(false);
        assert_eq!(
            guard
                .check("http://127.0.0.1:8787/jobs")
                .unwrap_err()
                .code(),
            "blocked_private_network"
        );
        assert_eq!(
            guard
                .check("http://169.254.169.254/latest/meta-data/")
                .unwrap_err()
                .code(),
            "blocked_private_network"
        );
        assert_eq!(
            guard.check("http://localhost/admin").unwrap_err().code(),
            "blocked_private_network"
        );
        assert!(guard
            .check("https://boards.greenhouse.io/acme/jobs/1")
            .is_ok());
    }

    #[test]
    fn the_escape_hatch_is_explicit_and_logged_not_silent() {
        let guard = Guard::new(true);
        assert!(
            guard.check("http://192.168.1.10/careers").is_ok(),
            "people hosting their own board on the LAN must be able to ingest it"
        );
    }

    #[test]
    fn non_http_schemes_are_rejected_even_when_the_host_is_public() {
        let guard = Guard::new(false);
        assert_eq!(
            guard.check("file:///etc/passwd").unwrap_err().code(),
            "invalid_url"
        );
        assert_eq!(
            guard.check("gopher://example.com/").unwrap_err().code(),
            "invalid_url"
        );
    }
}
