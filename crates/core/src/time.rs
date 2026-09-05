//! Time handling.
//!
//! Everything is UTC and serialized as RFC3339, which is lexicographically sortable (so
//! `ORDER BY` works on a `TEXT` column) and readable in the exported files. Job postings
//! routinely give vague dates ("30+ days ago"), so a partial date carries its own precision
//! rather than being rounded into a false exactness.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// UTC instant. Stored as RFC3339 text.
pub type Timestamp = DateTime<Utc>;

pub fn now() -> Timestamp {
    Utc::now()
}

/// RFC3339 with second precision and a `Z` suffix — one canonical spelling so hashes over
/// serialized records are stable.
pub fn to_rfc3339(ts: &Timestamp) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn parse_rfc3339(s: &str) -> crate::Result<Timestamp> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| crate::Error::BadRequest(format!("bad timestamp {s:?}: {e}")))
}

/// How precisely a source stated a date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatePrecision {
    Exact,
    Hour,
    Day,
    Month,
    /// The source said something like "30+ days ago": we know a lower bound, not a date.
    AtLeast,
    Unknown,
}

impl DatePrecision {
    pub fn as_str(self) -> &'static str {
        match self {
            DatePrecision::Exact => "exact",
            DatePrecision::Hour => "hour",
            DatePrecision::Day => "day",
            DatePrecision::Month => "month",
            DatePrecision::AtLeast => "at_least",
            DatePrecision::Unknown => "unknown",
        }
    }
}

/// A date we resolved from vague source text, with the precision we actually have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialDate {
    pub at: Timestamp,
    pub precision: DatePrecision,
}

impl PartialDate {
    pub fn exact(at: Timestamp) -> Self {
        Self {
            at,
            precision: DatePrecision::Exact,
        }
    }

    pub fn day(at: Timestamp) -> Self {
        Self {
            at,
            precision: DatePrecision::Day,
        }
    }

    pub fn at_least(at: Timestamp) -> Self {
        Self {
            at,
            precision: DatePrecision::AtLeast,
        }
    }

    pub fn from_naive_date(d: NaiveDate) -> Self {
        let at = Utc.from_utc_datetime(&d.and_hms_opt(12, 0, 0).expect("noon is a valid time"));
        Self {
            at,
            precision: DatePrecision::Day,
        }
    }

    /// Human-friendly age, honest about imprecision: `"3d"`, `"≥30d"`, `"today"`.
    pub fn age_label(&self, relative_to: Timestamp) -> String {
        let days = (relative_to - self.at).num_days();
        let prefix = if self.precision == DatePrecision::AtLeast {
            "≥"
        } else {
            ""
        };
        match days {
            d if d < 0 => "future".to_string(),
            0 => "today".to_string(),
            d if d < 30 => format!("{prefix}{d}d"),
            d if d < 365 => format!("{prefix}{}mo", d / 30),
            d => format!("{prefix}{}y", d / 365),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_round_trips_canonically() {
        let ts = parse_rfc3339("2026-09-05T20:21:00Z").unwrap();
        assert_eq!(to_rfc3339(&ts), "2026-09-05T20:21:00Z");
        // Offsets normalize to UTC so string comparison in SQL is meaningful.
        let offset = parse_rfc3339("2026-09-05T16:21:00-04:00").unwrap();
        assert_eq!(to_rfc3339(&offset), "2026-09-05T20:21:00Z");
    }

    #[test]
    fn timestamps_sort_lexicographically() {
        let a = to_rfc3339(&parse_rfc3339("2026-09-05T20:21:00Z").unwrap());
        let b = to_rfc3339(&parse_rfc3339("2026-09-06T00:00:00Z").unwrap());
        assert!(a < b);
    }

    #[test]
    fn age_label_reflects_precision() {
        let now = parse_rfc3339("2026-09-05T00:00:00Z").unwrap();
        let posted = parse_rfc3339("2026-09-02T00:00:00Z").unwrap();
        assert_eq!(PartialDate::day(posted).age_label(now), "3d");
        let old = parse_rfc3339("2026-08-01T00:00:00Z").unwrap();
        assert_eq!(PartialDate::at_least(old).age_label(now), "≥1mo");
    }
}
