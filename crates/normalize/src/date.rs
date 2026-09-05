//! Date parsing.
//!
//! Postings state dates three ways: an ISO timestamp (ATS APIs), a written date
//! (`"September 2, 2026"`), or a relative age (`"3 days ago"`, `"Posted 30+ days ago"`).
//! Relative ages are only as precise as the phrase, so [`PartialDate`] records the
//! precision alongside the instant — without it, "30+ days ago" silently becomes an exact
//! timestamp and every "posted this week" filter lies.

use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use jobseeker_core::time::{DatePrecision, PartialDate, Timestamp};
use once_cell::sync::Lazy;
use regex::Regex;

/// Parse a posting date relative to `now`, which is passed in so the function stays pure
/// and relative-age parsing is testable.
pub fn parse_posted_date(text: &str, now: Timestamp) -> Option<PartialDate> {
    let s = text.trim();
    if s.is_empty() {
        return None;
    }
    parse_iso(s)
        .or_else(|| parse_relative(s, now))
        .or_else(|| parse_written(s))
        .or_else(|| parse_numeric(s))
}

/// An exact instant from a machine-generated field.
fn parse_iso(s: &str) -> Option<PartialDate> {
    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(PartialDate {
            at: ts.with_timezone(&Utc),
            precision: DatePrecision::Exact,
        });
    }
    // ATS APIs also emit "2026-09-02T18:14:02" with no zone, and bare dates.
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(PartialDate {
            at: Utc.from_utc_datetime(&dt),
            precision: DatePrecision::Exact,
        });
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(day(d));
    }
    // "2026-09" — a month with no day, which must stay month-precision.
    static YM: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d{4})-(\d{1,2})$").unwrap());
    if let Some(c) = YM.captures(s) {
        let (y, m) = (c[1].parse().ok()?, c[2].parse().ok()?);
        let d = NaiveDate::from_ymd_opt(y, m, 1)?;
        return Some(PartialDate {
            at: Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?),
            precision: DatePrecision::Month,
        });
    }
    None
}

/// `"3 days ago"`, `"Posted 30+ days ago"`, `"today"`, `"Reposted 2 weeks ago"`.
///
/// The `+` in `"30+ days ago"` means the site has stopped counting: that is recorded as
/// month precision so nothing downstream treats it as an exact date.
fn parse_relative(s: &str, now: Timestamp) -> Option<PartialDate> {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?ix)
            \b(?P<n> \d{1,3} ) \s* (?P<plus> \+ )? \s*
            (?P<unit> minute|min|hour|hr|day|week|month|year )s?
            \s* (?:ago|old)
            ",
        )
        .unwrap()
    });
    static JUST_NOW: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(just posted|just now|today|new|posted today|moments? ago|a few (?:seconds|minutes) ago)\b")
            .unwrap()
    });
    static YESTERDAY: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\byesterday\b").unwrap());

    if JUST_NOW.is_match(s) {
        return Some(PartialDate {
            at: now,
            precision: DatePrecision::Day,
        });
    }
    if YESTERDAY.is_match(s) {
        return Some(PartialDate {
            at: now - Duration::days(1),
            precision: DatePrecision::Day,
        });
    }

    let c = RE.captures(s)?;
    let n: i64 = c.name("n")?.as_str().parse().ok()?;
    let unit = c.name("unit")?.as_str().to_lowercase();
    let capped = c.name("plus").is_some();

    let delta = match unit.as_str() {
        "minute" | "min" => Duration::minutes(n),
        "hour" | "hr" => Duration::hours(n),
        "day" => Duration::days(n),
        "week" => Duration::weeks(n),
        "month" => Duration::days(n * 30),
        "year" => Duration::days(n * 365),
        _ => return None,
    };

    // "3 hours ago" is precise to the hour; "30+ days ago" is a floor, not a date.
    let precision = if capped {
        // "30+ days ago" is the site refusing to say: a lower bound, not a date.
        DatePrecision::AtLeast
    } else {
        match unit.as_str() {
            "minute" | "min" | "hour" | "hr" => DatePrecision::Exact,
            "day" | "week" => DatePrecision::Day,
            "month" => DatePrecision::Month,
            _ => DatePrecision::Unknown,
        }
    };

    Some(PartialDate {
        at: now - delta,
        precision,
    })
}

