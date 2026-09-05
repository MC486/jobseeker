//! Inference from titles and prose.
//!
//! These are the `Provenance::Inferred` stage: the weakest signal in the pipeline, used only
//! when no source stated the field outright. Each function returns `None` rather than a
//! default, so "we do not know" stays distinguishable from "the posting said mid-level".

use jobseeker_core::domain::enums::{EmploymentType, Seniority, Tristate, WorkMode};
use once_cell::sync::Lazy;
use regex::Regex;

/// Seniority from a job title.
///
/// Order matters: `"Senior Engineering Manager"` is a manager, and `"Staff Engineer"` must
/// not be read as entry-level because it contains "staff".
pub fn infer_seniority(title: &str) -> Option<Seniority> {
    static INTERN: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(intern|internship|co[- ]?op|apprentice|trainee)\b").unwrap());
    static EXEC: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(chief\s+\w+\s+officer|cto|ceo|coo|cfo|ciso|cpo|founder|head of engineering)\b").unwrap()
    });
    static VP: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(vp|vice president|svp|evp)\b").unwrap());
    static DIRECTOR: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(director|head of)\b").unwrap());
    static MANAGER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(engineering manager|manager|em)\b").unwrap()
    });
    static PRINCIPAL: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(principal|distinguished|fellow|architect)\b").unwrap());
    static STAFF: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bstaff\b").unwrap());
    static LEAD: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(lead|tech lead|technical lead)\b").unwrap());
    static SENIOR: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(senior|snr|sr\.?|experienced)\b|\bi{3,}\b|\biv\b").unwrap());
    static JUNIOR: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(junior|jr\.?|associate)\b").unwrap());
    static ENTRY: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(entry[- ]level|new grad|graduate|early career|i)\b").unwrap()
    });
    static MID: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(mid[- ]level|intermediate|ii)\b").unwrap());

    // Intern before everything: a "Senior Software Engineering Intern" is an intern.
    if INTERN.is_match(title) {
        return Some(Seniority::Intern);
    }
    if EXEC.is_match(title) {
        return Some(Seniority::Exec);
    }
    if VP.is_match(title) {
        return Some(Seniority::Vp);
    }
    if DIRECTOR.is_match(title) {
        return Some(Seniority::Director);
    }
    if MANAGER.is_match(title) {
        return Some(Seniority::Manager);
    }
    if PRINCIPAL.is_match(title) {
        return Some(Seniority::Principal);
    }
    if STAFF.is_match(title) {
        return Some(Seniority::Staff);
    }
    if LEAD.is_match(title) {
        return Some(Seniority::Lead);
    }
    if SENIOR.is_match(title) {
        return Some(Seniority::Senior);
    }
    if JUNIOR.is_match(title) {
        return Some(Seniority::Junior);
    }
    if MID.is_match(title) {
        return Some(Seniority::Mid);
    }
    if ENTRY.is_match(title) {
        return Some(Seniority::Entry);
    }
    None
}

/// Work mode from a title, location string or description snippet.
///
/// Hybrid is checked first for the same reason as in location parsing: postings advertise
/// "Hybrid remote" and "Remote (2 days in office)", and reading those as fully remote is
/// the misclassification that wastes the most of a job seeker's time.
pub fn infer_work_mode(text: &str) -> Option<WorkMode> {
    static HYBRID: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(hybrid|\d\s*days?\s*(?:(?:a|per)\s*(?:week|month)\s*)?(?:in|per|at|from)\s*(?:the\s*)?office|partially remote|part[- ]remote|flex(?:ible)? (?:work )?(?:location|arrangement))\b")
            .unwrap()
    });
    static ONSITE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(on[- ]?site|in[- ]?office|in[- ]?person|not remote|no remote|relocation required|must be (?:located|based) in)\b").unwrap()
    });
    static REMOTE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(fully[- ]remote|100%\s*remote|remote[- ]first|remote|work from home|wfh|telecommut\w*|distributed team|anywhere)\b").unwrap()
    });

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

