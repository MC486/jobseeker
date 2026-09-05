//! schema.org `JobPosting` JSON-LD.
//!
//! The single best free signal on ATS pages and many company sites. Every field that can
//! be read from it is tagged `Provenance::Jsonld` (or `Api` when the body *is* the JSON
//! API response). Edge cases that occur in the wild are handled here so they do not fall
//! through to the model (`docs/06-extraction.md` §2).

use jobseeker_core::domain::enums::{EmploymentType, WorkMode};
use jobseeker_core::domain::job::ExtractedJob;
use jobseeker_core::domain::location::RawLocation;
use jobseeker_core::domain::salary::RawSalary;
use jobseeker_core::provenance::{merge_field, Provenance, Sourced};
use jobseeker_core::time::now;
use jobseeker_normalize::date::parse_posted_date;
use scraper::{Html, Selector};
use serde_json::Value;

pub fn apply_from_html(job: &mut ExtractedJob, document: &Html, page_meta: Option<&Value>) {
    if let Some(meta) = page_meta {
        apply_json_value(job, meta, Provenance::Jsonld);
    }
    let Ok(selector) = Selector::parse("script[type='application/ld+json']") else {
        return;
    };
    for script in document.select(&selector) {
        let text = script.text().collect::<String>();
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            apply_json_value(job, &value, Provenance::Jsonld);
        }
    }
}

/// Walk a JSON value looking for `JobPosting` objects, including those nested under
/// `@graph` or wrapped in an array — both appear on real pages.
pub fn apply_json_value(job: &mut ExtractedJob, value: &Value, provenance: Provenance) {
    match value {
        Value::Array(items) => {
            for item in items {
                apply_json_value(job, item, provenance);
            }
        }
        Value::Object(map) => {
            if let Some(graph) = map.get("@graph") {
                apply_json_value(job, graph, provenance);
            }
            if is_job_posting(value) || looks_like_posting(value) {
                apply_posting(job, value, provenance);
            }
            // Greenhouse's board API is a posting without an `@type`.
            if map.get("title").is_some() && (map.get("content").is_some() || map.get("description").is_some()) {
                apply_posting(job, value, provenance);
            }
        }
        _ => {}
    }
}

fn is_job_posting(value: &Value) -> bool {
    match value.get("@type") {
        Some(Value::String(t)) => t.eq_ignore_ascii_case("JobPosting") || t.eq_ignore_ascii_case("Job"),
        Some(Value::Array(types)) => types.iter().any(|t| {
            t.as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("JobPosting") || s.eq_ignore_ascii_case("Job"))
        }),
        _ => false,
    }
}

fn looks_like_posting(value: &Value) -> bool {
    value.get("hiringOrganization").is_some() && value.get("title").is_some()
}

fn apply_posting(job: &mut ExtractedJob, posting: &Value, provenance: Provenance) {
    if let Some(title) = text_field(posting, &["title", "name"]) {
        merge_field(&mut job.title, Sourced::new(title, provenance));
    }
    if let Some(company) = organization_name(posting.get("hiringOrganization").or_else(|| posting.get("company")))
    {
        merge_field(&mut job.company_name, Sourced::new(company, provenance));
    }
    if let Some(html) = text_field(posting, &["description", "content"]) {
        merge_field(&mut job.description_html, Sourced::new(html.clone(), provenance));
        merge_field(
            &mut job.description_md,
            Sourced::new(crate::html::to_markdown(&html), provenance),
        );
    }
    if let Some(url) = text_field(posting, &["url", "absolute_url", "applyUrl"]) {
        merge_field(&mut job.apply_url, Sourced::new(url, provenance));
    }
    if let Some(id) = identifier(posting) {
        merge_field(&mut job.source_job_id, Sourced::new(id, provenance));
    }
    if let Some(dept) = text_field(posting, &["department", "industry"]) {
        merge_field(&mut job.department, Sourced::new(dept, provenance));
    }

    if let Some(mode) = work_mode_from(posting) {
        merge_field(&mut job.work_mode, Sourced::new(mode, provenance));
    }
    if let Some(kind) = employment_type_from(posting) {
        merge_field(&mut job.employment_type, Sourced::new(kind, provenance));
    }

    for loc in locations_from(posting) {
        job.locations.push(Sourced::new(loc, provenance));
    }

    if let Some(raw) = salary_from(posting) {
        merge_field(&mut job.salary, Sourced::new(raw, provenance));
    }

    let captured = now();
    if let Some(posted) = text_field(posting, &["datePosted", "published_at", "first_published"]) {
        if let Some(d) = parse_posted_date(&posted, captured) {
            merge_field(&mut job.posted_at, Sourced::new(d, provenance));
        }
    }
    if let Some(closes) = text_field(posting, &["validThrough", "closes_at"]) {
        if let Some(d) = parse_posted_date(&closes, captured) {
            merge_field(&mut job.closes_at, Sourced::new(d, provenance));
        }
    }
}

