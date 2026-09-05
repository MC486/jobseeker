//! Salary parsing.
//!
//! The rule that shapes this whole module: **a wrong number is worse than no number.**
//! A misparsed salary silently poisons every filter and every comp subscore, and the user
//! has no way to notice. So every branch here either produces a figure it can defend or
//! returns `None` while retaining the raw text (FR-E-04).

use jobseeker_core::domain::enums::SourceKind;
use jobseeker_core::domain::salary::{is_plausible_annual, Salary, SalaryPeriod};
use once_cell::sync::Lazy;
use regex::Regex;

/// A number with an optional `k`/`m` suffix, optional currency symbol, and thousands
/// separators in either the `1,234.56` or `1.234,56` convention.
static AMOUNT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        (?P<symbol> [$€£¥₹] | (?:US|CA|AU|NZ)\$ )? \s*
        (?P<num> \d{1,3}(?:[,\s.]\d{3})+ (?:[.,]\d{1,2})? | \d+ (?:[.,]\d{1,2})? ) \s*
        (?P<mult> k|m )? \b
        ",
    )
    .unwrap()
});

static PERIOD_HINT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?ix)
        per \s+ (?P<a> hour|hr|day|week|wk|month|mo|year|yr|annum )
        | / \s* (?P<b> hour|hr|h|day|d|week|wk|w|month|mo|year|yr|y|annually|annum )\b
        | (?P<c> hourly|daily|weekly|monthly|yearly|annually|annual|per\ annum )
        | (?P<d> an\ hour|a\ year|a\ month|a\ week|a\ day )
        ",
    )
    .unwrap()
});

static ESTIMATE_HINT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(estimated|estimate|est\.|glassdoor est|indeed est|our estimate)\b").unwrap()
});

/// Phrases that mean "no figure was stated". Matching one of these is a successful parse
/// with an empty range, which is different from a failed parse.
static NO_FIGURE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(competitive|commensurate|doe|depending on experience|negotiable|tbd|market rate|not specified|undisclosed)\b").unwrap()
});

/// Parse a compensation string into a normalized [`Salary`].
///
/// `country_hint` disambiguates a bare `$` (US vs Canada vs Australia); `source` marks
/// figures from sites that publish their own estimates rather than the employer's band.
/// Returns `None` only when there is genuinely nothing to store — including the raw text,
/// which callers keep regardless.
pub fn parse_salary(
    text: &str,
    country_hint: Option<&str>,
    source: Option<SourceKind>,
) -> Option<Salary> {
    let raw = text.trim();
    if raw.is_empty() {
        return None;
    }

    let currency = detect_currency(raw, country_hint);
    let is_estimate =
        ESTIMATE_HINT.is_match(raw) || source.map(SourceKind::estimates_salary).unwrap_or(false);

    let mut amounts: Vec<i64> = AMOUNT
        .captures_iter(raw)
        .filter_map(|c| {
            let num = c.name("num")?.as_str();
            let mult = c.name("mult").map(|m| m.as_str().to_ascii_lowercase());
            to_cents(num, mult.as_deref())
        })
        .collect();

    // Equity percentages, "401k", years and headcounts all look like numbers. A figure with
    // no currency marker and no period marker is too ambiguous to trust as pay.
    let has_money_marker = raw.contains(['$', '€', '£', '¥', '₹'])
        || AMOUNT.captures_iter(raw).any(|c| c.name("mult").is_some())
        || PERIOD_HINT.is_match(raw);

    let period = detect_period(raw).unwrap_or_else(|| infer_period_from_magnitude(&amounts));

    if amounts.is_empty() || !has_money_marker {
        // "Competitive salary" is a real statement about the posting: record that we read a
        // compensation field and found no band, rather than pretending we never looked.
        if NO_FIGURE.is_match(raw) {
            return Some(Salary {
                min_cents: None,
                max_cents: None,
                currency,
                period: SalaryPeriod::Unknown,
                is_estimate,
                raw: Some(raw.to_string()),
            });
        }
        return None;
    }

    amounts.sort_unstable();
    amounts.dedup();
    // Discard figures that cannot be pay at the detected period. This is what stops
    // "401(k)" and "10% bonus" becoming a salary band.
    amounts.retain(|c| is_plausible_annual(period.annualize(*c)));
    if amounts.is_empty() {
        return Some(Salary {
            min_cents: None,
            max_cents: None,
            currency,
            period: SalaryPeriod::Unknown,
            is_estimate,
            raw: Some(raw.to_string()),
        });
    }

    let (min, max) = match amounts.len() {
        1 => {
            let only = amounts[0];
            if looks_like_minimum(raw) {
                (Some(only), None)
            } else if looks_like_maximum(raw) {
                (None, Some(only))
            } else {
                (Some(only), Some(only))
            }
        }
        _ => (Some(amounts[0]), Some(*amounts.last().unwrap())),
    };

    Some(Salary {
        min_cents: min,
        max_cents: max,
        currency,
        period,
        is_estimate,
        raw: Some(raw.to_string()),
    })
}

