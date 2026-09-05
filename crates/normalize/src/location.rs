//! Location parsing.
//!
//! `"Remote - US"`, `"San Francisco, CA (Hybrid)"` and `"Multiple Locations"` are three
//! different kinds of statement, and flattening them into one string loses the distinction
//! the user actually filters on. This module splits a location string into structured parts
//! and, critically, marks whether the row describes an office or a remote-eligibility
//! *region*.

use jobseeker_core::domain::enums::WorkMode;
use jobseeker_core::domain::location::RawLocation;
use once_cell::sync::Lazy;
use regex::Regex;

/// Structured location parts. Deliberately not `JobLocation`: that type needs a `JobId`,
/// which normalization has no business knowing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedLocation {
    pub raw: String,
    pub city: Option<String>,
    /// State / province code where one exists, else the region name as written.
    pub region: Option<String>,
    /// ISO 3166-1 alpha-2.
    pub country: Option<String>,
    pub postal_code: Option<String>,
    /// True when this names where you may *live* while remote, not where an office is.
    pub is_remote_scope: bool,
    /// Work mode implied by the location string itself, e.g. `"(Hybrid)"`.
    pub work_mode_hint: Option<WorkMode>,
    pub timezone_requirement: Option<String>,
}

/// Parse one location string.
///
/// Returns `None` only for strings that name no place at all (`"Multiple Locations"`), which
/// callers record as raw text rather than as a location.
pub fn parse_location(input: &RawLocation) -> Option<ParsedLocation> {
    let raw = input.text.trim();
    if raw.is_empty() {
        return None;
    }

    let mut out = ParsedLocation {
        raw: raw.to_string(),
        is_remote_scope: input.is_remote_hint,
        country: input.country_hint.clone(),
        ..Default::default()
    };

    let mut work = raw.to_string();

    // Parenthetical and trailing qualifiers carry the work mode: "Austin, TX (Remote)".
    if let Some(mode) = extract_work_mode(&work) {
        out.work_mode_hint = Some(mode);
        if mode == WorkMode::Remote {
            out.is_remote_scope = true;
        }
    }
    if let Some(tz) = extract_timezone(&work) {
        out.timezone_requirement = Some(tz);
    }
    work = strip_qualifiers(&work);

    if is_placeless(&work) {
        // "Remote" alone is a real statement (remote, scope unspecified); "Multiple
        // locations" names nothing at all.
        return out.is_remote_scope.then_some(out);
    }

    let parts: Vec<&str> = work
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();

    match parts.as_slice() {
        [city, region, country] => {
            out.city = Some(titlecase(city));
            out.region = Some(normalize_region(region, Some(country)));
            out.country = normalize_country(country).or(out.country);
        }
        [city, second] => {
            out.city = Some(titlecase(city));
            // The second part is either a state/province or a country: "San Francisco, CA"
            // vs "Berlin, Germany". Several codes are both — `CA` is California and Canada,
            // `IN` is Indiana and India — and in a `City, XX` pair the subdivision reading
            // is overwhelmingly the right one, so it is checked first.
            if let Some(cc) = infer_country_from_region(second) {
                out.region = Some(normalize_region(second, None));
                out.country = Some(cc);
            } else if let Some(cc) = normalize_country(second) {
                out.country = Some(cc);
            } else {
                out.region = Some(normalize_region(second, None));
            }
        }
        [single] => {
            // A lone token is read as a country first: "Remote - US" means the country, not
            // a city called US. Only a spelled-out subdivision name wins here.
            if let Some(cc) = normalize_country(single) {
                out.country = Some(cc);
            } else if let Some(cc) = infer_country_from_region(single) {
                out.region = Some(normalize_region(single, None));
                out.country = Some(cc);
            } else {
                out.city = Some(titlecase(single));
            }
        }
        _ => {
            // More than three comma-separated parts is usually a full postal address; the
            // last two are the informative ones.
            let n = parts.len();
            out.city = Some(titlecase(parts[n - 3]));
            out.region = Some(normalize_region(parts[n - 2], Some(parts[n - 1])));
            out.country = normalize_country(parts[n - 1]).or(out.country);
        }
    }

    out.postal_code = extract_postal_code(raw);
    if out.city.is_none() && out.region.is_none() && out.country.is_none() {
        return None;
    }
    Some(out)
}