fn text_field(value: &Value, names: &[&str]) -> Option<String> {
    for name in names {
        if let Some(v) = value.get(*name) {
            if let Some(s) = as_text(v) {
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}

fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(html_unescape(s)),
        Value::Number(n) => Some(n.to_string()),
        Value::Object(map) => map
            .get("name")
            .or_else(|| map.get("value"))
            .or_else(|| map.get("@value"))
            .and_then(as_text),
        Value::Array(items) => items.iter().find_map(as_text),
        _ => None,
    }
}

fn organization_name(value: Option<&Value>) -> Option<String> {
    as_text(value?)
}

fn identifier(posting: &Value) -> Option<String> {
    match posting.get("identifier").or_else(|| posting.get("id"))? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Object(map) => map.get("value").and_then(as_text),
        _ => None,
    }
}

fn work_mode_from(posting: &Value) -> Option<WorkMode> {
    let loc_type = posting
        .get("jobLocationType")
        .and_then(as_text)
        .unwrap_or_default();
    if loc_type.to_ascii_uppercase().contains("TELECOMMUTE") {
        return Some(WorkMode::Remote);
    }
    jobseeker_normalize::infer_work_mode(&loc_type)
}

fn employment_type_from(posting: &Value) -> Option<EmploymentType> {
    let raw = posting.get("employmentType").and_then(as_text)?;
    Some(match raw.to_ascii_uppercase().replace('-', "_").as_str() {
        "FULL_TIME" | "FULLTIME" => EmploymentType::FullTime,
        "PART_TIME" | "PARTTIME" => EmploymentType::PartTime,
        "CONTRACTOR" | "CONTRACT" => EmploymentType::Contract,
        "INTERN" | "INTERNSHIP" => EmploymentType::Internship,
        "TEMPORARY" | "TEMP" => EmploymentType::Temporary,
        "VOLUNTEER" => EmploymentType::Volunteer,
        other => jobseeker_normalize::infer_employment_type(other)?,
    })
}

fn locations_from(posting: &Value) -> Vec<RawLocation> {
    let mut out = Vec::new();
    collect_location(posting.get("jobLocation"), &mut out, false);
    collect_location(posting.get("applicantLocationRequirements"), &mut out, true);
    if out.is_empty() {
        if let Some(text) = text_field(posting, &["location", "offices"]) {
            out.push(RawLocation::new(text));
        }
    }
    out
}

fn collect_location(value: Option<&Value>, out: &mut Vec<RawLocation>, remote_hint: bool) {
    let Some(value) = value else { return };
    match value {
        Value::Array(items) => {
            for item in items {
                collect_location(Some(item), out, remote_hint);
            }
        }
        Value::Object(map) => {
            let address = map.get("address").unwrap_or(value);
            let city = address.get("addressLocality").and_then(as_text);
            let region = address.get("addressRegion").and_then(as_text);
            let country = address
                .get("addressCountry")
                .and_then(as_text)
                .or_else(|| map.get("name").and_then(as_text));
            let parts: Vec<String> = [city, region, country.clone()]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect();
            if !parts.is_empty() {
                out.push(RawLocation {
                    text: parts.join(", "),
                    is_remote_hint: remote_hint,
                    country_hint: country.and_then(|c| {
                        jobseeker_normalize::location::normalize_country(&c)
                    }),
                });
            } else if let Some(name) = map.get("name").and_then(as_text) {
                out.push(RawLocation {
                    text: name,
                    is_remote_hint: remote_hint,
                    country_hint: None,
                });
            }
        }
        Value::String(s) => out.push(RawLocation {
            text: s.clone(),
            is_remote_hint: remote_hint,
            country_hint: None,
        }),
        _ => {}
    }
}

