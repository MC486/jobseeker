//! Site-specific extractors (`Provenance::Adapter`).
//!
//! Each adapter matches a host and fills only empty fields from known markup. Selectors
//! are lists of alternatives: class names churn, so a miss degrades to the generic
//! heuristics instead of failing the job (`docs/06-extraction.md` §2).

mod ashby;
mod greenhouse;
mod indeed;
mod lever;
mod linkedin;
mod workday;

use jobseeker_core::domain::enums::SourceKind;
use jobseeker_core::domain::job::ExtractedJob;
use jobseeker_core::domain::location::RawLocation;
use jobseeker_core::domain::salary::RawSalary;
use jobseeker_core::provenance::{merge_field, Provenance, Sourced};
use jobseeker_normalize::seniority::infer_work_mode;
use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;

/// What an adapter is allowed to see of a capture.
pub struct CaptureView<'a> {
    pub url: Option<&'a str>,
    pub html: &'a str,
    pub document: &'a Html,
    pub page_meta: Option<&'a Value>,
}

pub trait SiteAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn matches(&self, url: &str) -> bool;
    fn extract(&self, job: &mut ExtractedJob, cap: &CaptureView<'_>);
}

const REGISTRY: &[&dyn SiteAdapter] = &[
    &linkedin::LinkedIn,
    &indeed::Indeed,
    &greenhouse::Greenhouse,
    &lever::Lever,
    &ashby::Ashby,
    &workday::Workday,
];

/// Run the first matching adapter. No match is a no-op.
pub fn apply(job: &mut ExtractedJob, cap: &CaptureView<'_>) {
    let Some(url) = cap.url else {
        return;
    };
    for adapter in REGISTRY {
        if adapter.matches(url) {
            adapter.extract(job, cap);
            return;
        }
    }
}

fn host_matches(url: &str, suffixes: &[&str]) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    suffixes
        .iter()
        .any(|s| host == *s || host.ends_with(&format!(".{s}")))
}

fn all_texts(document: &Html, selectors: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for sel in selectors {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        for node in document.select(&selector) {
            let text = node
                .text()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !text.is_empty()
                && !out
                    .iter()
                    .any(|existing: &String| existing.eq_ignore_ascii_case(&text))
            {
                out.push(text);
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

fn first_text(document: &Html, selectors: &[&str]) -> Option<String> {
    for sel in selectors {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        if let Some(node) = document.select(&selector).next() {
            let text = node
                .text()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn first_attr(document: &Html, selectors: &[&str], attr: &str) -> Option<String> {
    for sel in selectors {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        if let Some(node) = document.select(&selector).next() {
            if let Some(v) = node.value().attr(attr) {
                if !v.trim().is_empty() {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

fn first_inner_html(document: &Html, selectors: &[&str]) -> Option<String> {
    for sel in selectors {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        if let Some(node) = document.select(&selector).next() {
            let html = node.inner_html();
            if html.trim().len() >= 20 {
                return Some(html);
            }
        }
    }
    None
}

fn set_title(job: &mut ExtractedJob, title: impl Into<String>) {
    let title = title.into();
    if title.is_empty() {
        return;
    }
    merge_field(&mut job.title, Sourced::new(title, Provenance::Adapter));
}

fn set_company(job: &mut ExtractedJob, company: impl Into<String>) {
    let company = company.into();
    if company.is_empty() {
        return;
    }
    merge_field(
        &mut job.company_name,
        Sourced::new(company, Provenance::Adapter),
    );
}

fn set_description_html(job: &mut ExtractedJob, html: &str) {
    if html.trim().is_empty() {
        return;
    }
    merge_field(
        &mut job.description_html,
        Sourced::new(html.to_string(), Provenance::Adapter),
    );
    merge_field(
        &mut job.description_md,
        Sourced::new(crate::html::to_markdown(html), Provenance::Adapter),
    );
}

fn set_location(job: &mut ExtractedJob, text: impl Into<String>) {
    add_location(job, text);
}

fn add_location(job: &mut ExtractedJob, text: impl Into<String>) {
    let text = text.into();
    if text.is_empty() {
        return;
    }
    let mode = infer_work_mode(&text);
    if !job
        .locations
        .iter()
        .any(|existing| existing.value.text.eq_ignore_ascii_case(&text))
    {
        job.locations.push(Sourced::new(
            RawLocation {
                text: text.clone(),
                is_remote_hint: mode == Some(jobseeker_core::domain::enums::WorkMode::Remote),
                country_hint: None,
            },
            Provenance::Adapter,
        ));
    }
    if let Some(mode) = mode {
        merge_field(&mut job.work_mode, Sourced::new(mode, Provenance::Adapter));
    }
}

fn set_salary(job: &mut ExtractedJob, text: impl Into<String>, source: SourceKind) {
    let text = text.into();
    if text.is_empty() {
        return;
    }
    merge_field(
        &mut job.salary,
        Sourced::new(
            RawSalary {
                text,
                country_hint: None,
                source_kind: Some(source),
            },
            Provenance::Adapter,
        ),
    );
}

fn set_source_job_id(job: &mut ExtractedJob, id: impl Into<String>) {
    let id = id.into();
    if id.is_empty() {
        return;
    }
    merge_field(
        &mut job.source_job_id,
        Sourced::new(id, Provenance::Adapter),
    );
}

/// Parse the first JSON object assigned to `name` in a script (`window.foo = {…}`).
fn js_assignment_object(html: &str, name: &str) -> Option<Value> {
    let idx = html.find(name)?;
    let after_name = &html[idx + name.len()..];
    let eq = after_name.find('=')?;
    let after_eq = after_name[eq + 1..].trim_start();
    let start = after_eq.find('{')?;
    let object = first_json_object(&after_eq[start..])?;
    serde_json::from_str(&sanitize_js_object(&object)).ok()
}

fn sanitize_js_object(src: &str) -> String {
    static UNDEF: Lazy<Regex> = Lazy::new(|| Regex::new(r":\s*undefined\b").unwrap());
    static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r",(\s*[}\]])").unwrap());
    let without_undef = UNDEF.replace_all(src, ":null");
    TRAIL.replace_all(&without_undef, "$1").into_owned()
}

fn first_json_object(src: &str) -> Option<String> {
    let bytes = src.as_bytes();
    if bytes.first() != Some(&b'{') {
        return None;
    }
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(src[..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn json_text(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(v) = value.get(*key) {
            if let Some(s) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                return Some(s.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html;

    #[test]
    fn unknown_hosts_are_ignored() {
        let doc = html::parse("<html><body><h1>X</h1></body></html>");
        let mut job = ExtractedJob::default();
        apply(
            &mut job,
            &CaptureView {
                url: Some("https://example.com/jobs/1"),
                html: "",
                document: &doc,
                page_meta: None,
            },
        );
        assert!(job.title.is_none());
    }

    #[test]
    fn js_assignment_survives_undefined() {
        let html = r#"window._initialData = {"jobTitle":"SRE","skip": undefined,"ok":true,};"#;
        let v = js_assignment_object(html, "_initialData").unwrap();
        assert_eq!(v["jobTitle"], "SRE");
        assert!(v["skip"].is_null());
    }
}