/// Split a multi-location string (`"San Francisco, CA; New York, NY"` or
/// `"Austin, TX or Remote"`) into individual locations.
pub fn split_locations(input: &str) -> Vec<String> {
    static SEP: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\s*(?:;|\||\bor\b|/)\s*").unwrap());
    SEP.split(input)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn extract_work_mode(text: &str) -> Option<WorkMode> {
    static REMOTE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(fully[ -]?remote|remote|work from home|wfh|telecommute|distributed)\b")
            .unwrap()
    });
    static HYBRID: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(hybrid|flexible|partially remote|\d\s*days?\s*(?:(?:a|per)\s*(?:week|month)\s*)?(?:in|per|at|from)\s*(?:the\s*)?office)\b").unwrap());
    static ONSITE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(on[- ]?site|in[- ]?office|in[- ]?person)\b").unwrap());

    // Hybrid first: "Remote/Hybrid" and "Hybrid remote" are hybrid, and Indeed in
    // particular labels hybrid roles with the word "remote" in them.
    if HYBRID.is_match(text) {
        Some(WorkMode::Hybrid)
    } else if ONSITE.is_match(text) {
        Some(WorkMode::Onsite)
    } else if REMOTE.is_match(text) {
        Some(WorkMode::Remote)
    } else {
        None
    }
}

fn extract_timezone(text: &str) -> Option<String> {
    static TZ: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(?:(?:UTC|GMT)\s*[+-]\s*\d{1,2}|[PMCE][SD]T|CET|CEST|BST|IST|AEST)\b")
            .unwrap()
    });
    TZ.find(text).map(|m| m.as_str().to_uppercase())
}

/// Remove the qualifiers already harvested, leaving a bare place string to split on commas.
fn strip_qualifiers(text: &str) -> String {
    static PARENS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\([^)]*\)").unwrap());
    static PREFIX: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^\s*(?:fully[ -]?remote|remote|hybrid|on[- ]?site|in[- ]?office|telecommute|wfh)\s*(?:[-–:,]|\bin\b|\bwithin\b|\bfrom\b)?\s*")
            .unwrap()
    });
    static SUFFIX: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\s*[-–,]?\s*(?:fully[ -]?remote|remote|hybrid|on[- ]?site|in[- ]?office|telecommute|wfh)\s*$")
            .unwrap()
    });
    let s = PARENS.replace_all(text, " ");
    let s = PREFIX.replace(&s, "");
    let s = SUFFIX.replace(&s, "");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strings that name no specific place. These become raw-only rows so the posting still
/// reads correctly, without inventing a city.
fn is_placeless(text: &str) -> bool {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^\s*(multiple locations|various|various locations|anywhere|worldwide|global|globally|n/?a|tbd|flexible|multiple|-|)\s*$")
            .unwrap()
    });
    RE.is_match(text)
}

fn extract_postal_code(text: &str) -> Option<String> {
    static US_ZIP: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(\d{5})(?:-\d{4})?\b").unwrap());
    static UK: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b([A-Z]{1,2}\d[A-Z\d]?\s*\d[A-Z]{2})\b").unwrap());
    UK.captures(text)
        .map(|c| c[1].to_uppercase())
        .or_else(|| US_ZIP.captures(text).map(|c| c[1].to_string()))
}

/// US states and Canadian provinces, folded to their standard codes so `"California"` and
/// `"CA"` compare equal. Other regions keep their written form.
fn normalize_region(region: &str, country: Option<&str>) -> String {
    let r = region.trim();
    if r.len() == 2 && r.chars().all(|c| c.is_ascii_alphabetic()) {
        return r.to_uppercase();
    }
    let lower = r.to_lowercase();
    for (name, code) in US_STATES.iter().chain(CA_PROVINCES.iter()) {
        if lower == *name {
            return code.to_string();
        }
    }
    let _ = country;
    titlecase(r)
}

fn infer_country_from_region(region: &str) -> Option<String> {
    let r = region.trim();
    let lower = r.to_lowercase();
    let code = r.to_uppercase();
    if US_STATES.iter().any(|(n, c)| lower == *n || code == *c) {
        return Some("US".into());
    }
    if CA_PROVINCES.iter().any(|(n, c)| lower == *n || code == *c) {
        return Some("CA".into());
    }
    None
}