pub fn infer_employment_type(text: &str) -> Option<EmploymentType> {
    static INTERN: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(internship|intern|co[- ]?op)\b").unwrap());
    static C2H: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(contract[- ]to[- ]hire|c2h|contract to perm|temp[- ]to[- ]perm)\b").unwrap()
    });
    static CONTRACT: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(contract|contractor|freelance|1099|w2 contract|consulting|sow)\b").unwrap()
    });
    static TEMP: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(temporary|temp|seasonal)\b").unwrap());
    static PART: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(part[- ]time|parttime|pt)\b").unwrap());
    static VOLUNTEER: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(volunteer|unpaid|pro bono)\b").unwrap());
    static FULL: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(full[- ]time|fulltime|ft|permanent|perm)\b").unwrap()
    });

    if INTERN.is_match(text) {
        Some(EmploymentType::Internship)
    } else if C2H.is_match(text) {
        Some(EmploymentType::ContractToHire)
    } else if CONTRACT.is_match(text) {
        Some(EmploymentType::Contract)
    } else if TEMP.is_match(text) {
        Some(EmploymentType::Temporary)
    } else if VOLUNTEER.is_match(text) {
        Some(EmploymentType::Volunteer)
    } else if PART.is_match(text) {
        Some(EmploymentType::PartTime)
    } else if FULL.is_match(text) {
        Some(EmploymentType::FullTime)
    } else {
        None
    }
}

/// Security clearance demanded by the posting, normalized to a comparable key. A clearance
/// you do not hold is a hard blocker, so detecting it is worth more than most fields.
pub fn detect_clearance(text: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?ix)
            \b(?:
              (?P<tssci> ts\s*/\s*sci | top\ secret\s*/\s*sci | ts\ sci )
            | (?P<poly> (?:with\ )?(?:ci|full\ scope|fs)\ poly\w* )
            | (?P<ts> top\ secret | \bts\b )
            | (?P<secret> \bsecret\b )
            | (?P<publictrust> public\ trust )
            | (?P<q> \bq\ clearance\b )
            )\b
            ",
        )
        .unwrap()
    });
    static NEGATED: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(no clearance (?:is )?(?:required|needed)|clearance not required|do(?:es)? not require (?:a )?clearance)\b").unwrap()
    });

    // "No clearance required" contains the word "clearance"; a naive match would invent a
    // blocker out of a statement that there is none.
    if NEGATED.is_match(text) {
        return None;
    }
    let c = RE.captures(text)?;
    Some(
        if c.name("tssci").is_some() {
            "ts_sci"
        } else if c.name("poly").is_some() {
            "ts_sci_poly"
        } else if c.name("ts").is_some() {
            "top_secret"
        } else if c.name("q").is_some() {
            "doe_q"
        } else if c.name("publictrust").is_some() {
            "public_trust"
        } else {
            "secret"
        }
        .to_string(),
    )
}

/// Visa sponsorship stance. `Unspecified` is the honest answer for most postings and is
/// materially different from "no".
pub fn detect_visa_sponsorship(text: &str) -> Tristate {
    static YES: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(will sponsor|sponsorship (?:is )?available|we sponsor|visa sponsorship provided|open to sponsorship|h-?1b (?:transfer|sponsorship) (?:available|offered))\b").unwrap()
    });
    static NO: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(no (?:visa )?sponsorship|unable to sponsor|cannot sponsor|not able to sponsor|sponsorship (?:is )?not available|must be (?:legally )?authorized to work .{0,40}without sponsorship|no c2c)\b").unwrap()
    });
    // Check the refusal first: "we are unable to provide visa sponsorship" contains
    // "sponsorship available" fragments under loose matching.
    if NO.is_match(text) {
        Tristate::No
    } else if YES.is_match(text) {
        Tristate::Yes
    } else {
        Tristate::Unspecified
    }
}

