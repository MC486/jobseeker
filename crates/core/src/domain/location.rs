//! Locations.
//!
//! A posting may name several cities, or a remote-eligibility *region* ("Remote — US"),
//! which is a different kind of statement from a city. Both are rows; `is_remote_scope`
//! distinguishes them.

use serde::{Deserialize, Serialize};

use crate::ids::JobId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobLocation {
    pub job_id: JobId,
    /// Verbatim source string. Never discarded, even when parsing succeeds.
    pub raw: String,
    pub city: Option<String>,
    /// State / province / region, normalized to a code where one exists (`CA`, `NY`, `ON`).
    pub region: Option<String>,
    /// ISO 3166-1 alpha-2.
    pub country: Option<String>,
    pub postal_code: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub is_primary: bool,
    /// True when this row describes where you may *live* while remote, not where an office
    /// is.
    pub is_remote_scope: bool,
    pub timezone_requirement: Option<String>,
    pub ordinal: i64,
}

impl JobLocation {
    /// `"San Francisco, CA, US"` / `"Remote (US)"`.
    pub fn display(&self) -> String {
        let parts: Vec<&str> = [
            self.city.as_deref(),
            self.region.as_deref(),
            self.country.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        if parts.is_empty() {
            return self.raw.clone();
        }
        let joined = parts.join(", ");
        if self.is_remote_scope {
            format!("Remote ({joined})")
        } else {
            joined
        }
    }

    /// Loose match against a location the user is willing to work in. Any provided level
    /// (city/region/country) must agree; missing levels are treated as compatible so that a
    /// posting that only says "US" still matches "San Francisco, CA, US".
    pub fn matches_preference(
        &self,
        city: Option<&str>,
        region: Option<&str>,
        country: Option<&str>,
    ) -> bool {
        fn agrees(a: Option<&str>, b: Option<&str>) -> bool {
            match (a, b) {
                (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                _ => true,
            }
        }
        agrees(self.city.as_deref(), city)
            && agrees(self.region.as_deref(), region)
            && agrees(self.country.as_deref(), country)
    }
}

/// A location string before normalization, carrying whatever hints the source gave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLocation {
    pub text: String,
    pub is_remote_hint: bool,
    pub country_hint: Option<String>,
}

impl RawLocation {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_remote_hint: false,
            country_hint: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(
        city: Option<&str>,
        region: Option<&str>,
        country: Option<&str>,
        remote: bool,
    ) -> JobLocation {
        JobLocation {
            job_id: JobId::new(),
            raw: "raw".into(),
            city: city.map(Into::into),
            region: region.map(Into::into),
            country: country.map(Into::into),
            postal_code: None,
            lat: None,
            lon: None,
            is_primary: true,
            is_remote_scope: remote,
            timezone_requirement: None,
            ordinal: 0,
        }
    }

    #[test]
    fn display_distinguishes_remote_scope_from_an_office() {
        assert_eq!(
            loc(Some("San Francisco"), Some("CA"), Some("US"), false).display(),
            "San Francisco, CA, US"
        );
        assert_eq!(loc(None, None, Some("US"), true).display(), "Remote (US)");
    }

    #[test]
    fn display_falls_back_to_the_raw_string() {
        let mut l = loc(None, None, None, false);
        l.raw = "Multiple locations".into();
        assert_eq!(l.display(), "Multiple locations");
    }

    #[test]
    fn coarse_locations_still_match_specific_preferences() {
        let us_only = loc(None, None, Some("US"), true);
        assert!(us_only.matches_preference(Some("San Francisco"), Some("CA"), Some("US")));
        assert!(!us_only.matches_preference(Some("Berlin"), None, Some("DE")));

        let sf = loc(Some("San Francisco"), Some("CA"), Some("US"), false);
        assert!(sf.matches_preference(Some("san francisco"), None, None));
        assert!(!sf.matches_preference(Some("Oakland"), None, None));
    }
}