/// `"September 2, 2026"`, `"2 Sep 2026"`, `"Sep 2026"`.
fn parse_written(s: &str) -> Option<PartialDate> {
    static MONTH_DAY_YEAR: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(?P<mon>[a-z]{3,9})\.?\s+(?P<day>\d{1,2})(?:st|nd|rd|th)?,?\s+(?P<year>\d{4})\b").unwrap()
    });
    static DAY_MONTH_YEAR: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(?P<day>\d{1,2})(?:st|nd|rd|th)?\s+(?P<mon>[a-z]{3,9})\.?,?\s+(?P<year>\d{4})\b").unwrap()
    });
    static MONTH_YEAR: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(?P<mon>[a-z]{3,9})\.?\s+(?P<year>\d{4})\b").unwrap());

    for re in [&*MONTH_DAY_YEAR, &*DAY_MONTH_YEAR] {
        if let Some(c) = re.captures(s) {
            let month = month_number(c.name("mon")?.as_str())?;
            let d: u32 = c.name("day")?.as_str().parse().ok()?;
            let y: i32 = c.name("year")?.as_str().parse().ok()?;
            return NaiveDate::from_ymd_opt(y, month, d).map(day);
        }
    }
    if let Some(c) = MONTH_YEAR.captures(s) {
        let month = month_number(c.name("mon")?.as_str())?;
        let y: i32 = c.name("year")?.as_str().parse().ok()?;
        let d = NaiveDate::from_ymd_opt(y, month, 1)?;
        return Some(PartialDate {
            at: Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?),
            precision: DatePrecision::Month,
        });
    }
    None
}

/// `"09/02/2026"`. Ambiguous between US and European ordering: when the first number
/// cannot be a month it is read as a day, and otherwise US ordering is assumed because the
/// posting corpus is US-dominant. The `raw` text is always retained so a wrong guess is
/// visible and correctable.
fn parse_numeric(s: &str) -> Option<PartialDate> {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\b(\d{1,2})[/.-](\d{1,2})[/.-](\d{2,4})\b").unwrap());
    let c = RE.captures(s)?;
    let (a, b): (u32, u32) = (c[1].parse().ok()?, c[2].parse().ok()?);
    let mut y: i32 = c[3].parse().ok()?;
    if y < 100 {
        y += 2000;
    }
    let (month, dom) = if a > 12 { (b, a) } else { (a, b) };
    NaiveDate::from_ymd_opt(y, month, dom).map(day)
}

fn day(d: NaiveDate) -> PartialDate {
    PartialDate {
        at: Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight is always valid")),
        precision: DatePrecision::Day,
    }
}

fn month_number(name: &str) -> Option<u32> {
    let n = name.to_lowercase();
    const MONTHS: [&str; 12] = [
        "january", "february", "march", "april", "may", "june", "july", "august", "september",
        "october", "november", "december",
    ];
    MONTHS
        .iter()
        .position(|m| m.starts_with(&n[..n.len().min(3)]) && n.len() >= 3 && m.starts_with(&n) || *m == n)
        .map(|i| i as u32 + 1)
        .or_else(|| {
            MONTHS
                .iter()
                .position(|m| n.len() >= 3 && m.starts_with(&n[..3]))
                .map(|i| i as u32 + 1)
        })
}