/// Years of experience demanded, as `(min, max)`. `"5-7 years"` → `(5, Some(7))`,
/// `"5+ years"` → `(5, None)`.
pub fn extract_years(text: &str) -> Option<(f32, Option<f32>)> {
    static RANGE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(\d{1,2})\s*(?:-|–|to)\s*(\d{1,2})\+?\s*(?:\+|or more)?\s*years?\b").unwrap()
    });
    static MIN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(?:at least\s*|minimum (?:of )?\s*|min\.?\s*|over\s*|)(\d{1,2})\s*\+?\s*(?:or more\s*)?years?\b").unwrap()
    });
    static WORDS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(one|two|three|four|five|six|seven|eight|nine|ten)\s*\+?\s*years?\b").unwrap()
    });

    if let Some(c) = RANGE.captures(text) {
        let a: f32 = c[1].parse().ok()?;
        let b: f32 = c[2].parse().ok()?;
        return Some((a.min(b), Some(a.max(b))));
    }
    if let Some(c) = MIN.captures(text) {
        let n: f32 = c[1].parse().ok()?;
        return Some((n, None));
    }
    if let Some(c) = WORDS.captures(text) {
        let n = match c[1].to_lowercase().as_str() {
            "one" => 1.0,
            "two" => 2.0,
            "three" => 3.0,
            "four" => 4.0,
            "five" => 5.0,
            "six" => 6.0,
            "seven" => 7.0,
            "eight" => 8.0,
            "nine" => 9.0,
            _ => 10.0,
        };
        return Some((n, None));
    }
    None
}

