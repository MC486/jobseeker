//! Compensation.
//!
//! Stored as integer minor units plus an ISO-4217 code — never floats, because a rounding
//! error in a salary filter is the kind of bug that silently hides jobs.

use serde::{Deserialize, Serialize};

use crate::domain::enums::SourceKind;

/// The interval a figure is quoted over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SalaryPeriod {
    Year,
    Month,
    Week,
    Day,
    Hour,
    /// A fixed fee for the whole engagement.
    Project,
    Unknown,
}

impl SalaryPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            SalaryPeriod::Year => "year",
            SalaryPeriod::Month => "month",
            SalaryPeriod::Week => "week",
            SalaryPeriod::Day => "day",
            SalaryPeriod::Hour => "hour",
            SalaryPeriod::Project => "project",
            SalaryPeriod::Unknown => "unknown",
        }
    }

    pub const ALL: &'static [SalaryPeriod] = &[
        SalaryPeriod::Year,
        SalaryPeriod::Month,
        SalaryPeriod::Week,
        SalaryPeriod::Day,
        SalaryPeriod::Hour,
        SalaryPeriod::Project,
        SalaryPeriod::Unknown,
    ];

    /// Multiply a figure up to an annual equivalent. 2080 hours = 40h × 52w, the
    /// conventional US assumption; documented here because it is a judgement call.
    pub fn annualize(self, cents: i64) -> i64 {
        match self {
            SalaryPeriod::Year | SalaryPeriod::Project | SalaryPeriod::Unknown => cents,
            SalaryPeriod::Month => cents * 12,
            SalaryPeriod::Week => cents * 52,
            SalaryPeriod::Day => cents * 260,
            SalaryPeriod::Hour => cents * 2080,
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            SalaryPeriod::Year => "/yr",
            SalaryPeriod::Month => "/mo",
            SalaryPeriod::Week => "/wk",
            SalaryPeriod::Day => "/day",
            SalaryPeriod::Hour => "/hr",
            SalaryPeriod::Project => " total",
            SalaryPeriod::Unknown => "",
        }
    }
}

impl std::fmt::Display for SalaryPeriod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SalaryPeriod {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        Ok(match s {
            "year" | "yr" | "annual" | "annum" => SalaryPeriod::Year,
            "month" | "mo" => SalaryPeriod::Month,
            "week" | "wk" => SalaryPeriod::Week,
            "day" => SalaryPeriod::Day,
            "hour" | "hr" => SalaryPeriod::Hour,
            "project" => SalaryPeriod::Project,
            "unknown" => SalaryPeriod::Unknown,
            other => {
                return Err(crate::Error::BadRequest(format!(
                    "unknown salary period: {other}"
                )))
            }
        })
    }
}

/// Normalized compensation, faithful to what the posting said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Salary {
    pub min_cents: Option<i64>,
    pub max_cents: Option<i64>,
    /// ISO 4217, uppercase.
    pub currency: String,
    pub period: SalaryPeriod,
    /// True when the figure is the *site's* estimate rather than the employer's band.
    pub is_estimate: bool,
    /// Verbatim source text, always retained so a parsing bug is recoverable.
    pub raw: Option<String>,
}

impl Salary {
    /// Annualized midpoint, for comparing an hourly contract against a salaried role.
    /// Computed on demand rather than stored, so the stored record stays faithful.
    pub fn annualized_max_cents(&self) -> Option<i64> {
        let max = self.max_cents.or(self.min_cents)?;
        Some(self.period.annualize(max))
    }

    pub fn annualized_min_cents(&self) -> Option<i64> {
        let min = self.min_cents.or(self.max_cents)?;
        Some(self.period.annualize(min))
    }

    pub fn is_empty(&self) -> bool {
        self.min_cents.is_none() && self.max_cents.is_none()
    }

    /// `"$185,000–$225,000/yr"`, or `"est. $95,000/yr"` when the site guessed.
    pub fn display(&self) -> String {
        let sym = currency_symbol(&self.currency);
        let fmt = |cents: i64| -> String {
            let units = cents / 100;
            let mut s = units.to_string();
            let mut i = s.len() as i32 - 3;
            while i > 0 {
                s.insert(i as usize, ',');
                i -= 3;
            }
            format!("{sym}{s}")
        };
        let body = match (self.min_cents, self.max_cents) {
            (Some(a), Some(b)) if a == b => fmt(a),
            (Some(a), Some(b)) => format!("{}–{}", fmt(a), fmt(b)),
            (Some(a), None) => format!("from {}", fmt(a)),
            (None, Some(b)) => format!("up to {}", fmt(b)),
            (None, None) => return "not stated".to_string(),
        };
        let out = format!("{body}{}", self.period.suffix());
        if self.is_estimate {
            format!("est. {out}")
        } else {
            out
        }
    }
}

/// A salary as it appeared before normalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSalary {
    pub text: String,
    /// Country hint used to disambiguate a bare `$`.
    pub country_hint: Option<String>,
    pub source_kind: Option<SourceKind>,
}

/// Sanity bounds. Anything outside these is a parsing failure, not a real offer, and is
/// discarded in favour of retaining the raw text.
pub const MIN_PLAUSIBLE_ANNUAL_CENTS: i64 = 1_000 * 100;
pub const MAX_PLAUSIBLE_ANNUAL_CENTS: i64 = 10_000_000 * 100;

pub fn is_plausible_annual(cents: i64) -> bool {
    (MIN_PLAUSIBLE_ANNUAL_CENTS..=MAX_PLAUSIBLE_ANNUAL_CENTS).contains(&cents)
}

pub fn currency_symbol(code: &str) -> &'static str {
    match code {
        "USD" | "CAD" | "AUD" | "NZD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" => "¥",
        "INR" => "₹",
        "CHF" => "CHF ",
        "SEK" | "NOK" | "DKK" => "kr ",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sal(min: Option<i64>, max: Option<i64>, period: SalaryPeriod, est: bool) -> Salary {
        Salary {
            min_cents: min,
            max_cents: max,
            currency: "USD".into(),
            period,
            is_estimate: est,
            raw: None,
        }
    }

    #[test]
    fn annualizes_across_periods() {
        let hourly = sal(Some(60_00), Some(60_00), SalaryPeriod::Hour, false);
        assert_eq!(hourly.annualized_max_cents(), Some(60_00 * 2080));

        let monthly = sal(Some(8_000_00), None, SalaryPeriod::Month, false);
        assert_eq!(monthly.annualized_max_cents(), Some(8_000_00 * 12));

        let yearly = sal(
            Some(185_000_00),
            Some(225_000_00),
            SalaryPeriod::Year,
            false,
        );
        assert_eq!(yearly.annualized_max_cents(), Some(225_000_00));
    }

    #[test]
    fn display_is_readable_and_marks_estimates() {
        assert_eq!(
            sal(
                Some(185_000_00),
                Some(225_000_00),
                SalaryPeriod::Year,
                false
            )
            .display(),
            "$185,000–$225,000/yr"
        );
        assert_eq!(
            sal(Some(95_000_00), None, SalaryPeriod::Year, true).display(),
            "est. from $95,000/yr"
        );
        assert_eq!(
            sal(Some(60_00), Some(60_00), SalaryPeriod::Hour, false).display(),
            "$60/hr"
        );
        assert_eq!(
            sal(None, None, SalaryPeriod::Year, false).display(),
            "not stated"
        );
    }

    #[test]
    fn implausible_figures_are_rejected() {
        assert!(!is_plausible_annual(500));
        assert!(!is_plausible_annual(50_000_000 * 100));
        assert!(is_plausible_annual(150_000 * 100));
    }
}