/// Country names and codes seen in postings. Only the ones that actually appear are listed;
/// an unrecognized country falls through to the region/city branches, which is safer than
/// guessing.
pub fn normalize_country(input: &str) -> Option<String> {
    let s = input.trim().trim_end_matches('.').to_lowercase();
    Some(
        match s.as_str() {
            "us" | "usa" | "u.s." | "u.s.a." | "united states" | "united states of america" => "US",
            "ca" | "can" | "canada" => "CA",
            "uk" | "gb" | "united kingdom" | "great britain" | "england" | "scotland"
            | "wales" => "GB",
            "de" | "germany" | "deutschland" => "DE",
            "fr" | "france" => "FR",
            "es" | "spain" => "ES",
            "it" | "italy" => "IT",
            "nl" | "netherlands" | "the netherlands" | "holland" => "NL",
            "ie" | "ireland" => "IE",
            "pt" | "portugal" => "PT",
            "pl" | "poland" => "PL",
            "se" | "sweden" => "SE",
            "no" | "norway" => "NO",
            "dk" | "denmark" => "DK",
            "fi" | "finland" => "FI",
            "ch" | "switzerland" => "CH",
            "at" | "austria" => "AT",
            "be" | "belgium" => "BE",
            "au" | "australia" => "AU",
            "nz" | "new zealand" => "NZ",
            "in" | "india" => "IN",
            "jp" | "japan" => "JP",
            "sg" | "singapore" => "SG",
            "br" | "brazil" => "BR",
            "mx" | "mexico" => "MX",
            "za" | "south africa" => "ZA",
            "il" | "israel" => "IL",
            "ae" | "uae" | "united arab emirates" => "AE",
            "remote" | "emea" | "apac" | "latam" | "americas" | "europe" => return None,
            _ => return None,
        }
        .to_string(),
    )
}