/// `"185"` + `Some("k")` → 18_500_000 cents. Handles both thousands conventions.
fn to_cents(num: &str, mult: Option<&str>) -> Option<i64> {
    let cleaned = disambiguate_separators(num)?;
    let value: f64 = cleaned.parse().ok()?;
    let scaled = match mult {
        Some("k") => value * 1_000.0,
        Some("m") => value * 1_000_000.0,
        _ => value,
    };
    if !scaled.is_finite() || scaled < 0.0 {
        return None;
    }
    Some((scaled * 100.0).round() as i64)
}

/// `1,234.56` (US) and `1.234,56` (EU) both appear in postings. Decide which separator is
/// decimal by looking at what follows it: exactly two digits at the end of the string is a
/// decimal, three digits is a thousands group.
fn disambiguate_separators(num: &str) -> Option<String> {
    let s: String = num.chars().filter(|c| !c.is_whitespace()).collect();
    let last_comma = s.rfind(',');
    let last_dot = s.rfind('.');

    let decimal_sep = match (last_comma, last_dot) {
        (Some(c), Some(d)) => Some(if c > d { ',' } else { '.' }),
        (Some(c), None) => {
            let tail = s.len() - c - 1;
            (tail == 1 || tail == 2).then_some(',')
        }
        (None, Some(d)) => {
            let tail = s.len() - d - 1;
            (tail == 1 || tail == 2).then_some('.')
        }
        (None, None) => None,
    };

    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            ',' | '.' if Some(ch) == decimal_sep => out.push('.'),
            ',' | '.' => {}
            d if d.is_ascii_digit() => out.push(d),
            _ => return None,
        }
    }
    (!out.is_empty()).then_some(out)
}

fn detect_period(text: &str) -> Option<SalaryPeriod> {
    let c = PERIOD_HINT.captures(text)?;
    let word = ["a", "b", "c", "d"]
        .iter()
        .find_map(|n| c.name(n))
        .map(|m| m.as_str().to_ascii_lowercase())?;
    Some(match word.trim() {
        "hour" | "hr" | "h" | "hourly" | "an hour" => SalaryPeriod::Hour,
        "day" | "d" | "daily" | "a day" => SalaryPeriod::Day,
        "week" | "wk" | "w" | "weekly" | "a week" => SalaryPeriod::Week,
        "month" | "mo" | "monthly" | "a month" => SalaryPeriod::Month,
        _ => SalaryPeriod::Year,
    })
}

/// With no stated period, magnitude is the only evidence available. The bands are wide
/// enough that a misclassification needs a genuinely unusual figure.
fn infer_period_from_magnitude(amounts: &[i64]) -> SalaryPeriod {
    match amounts.iter().copied().max() {
        Some(max) if max < 500_00 => SalaryPeriod::Hour,
        Some(max) if max < 20_000_00 => SalaryPeriod::Month,
        Some(_) => SalaryPeriod::Year,
        None => SalaryPeriod::Unknown,
    }
}