/// An application deadline, which unlike a posting date is always in the future and is
/// frequently written as `"Applications close October 15"` with no year.
pub fn parse_close_date(text: &str, now: Timestamp) -> Option<PartialDate> {
    static NO_YEAR: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(?P<mon>[a-z]{3,9})\.?\s+(?P<day>\d{1,2})(?:st|nd|rd|th)?\b").unwrap()
    });

    if let Some(d) = parse_posted_date(text, now) {
        return Some(d);
    }
    // A bare "October 15" means the next October 15, not one in the past.
    let c = NO_YEAR.captures(text)?;
    let month = month_number(c.name("mon")?.as_str())?;
    let dom: u32 = c.name("day")?.as_str().parse().ok()?;
    let this_year = NaiveDate::from_ymd_opt(now.year(), month, dom)?;
    let resolved = if this_year < now.date_naive() {
        NaiveDate::from_ymd_opt(now.year() + 1, month, dom)?
    } else {
        this_year
    };
    Some(day(resolved))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        Utc.with_ymd_and_hms(2026, 9, 5, 12, 0, 0).unwrap()
    }

    fn parsed(s: &str) -> PartialDate {
        parse_posted_date(s, now()).unwrap_or_else(|| panic!("failed to parse {s:?}"))
    }

    #[test]
    fn iso_timestamps_keep_second_precision() {
        let d = parsed("2026-09-02T18:14:02Z");
        assert_eq!(d.precision, DatePrecision::Exact);
        assert_eq!(d.at.date_naive(), NaiveDate::from_ymd_opt(2026, 9, 2).unwrap());
    }

    #[test]
    fn a_bare_iso_date_is_day_precision_not_midnight_exactly() {
        let d = parsed("2026-09-02");
        assert_eq!(d.precision, DatePrecision::Day);
    }

    #[test]
    fn a_year_month_stays_month_precision() {
        let d = parsed("2026-09");
        assert_eq!(d.precision, DatePrecision::Month);
        assert_eq!(d.at.day(), 1);
    }

    #[test]
    fn relative_ages_are_resolved_against_now() {
        assert_eq!(
            parsed("3 days ago").at.date_naive(),
            NaiveDate::from_ymd_opt(2026, 9, 2).unwrap()
        );
        assert_eq!(
            parsed("Posted 2 weeks ago").at.date_naive(),
            NaiveDate::from_ymd_opt(2026, 8, 22).unwrap()
        );
        assert_eq!(parsed("Just posted").at.date_naive(), now().date_naive());
        assert_eq!(
            parsed("yesterday").at.date_naive(),
            NaiveDate::from_ymd_opt(2026, 9, 4).unwrap()
        );
    }

    #[test]
    fn a_capped_age_is_recorded_as_imprecise_not_exact() {
        // "30+ days ago" is the site refusing to say. Treating it as exactly 30 days would
        // make every "posted in the last month" filter wrong.
        let d = parsed("Posted 30+ days ago");
        assert_eq!(d.precision, DatePrecision::AtLeast);
        assert!(d.at < now() - Duration::days(29));
    }

    #[test]
    fn hour_level_ages_keep_finer_precision_than_day_level_ones() {
        assert_eq!(parsed("5 hours ago").precision, DatePrecision::Exact);
        assert_eq!(parsed("5 days ago").precision, DatePrecision::Day);
        assert_eq!(parsed("5 months ago").precision, DatePrecision::Month);
    }

    #[test]
    fn written_dates_parse_in_both_orderings() {
        let expected = NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
        for s in [
            "September 2, 2026",
            "Sep 2, 2026",
            "Sept. 2 2026",
            "2 September 2026",
            "2nd Sep 2026",
        ] {
            assert_eq!(parsed(s).at.date_naive(), expected, "input: {s:?}");
        }
    }

    #[test]
    fn a_month_and_year_without_a_day_stays_month_precision() {
        let d = parsed("September 2026");
        assert_eq!(d.precision, DatePrecision::Month);
        assert_eq!(d.at.month(), 9);
    }

    #[test]
    fn unambiguous_numeric_dates_are_read_correctly() {
        // 25 cannot be a month, so this is day-first regardless of locale.
        assert_eq!(
            parsed("25/12/2026").at.date_naive(),
            NaiveDate::from_ymd_opt(2026, 12, 25).unwrap()
        );
        assert_eq!(
            parsed("09/02/2026").at.date_naive(),
            NaiveDate::from_ymd_opt(2026, 9, 2).unwrap(),
            "US ordering is assumed for ambiguous dates"
        );
    }

    #[test]
    fn unparseable_text_yields_none_rather_than_today() {
        assert_eq!(parse_posted_date("", now()), None);
        assert_eq!(parse_posted_date("sometime soon", now()), None);
        assert_eq!(parse_posted_date("ASAP", now()), None);
    }

    #[test]
    fn a_deadline_without_a_year_resolves_to_the_future() {
        let soon = parse_close_date("Applications close October 15", now()).unwrap();
        assert_eq!(soon.at.year(), 2026, "October is still ahead of September");

        let wrapped = parse_close_date("Applications close January 10", now()).unwrap();
        assert_eq!(wrapped.at.year(), 2027, "January must not resolve into the past");
    }

    #[test]
    fn a_deadline_with_an_explicit_date_is_taken_literally() {
        let d = parse_close_date("2026-10-15", now()).unwrap();
        assert_eq!(d.at.date_naive(), NaiveDate::from_ymd_opt(2026, 10, 15).unwrap());
    }
}
