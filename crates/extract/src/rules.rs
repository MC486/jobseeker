//! Deterministic heuristics (`Provenance::Rules`).
//!
//! Cheap, precise, and the last stage before the model. They exist so a posting without
//! JSON-LD still becomes a structured record, and so the model is only asked about the
//! fields these rules could not fill.

use jobseeker_core::domain::enums::SourceKind;
use jobseeker_core::domain::job::ExtractedJob;
use jobseeker_core::domain::location::RawLocation;
use jobseeker_core::domain::salary::RawSalary;
use jobseeker_core::provenance::{merge_field, Provenance, Sourced};
use jobseeker_core::time::now;
use jobseeker_normalize::date::parse_posted_date;
use jobseeker_normalize::seniority::{
    detect_clearance, detect_education, detect_visa_sponsorship, extract_years,
    infer_employment_type, infer_seniority, infer_work_mode,
};
use once_cell::sync::Lazy;
use regex::Regex;
use scraper::Html;

use crate::html;

pub fn apply_url_hints(job: &mut ExtractedJob, url: &str, source: SourceKind) {
    if job.apply_url.is_none() {
        merge_field(
            &mut job.apply_url,
            Sourced::new(url.to_string(), Provenance::Rules),
        );
    }
    if source.requires_authentication() {
        // Aggregator apply URLs are not the employer's ATS; leave apply_kind unset so a
        // later listing with a real ATS URL can win.
        return;
    }
    if job.apply_kind.is_none() {
        use jobseeker_core::domain::enums::ApplyKind;
        merge_field(
            &mut job.apply_kind,
            Sourced::new(ApplyKind::Ats, Provenance::Inferred),
        );
    }
}

pub fn apply_html(job: &mut ExtractedJob, document: &Html, source: SourceKind) {
    if job.title.is_none() {
        if let Some(title) = html::first_heading(document) {
            merge_field(&mut job.title, Sourced::new(title, Provenance::Rules));
        }
    }

    let text = html::main_text(document);
    if job.description_html.is_none() && !text.is_empty() {
        merge_field(
            &mut job.description_md,
            Sourced::new(
                jobseeker_normalize::text::tidy_markdown(&text),
                Provenance::Rules,
            ),
        );
    }

    if job.work_mode.is_none() {
        if let Some(mode) = infer_work_mode(&text) {
            merge_field(&mut job.work_mode, Sourced::new(mode, Provenance::Rules));
        }
    }
    if job.employment_type.is_none() {
        if let Some(kind) = infer_employment_type(&text) {
            merge_field(
                &mut job.employment_type,
                Sourced::new(kind, Provenance::Rules),
            );
        }
    }
    if job.seniority.is_none() {
        if let Some(title) = job.title.as_ref() {
            if let Some(level) = infer_seniority(&title.value) {
                merge_field(
                    &mut job.seniority,
                    Sourced::new(level, Provenance::Inferred),
                );
            }
        }
    }

    if job.salary.is_none() {
        if let Some(raw) = find_salary(&text) {
            merge_field(
                &mut job.salary,
                Sourced::new(
                    RawSalary {
                        text: raw,
                        country_hint: None,
                        source_kind: Some(source),
                    },
                    Provenance::Rules,
                ),
            );
        }
    }

    if job.locations.is_empty() {
        if let Some(loc) = find_location(&text) {
            job.locations
                .push(Sourced::new(RawLocation::new(loc), Provenance::Rules));
        }
    }

    if job.posted_at.is_none() {
        if let Some(raw) = find_date(&text) {
            if let Some(d) = parse_posted_date(&raw, now()) {
                merge_field(&mut job.posted_at, Sourced::new(d, Provenance::Rules));
            }
        }
    }

    if job.requires_clearance.is_none() {
        if let Some(c) = detect_clearance(&text) {
            merge_field(
                &mut job.requires_clearance,
                Sourced::new(c, Provenance::Rules),
            );
        }
    }
    if job.visa_sponsorship.is_none() {
        let stance = detect_visa_sponsorship(&text);
        if stance != jobseeker_core::domain::enums::Tristate::Unspecified {
            merge_field(
                &mut job.visa_sponsorship,
                Sourced::new(stance, Provenance::Rules),
            );
        }
    }
    if job.education_min.is_none() {
        if let Some(e) = detect_education(&text) {
            merge_field(&mut job.education_min, Sourced::new(e, Provenance::Rules));
        }
    }
    if job.years_experience_min.is_none() {
        if let Some((min, _)) = extract_years(&text) {
            merge_field(
                &mut job.years_experience_min,
                Sourced::new(min, Provenance::Rules),
            );
        }
    }

    if job.company_name.is_none() {
        if let Some(name) = find_company(&text) {
            merge_field(&mut job.company_name, Sourced::new(name, Provenance::Rules));
        }
    }
}

fn find_salary(text: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?i)(?:salary|compensation|pay|base)[:\s]+(.{0,80}\d[\d,.]{2,}.{0,40}(?:year|yr|hour|hr|annum|month))",
        )
        .unwrap()
    });
    RE.captures(text)
        .map(|c| c[1].trim().to_string())
        .or_else(|| {
            static BARE: Lazy<Regex> = Lazy::new(|| {
                Regex::new(r"(?i)\$\s*\d[\d,]*(?:\s*[-–]\s*\$?\s*\d[\d,]*)?(?:\s*k)?(?:\s*(?:per|/)\s*(?:year|yr|hour|hr))?")
                    .unwrap()
            });
            BARE.find(text).map(|m| m.as_str().to_string())
        })
}

fn find_location(text: &str) -> Option<String> {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(?:location|based in|office)[:\s]+([^\n]{3,60})").unwrap());
    RE.captures(text)
        .map(|c| c[1].trim().trim_end_matches('.').to_string())
}

fn find_date(text: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)(?:posted|published|date posted)[:\s]+([^\n]{3,40})").unwrap()
    });
    RE.captures(text).map(|c| c[1].trim().to_string())
}

fn find_company(text: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?i)\b(?:at|join|about)\s+([A-Z][\w&. ]{1,40}?)\s+(?:is hiring|is looking|we are)",
        )
        .unwrap()
    });
    RE.captures(text).map(|c| c[1].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_salary_line_is_lifted_out_of_prose() {
        assert!(find_salary("Salary: $185,000 - $225,000 per year plus equity").is_some());
        assert!(find_salary("Compensation $60/hr").is_some());
        assert_eq!(find_salary("Great benefits and a strong team"), None);
    }

    #[test]
    fn a_location_line_is_lifted_out_of_prose() {
        assert_eq!(
            find_location("Location: Remote - United States\nSalary: $1").as_deref(),
            Some("Remote - United States")
        );
    }
}