/// Minimum education level demanded.
pub fn detect_education(text: &str) -> Option<jobseeker_core::domain::enums::EducationLevel> {
    use jobseeker_core::domain::enums::EducationLevel as E;
    static PHD: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(ph\.?d|doctorate|doctoral)\b").unwrap());
    static MASTER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(master'?s?|m\.?s\.?c?\.?|m\.?eng|mba)\b").unwrap()
    });
    static BACHELOR: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(bachelor'?s?|b\.?s\.?c?\.?|b\.?a\.?|b\.?eng|undergraduate degree|4[- ]year degree)\b").unwrap()
    });
    static ASSOCIATE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(associate'?s? degree|a\.?a\.?s?\.?)\b").unwrap());
    static HS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(high school|ged|secondary school)\b").unwrap()
    });
    static NONE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(no degree required|degree not required|equivalent experience in lieu of|or equivalent practical experience)\b").unwrap()
    });

    // "Bachelor's degree or equivalent practical experience" is not a degree requirement.
    if NONE.is_match(text) {
        return Some(E::None_);
    }
    if PHD.is_match(text) {
        Some(E::Doctorate)
    } else if MASTER.is_match(text) {
        Some(E::Master)
    } else if BACHELOR.is_match(text) {
        Some(E::Bachelor)
    } else if ASSOCIATE.is_match(text) {
        Some(E::Associate)
    } else if HS.is_match(text) {
        Some(E::Hs)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seniority_is_read_from_the_title() {
        assert_eq!(infer_seniority("Senior Software Engineer"), Some(Seniority::Senior));
        assert_eq!(infer_seniority("Sr. Backend Engineer"), Some(Seniority::Senior));
        assert_eq!(infer_seniority("Staff Platform Engineer"), Some(Seniority::Staff));
        assert_eq!(infer_seniority("Principal Engineer"), Some(Seniority::Principal));
        assert_eq!(infer_seniority("Junior Developer"), Some(Seniority::Junior));
        assert_eq!(infer_seniority("Software Engineer II"), Some(Seniority::Mid));
    }

    #[test]
    fn management_titles_outrank_the_seniority_adjective_in_front_of_them() {
        assert_eq!(
            infer_seniority("Senior Engineering Manager"),
            Some(Seniority::Manager),
            "this is a management role, not a senior IC role"
        );
        assert_eq!(infer_seniority("Director of Engineering"), Some(Seniority::Director));
        assert_eq!(infer_seniority("VP of Platform"), Some(Seniority::Vp));
        assert_eq!(infer_seniority("Chief Technology Officer"), Some(Seniority::Exec));
    }

    #[test]
    fn an_internship_stays_an_internship_however_it_is_dressed_up() {
        assert_eq!(
            infer_seniority("Senior Software Engineering Intern"),
            Some(Seniority::Intern)
        );
        assert_eq!(infer_seniority("Engineering Co-op"), Some(Seniority::Intern));
    }

    #[test]
    fn an_unmarked_title_yields_none_rather_than_mid() {
        assert_eq!(
            infer_seniority("Software Engineer"),
            None,
            "guessing a level from a bare title would be a fabricated field"
        );
    }

    #[test]
    fn hybrid_is_never_misread_as_fully_remote() {
        assert_eq!(infer_work_mode("Hybrid remote - Austin, TX"), Some(WorkMode::Hybrid));
        assert_eq!(
            infer_work_mode("Remote, with 2 days per week in the office"),
            Some(WorkMode::Hybrid)
        );
        assert_eq!(infer_work_mode("100% remote"), Some(WorkMode::Remote));
        assert_eq!(infer_work_mode("This role is on-site in Seattle"), Some(WorkMode::Onsite));
        assert_eq!(infer_work_mode("Engineering role"), None);
    }

    #[test]
    fn employment_type_distinguishes_contract_from_contract_to_hire() {
        assert_eq!(
            infer_employment_type("6 month contract-to-hire"),
            Some(EmploymentType::ContractToHire)
        );
        assert_eq!(infer_employment_type("W2 contract, 12 months"), Some(EmploymentType::Contract));
        assert_eq!(infer_employment_type("Full-time, permanent"), Some(EmploymentType::FullTime));
        assert_eq!(infer_employment_type("Summer internship"), Some(EmploymentType::Internship));
        assert_eq!(infer_employment_type("Part-time, 20 hrs/week"), Some(EmploymentType::PartTime));
    }

    #[test]
    fn clearances_are_normalized_to_comparable_keys() {
        assert_eq!(detect_clearance("Must hold an active TS/SCI").as_deref(), Some("ts_sci"));
        assert_eq!(
            detect_clearance("TS/SCI with CI polygraph required").as_deref(),
            Some("ts_sci"),
        );
        assert_eq!(detect_clearance("Active Secret clearance").as_deref(), Some("secret"));
        assert_eq!(detect_clearance("Public Trust required").as_deref(), Some("public_trust"));
    }

    #[test]
    fn a_statement_that_no_clearance_is_needed_does_not_create_a_blocker() {
        assert_eq!(detect_clearance("No clearance required for this role"), None);
        assert_eq!(detect_clearance("Security clearance not required"), None);
        assert_eq!(detect_clearance("Build great software"), None);
    }

    #[test]
    fn visa_stance_distinguishes_no_from_unstated() {
        assert_eq!(
            detect_visa_sponsorship("We are unable to sponsor visas at this time"),
            Tristate::No
        );
        assert_eq!(
            detect_visa_sponsorship("Visa sponsorship available for exceptional candidates"),
            Tristate::Yes
        );
        assert_eq!(
            detect_visa_sponsorship("Great benefits and a strong team"),
            Tristate::Unspecified,
            "silence is not a refusal"
        );
    }

    #[test]
    fn years_of_experience_are_extracted_as_ranges_or_floors() {
        assert_eq!(extract_years("5+ years of experience"), Some((5.0, None)));
        assert_eq!(extract_years("5-7 years in distributed systems"), Some((5.0, Some(7.0))));
        assert_eq!(extract_years("At least 3 years"), Some((3.0, None)));
        assert_eq!(extract_years("three years of Rust"), Some((3.0, None)));
        assert_eq!(extract_years("Strong communication skills"), None);
    }

    #[test]
    fn education_is_detected_at_the_highest_stated_level() {
        use jobseeker_core::domain::enums::EducationLevel as E;
        assert_eq!(detect_education("PhD in Computer Science"), Some(E::Doctorate));
        assert_eq!(detect_education("Master's degree preferred"), Some(E::Master));
        assert_eq!(detect_education("BS in Engineering"), Some(E::Bachelor));
        assert_eq!(detect_education("Build great software"), None);
    }

    #[test]
    fn an_equivalent_experience_escape_hatch_is_not_a_degree_requirement() {
        use jobseeker_core::domain::enums::EducationLevel as E;
        assert_eq!(
            detect_education("Bachelor's degree or equivalent practical experience"),
            Some(E::None_),
            "the escape hatch is the actual requirement"
        );
    }
}