fn salary_from(posting: &Value) -> Option<RawSalary> {
    let salary = posting.get("baseSalary").or_else(|| posting.get("estimatedSalary"))?;
    let currency = salary.get("currency").and_then(as_text);
    let value = salary.get("value").unwrap_or(salary);
    let min = numberish(value.get("minValue")).or_else(|| numberish(value.get("value")));
    let max = numberish(value.get("maxValue")).or(min);
    let period = value.get("unitText").and_then(as_text).unwrap_or_default();
    let mut text = String::new();
    if let Some(c) = &currency {
        text.push_str(c);
        text.push(' ');
    }
    match (min, max) {
        (Some(a), Some(b)) if (a - b).abs() < f64::EPSILON => text.push_str(&format!("{a:.0}")),
        (Some(a), Some(b)) => text.push_str(&format!("{a:.0} - {b:.0}")),
        (Some(a), None) => text.push_str(&format!("{a:.0}")),
        _ => return as_text(salary).map(|t| RawSalary {
            text: t,
            country_hint: None,
            source_kind: None,
        }),
    }
    if !period.is_empty() {
        text.push(' ');
        text.push_str(&period);
    }
    Some(RawSalary {
        text,
        country_hint: None,
        source_kind: None,
    })
}

fn numberish(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.replace(',', "").parse().ok(),
        _ => None,
    }
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job_from(json: &str) -> ExtractedJob {
        let mut job = ExtractedJob::default();
        apply_json_value(&mut job, &serde_json::from_str(json).unwrap(), Provenance::Jsonld);
        job
    }

    #[test]
    fn a_canonical_job_posting_fills_the_structured_fields() {
        let job = job_from(
            r#"{
              "@type": "JobPosting",
              "title": "Senior Platform Engineer",
              "hiringOrganization": {"name": "Acme Robotics"},
              "datePosted": "2026-09-02",
              "employmentType": "FULL_TIME",
              "jobLocationType": "TELECOMMUTE",
              "applicantLocationRequirements": {"@type":"Country","name":"US"},
              "baseSalary": {"currency":"USD","value":{"minValue":185000,"maxValue":225000,"unitText":"YEAR"}},
              "description": "<p>Build the platform.</p>"
            }"#,
        );
        assert_eq!(job.title.unwrap().value, "Senior Platform Engineer");
        assert_eq!(job.company_name.unwrap().value, "Acme Robotics");
        assert_eq!(job.work_mode.unwrap().value, WorkMode::Remote);
        assert_eq!(job.employment_type.unwrap().value, EmploymentType::FullTime);
        assert_eq!(job.locations[0].value.text, "US");
        assert!(job.salary.unwrap().value.text.contains("185000"));
        assert!(job.description_md.unwrap().value.contains("Build the platform"));
    }

    #[test]
    fn a_graph_wrapper_and_an_array_are_unwrapped() {
        let job = job_from(
            r#"{"@graph":[{"@type":"Organization","name":"Nope"},{"@type":"JobPosting","title":"Hidden","hiringOrganization":"Acme","description":"x"}]}"#,
        );
        assert_eq!(job.title.unwrap().value, "Hidden");
        assert_eq!(job.company_name.unwrap().value, "Acme", "a bare string org is accepted");
    }

    #[test]
    fn hiring_organization_as_a_string_is_accepted() {
        let job = job_from(r#"{"@type":"JobPosting","title":"E","hiringOrganization":"Acme","description":"d"}"#);
        assert_eq!(job.company_name.unwrap().value, "Acme");
    }

    #[test]
    fn a_greenhouse_api_payload_without_a_type_is_still_read() {
        let job = job_from(
            r#"{"id":5512034,"title":"Platform Engineer","content":"<p>Hi</p>","absolute_url":"https://boards.greenhouse.io/acme/jobs/5512034"}"#,
        );
        assert_eq!(job.title.unwrap().value, "Platform Engineer");
        assert_eq!(job.source_job_id.unwrap().value, "5512034");
        assert_eq!(job.apply_url.unwrap().value, "https://boards.greenhouse.io/acme/jobs/5512034");
    }
}