fn looks_like_minimum(text: &str) -> bool {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(from|starting at|at least|minimum|min\.?|\+)\b").unwrap());
    RE.is_match(text)
}

fn looks_like_maximum(text: &str) -> bool {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(up to|max\.?|maximum|no more than)\b").unwrap());
    RE.is_match(text)
}

/// ISO 4217 code for the figure. A bare `$` is ambiguous, so the country hint decides;
/// without one, USD is the pragmatic default and the raw text is retained either way.
pub fn detect_currency(text: &str, country_hint: Option<&str>) -> String {
    static CODE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?i)\b(USD|CAD|AUD|NZD|EUR|GBP|JPY|INR|CHF|SEK|NOK|DKK|PLN|BRL|MXN|SGD|HKD|ZAR)\b",
        )
        .unwrap()
    });
    if let Some(m) = CODE.find(text) {
        return m.as_str().to_uppercase();
    }
    if text.contains('€') {
        return "EUR".into();
    }
    if text.contains('£') {
        return "GBP".into();
    }
    if text.contains('₹') {
        return "INR".into();
    }
    if text.contains('¥') {
        return "JPY".into();
    }
    if text.contains("C$") || text.contains("CA$") {
        return "CAD".into();
    }
    if text.contains("A$") || text.contains("AU$") {
        return "AUD".into();
    }
    match country_hint.map(str::to_uppercase).as_deref() {
        Some("CA") => "CAD".into(),
        Some("AU") => "AUD".into(),
        Some("NZ") => "NZD".into(),
        Some("GB") | Some("UK") => "GBP".into(),
        Some("IN") => "INR".into(),
        Some("JP") => "JPY".into(),
        Some("CH") => "CHF".into(),
        Some("DE") | Some("FR") | Some("ES") | Some("IT") | Some("NL") | Some("IE")
        | Some("PT") | Some("AT") | Some("BE") | Some("FI") => "EUR".into(),
        _ => "USD".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Salary {
        parse_salary(text, None, None).unwrap_or_else(|| panic!("failed to parse {text:?}"))
    }

    #[test]
    fn parses_the_common_annual_range_spellings() {
        for text in [
            "$185,000 - $225,000 per year",
            "$185,000–$225,000/yr",
            "$185k - $225k annually",
            "185,000 - 225,000 USD per year",
            "USD 185000 to 225000 a year",
        ] {
            let s = p(text);
            assert_eq!(s.min_cents, Some(185_000_00), "min for {text:?}");
            assert_eq!(s.max_cents, Some(225_000_00), "max for {text:?}");
            assert_eq!(s.period, SalaryPeriod::Year, "period for {text:?}");
            assert_eq!(s.currency, "USD");
        }
    }

    #[test]
    fn parses_hourly_and_keeps_the_period_rather_than_annualizing() {
        let s = p("$60.00 - $75.00 per hour");
        assert_eq!((s.min_cents, s.max_cents), (Some(60_00), Some(75_00)));
        assert_eq!(s.period, SalaryPeriod::Hour);
        // Comparison happens on demand, so the stored record stays faithful to the posting.
        assert_eq!(s.annualized_max_cents(), Some(75_00 * 2080));
    }

    #[test]
    fn a_single_figure_becomes_a_point_not_a_fabricated_range() {
        let s = p("$150,000 per year");
        assert_eq!(
            (s.min_cents, s.max_cents),
            (Some(150_000_00), Some(150_000_00))
        );
    }

    #[test]
    fn open_ended_phrasing_is_preserved_as_open_ended() {
        let from = p("From $150,000 a year");
        assert_eq!((from.min_cents, from.max_cents), (Some(150_000_00), None));

        let up_to = p("Up to $220,000 per year");
        assert_eq!((up_to.min_cents, up_to.max_cents), (None, Some(220_000_00)));
    }

    #[test]
    fn currency_is_detected_from_symbol_code_or_country() {
        assert_eq!(p("€80.000 - €95.000 pro Jahr").currency, "EUR");
        assert_eq!(p("£65,000 - £80,000 per annum").currency, "GBP");
        assert_eq!(p("C$120,000/yr").currency, "CAD");
        assert_eq!(
            parse_salary("$120,000/yr", Some("CA"), None)
                .unwrap()
                .currency,
            "CAD",
            "a bare dollar sign on a Canadian posting is CAD"
        );
        assert_eq!(p("CHF 140,000 per year").currency, "CHF");
    }

    #[test]
    fn european_separator_convention_is_handled() {
        let s = p("€80.000 - €95.000 pro Jahr");
        assert_eq!(
            (s.min_cents, s.max_cents),
            (Some(80_000_00), Some(95_000_00))
        );

        let decimal = p("€28,50 per hour");
        assert_eq!(decimal.min_cents, Some(28_50));
    }

    #[test]
    fn site_estimates_are_flagged_so_they_can_be_distrusted() {
        let explicit = p("Estimated $120,000 - $150,000 a year");
        assert!(explicit.is_estimate);

        let by_source =
            parse_salary("$120,000 - $150,000 a year", None, Some(SourceKind::Indeed)).unwrap();
        assert!(
            by_source.is_estimate,
            "Indeed figures are frequently its own estimate"
        );

        let ats = parse_salary(
            "$120,000 - $150,000 a year",
            None,
            Some(SourceKind::Greenhouse),
        )
        .unwrap();
        assert!(!ats.is_estimate, "an employer's own ATS states real bands");
    }

    #[test]
    fn prose_without_a_figure_records_that_we_looked() {
        let s = p("Competitive salary, commensurate with experience");
        assert!(s.is_empty(), "no band");
        assert!(s.raw.is_some(), "but the statement is retained");
        assert_eq!(s.period, SalaryPeriod::Unknown);
    }

    #[test]
    fn non_salary_numbers_do_not_become_a_salary() {
        // These are the false positives that would silently corrupt every salary filter.
        assert_eq!(parse_salary("401(k) with 4% match", None, None), None);
        assert_eq!(parse_salary("0.1% - 0.5% equity", None, None), None);
        assert_eq!(parse_salary("5+ years of experience", None, None), None);
        assert_eq!(parse_salary("Team of 12 engineers", None, None), None);
        assert_eq!(parse_salary("", None, None), None);
    }

    #[test]
    fn implausible_figures_are_dropped_but_the_text_survives() {
        let s = p("$5 - $12 per year");
        assert!(
            s.is_empty(),
            "nobody is paid $5/yr; this is a parse failure"
        );
        assert_eq!(s.raw.as_deref(), Some("$5 - $12 per year"));
    }

    #[test]
    fn a_bare_hourly_magnitude_is_inferred_when_no_period_is_stated() {
        let s = p("$65/hr");
        assert_eq!(s.period, SalaryPeriod::Hour);
        assert_eq!(s.min_cents, Some(65_00));
    }

    #[test]
    fn magnitude_infers_the_period_when_the_posting_omits_it() {
        assert_eq!(p("$185,000").period, SalaryPeriod::Year);
        assert_eq!(p("$8,500 monthly").period, SalaryPeriod::Month);
    }

    #[test]
    fn ranges_are_ordered_even_when_written_backwards() {
        let s = p("$225,000 to $185,000 per year");
        assert!(
            s.min_cents <= s.max_cents,
            "the range must come out ordered"
        );
        assert_eq!(s.min_cents, Some(185_000_00));
    }

    #[test]
    fn separator_disambiguation_is_correct_in_isolation() {
        assert_eq!(
            disambiguate_separators("1,234.56").as_deref(),
            Some("1234.56")
        );
        assert_eq!(
            disambiguate_separators("1.234,56").as_deref(),
            Some("1234.56")
        );
        assert_eq!(
            disambiguate_separators("185,000").as_deref(),
            Some("185000")
        );
        assert_eq!(disambiguate_separators("80.000").as_deref(), Some("80000"));
        assert_eq!(disambiguate_separators("28,50").as_deref(), Some("28.50"));
    }
}