fn titlecase(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            // Hyphenated and apostrophed names need each part capitalized: "Winston-Salem",
            // "O'Fallon".
            let mut out = String::with_capacity(w.len());
            let mut capitalize = true;
            for ch in w.chars() {
                if capitalize {
                    out.extend(ch.to_uppercase());
                    capitalize = false;
                } else {
                    out.extend(ch.to_lowercase());
                }
                if ch == '-' || ch == '\'' || ch == '.' {
                    capitalize = true;
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

const US_STATES: &[(&str, &str)] = &[
    ("alabama", "AL"), ("alaska", "AK"), ("arizona", "AZ"), ("arkansas", "AR"),
    ("california", "CA"), ("colorado", "CO"), ("connecticut", "CT"), ("delaware", "DE"),
    ("florida", "FL"), ("georgia", "GA"), ("hawaii", "HI"), ("idaho", "ID"),
    ("illinois", "IL"), ("indiana", "IN"), ("iowa", "IA"), ("kansas", "KS"),
    ("kentucky", "KY"), ("louisiana", "LA"), ("maine", "ME"), ("maryland", "MD"),
    ("massachusetts", "MA"), ("michigan", "MI"), ("minnesota", "MN"), ("mississippi", "MS"),
    ("missouri", "MO"), ("montana", "MT"), ("nebraska", "NE"), ("nevada", "NV"),
    ("new hampshire", "NH"), ("new jersey", "NJ"), ("new mexico", "NM"), ("new york", "NY"),
    ("north carolina", "NC"), ("north dakota", "ND"), ("ohio", "OH"), ("oklahoma", "OK"),
    ("oregon", "OR"), ("pennsylvania", "PA"), ("rhode island", "RI"),
    ("south carolina", "SC"), ("south dakota", "SD"), ("tennessee", "TN"), ("texas", "TX"),
    ("utah", "UT"), ("vermont", "VT"), ("virginia", "VA"), ("washington", "WA"),
    ("west virginia", "WV"), ("wisconsin", "WI"), ("wyoming", "WY"),
    ("district of columbia", "DC"), ("washington dc", "DC"),
];

const CA_PROVINCES: &[(&str, &str)] = &[
    ("alberta", "AB"), ("british columbia", "BC"), ("manitoba", "MB"),
    ("new brunswick", "NB"), ("newfoundland and labrador", "NL"), ("nova scotia", "NS"),
    ("ontario", "ON"), ("prince edward island", "PE"), ("quebec", "QC"),
    ("saskatchewan", "SK"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> ParsedLocation {
        parse_location(&RawLocation::new(text))
            .unwrap_or_else(|| panic!("failed to parse {text:?}"))
    }

    #[test]
    fn parses_city_state_country() {
        let l = p("San Francisco, CA, US");
        assert_eq!(l.city.as_deref(), Some("San Francisco"));
        assert_eq!(l.region.as_deref(), Some("CA"));
        assert_eq!(l.country.as_deref(), Some("US"));
        assert!(!l.is_remote_scope);
    }

    #[test]
    fn a_two_part_string_distinguishes_a_state_from_a_country() {
        let us = p("San Francisco, CA");
        assert_eq!(us.region.as_deref(), Some("CA"));
        assert_eq!(us.country.as_deref(), Some("US"), "the state implies the country");

        let de = p("Berlin, Germany");
        assert_eq!(de.city.as_deref(), Some("Berlin"));
        assert_eq!(de.country.as_deref(), Some("DE"));
        assert_eq!(de.region, None, "Germany is not a region");
    }

    #[test]
    fn state_names_fold_to_codes_so_spellings_agree() {
        assert_eq!(p("Austin, Texas").region.as_deref(), Some("TX"));
        assert_eq!(p("Austin, TX").region.as_deref(), Some("TX"));
        assert_eq!(p("Toronto, Ontario").region.as_deref(), Some("ON"));
        assert_eq!(p("Toronto, Ontario").country.as_deref(), Some("CA"));
    }

    #[test]
    fn remote_scope_is_distinguished_from_an_office_location() {
        let scope = p("Remote - US");
        assert!(scope.is_remote_scope, "this says where you may live");
        assert_eq!(scope.country.as_deref(), Some("US"));
        assert_eq!(scope.city, None, "no city may be invented");
        assert_eq!(scope.work_mode_hint, Some(WorkMode::Remote));

        let office = p("San Francisco, CA");
        assert!(!office.is_remote_scope, "this says where the office is");
    }

    #[test]
    fn a_remote_office_hybrid_is_read_as_hybrid_not_remote() {
        // Aggregators label hybrid roles with the word "remote"; reading these as fully
        // remote is the single most annoying misclassification in a job search.
        let l = p("Austin, TX (Hybrid remote)");
        assert_eq!(l.work_mode_hint, Some(WorkMode::Hybrid));
        assert_eq!(l.city.as_deref(), Some("Austin"));
        assert_eq!(l.region.as_deref(), Some("TX"));
    }

    #[test]
    fn work_mode_qualifiers_are_stripped_from_the_place() {
        let l = p("New York, NY (On-site)");
        assert_eq!(l.city.as_deref(), Some("New York"));
        assert_eq!(l.work_mode_hint, Some(WorkMode::Onsite));
    }

    #[test]
    fn bare_remote_is_a_location_with_no_place() {
        let l = p("Remote");
        assert!(l.is_remote_scope);
        assert_eq!(l.city, None);
        assert_eq!(l.country, None, "scope unspecified must not become US");
    }

    #[test]
    fn placeless_strings_yield_nothing_rather_than_a_fake_city() {
        for text in ["Multiple Locations", "Various", "Anywhere", "TBD", "N/A"] {
            assert_eq!(
                parse_location(&RawLocation::new(text)),
                None,
                "{text:?} names no place"
            );
        }
    }

    #[test]
    fn timezone_requirements_are_captured() {
        let l = p("Remote (US, PST timezone)");
        assert_eq!(l.timezone_requirement.as_deref(), Some("PST"));
        assert!(l.is_remote_scope);

        let utc = p("Remote - Europe (UTC+1)");
        assert_eq!(utc.timezone_requirement.as_deref(), Some("UTC+1"));
    }

    #[test]
    fn postal_codes_are_extracted_when_present() {
        assert_eq!(p("Mountain View, CA 94043").postal_code.as_deref(), Some("94043"));
        assert_eq!(p("London, EC2A 4NE, UK").postal_code.as_deref(), Some("EC2A 4NE"));
        assert_eq!(p("Austin, TX").postal_code, None);
    }

    #[test]
    fn multi_location_strings_split_into_parts() {
        assert_eq!(
            split_locations("San Francisco, CA; New York, NY"),
            vec!["San Francisco, CA", "New York, NY"]
        );
        assert_eq!(split_locations("Austin, TX or Remote"), vec!["Austin, TX", "Remote"]);
    }

    #[test]
    fn a_full_address_keeps_the_informative_tail() {
        let l = p("1600 Amphitheatre Parkway, Mountain View, CA, US");
        assert_eq!(l.city.as_deref(), Some("Mountain View"));
        assert_eq!(l.region.as_deref(), Some("CA"));
        assert_eq!(l.country.as_deref(), Some("US"));
    }

    #[test]
    fn city_names_are_titlecased_including_hyphenated_ones() {
        assert_eq!(p("winston-salem, nc").city.as_deref(), Some("Winston-Salem"));
        assert_eq!(p("SAN FRANCISCO, CA").city.as_deref(), Some("San Francisco"));
    }

    #[test]
    fn a_country_hint_is_used_but_never_overrides_an_explicit_country() {
        let hinted = parse_location(&RawLocation {
            text: "Remote".into(),
            is_remote_hint: true,
            country_hint: Some("CA".into()),
        })
        .unwrap();
        assert_eq!(hinted.country.as_deref(), Some("CA"));

        let explicit = parse_location(&RawLocation {
            text: "Berlin, Germany".into(),
            is_remote_hint: false,
            country_hint: Some("US".into()),
        })
        .unwrap();
        assert_eq!(explicit.country.as_deref(), Some("DE"), "the string wins");
    }
}
