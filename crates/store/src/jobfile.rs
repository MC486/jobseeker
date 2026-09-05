//! `job.md` and `job.json` generation.
//!
//! `job.md` is the file you would actually read. Frontmatter is the queryable projection;
//! the body is readable prose. Both are generated from structured records so a re-export
//! of unchanged data is byte-identical.

use jobseeker_core::domain::job::ExtractedJob;
use jobseeker_normalize::requirement::AtomizedRequirement;
use jobseeker_normalize::text::tidy_markdown;
use serde::Serialize;

use crate::to_stable_json;
use jobseeker_core::Result;

#[derive(Debug, Clone, Serialize)]
pub struct JobDocument {
    pub title: String,
    pub company: String,
    pub status: String,
    pub work_mode: Option<String>,
    pub employment_type: Option<String>,
    pub seniority: Option<String>,
    pub salary: Option<String>,
    pub apply_url: Option<String>,
    pub posted_at: Option<String>,
    pub closes_at: Option<String>,
    pub content_hash: String,
    pub extraction_partial: bool,
}

/// Render `job.md`: YAML-ish frontmatter plus Markdown body. Kept simple (not a YAML
/// library) so the output is stable and the crate stays small.
pub fn render_markdown(doc: &JobDocument, body: &str, requirements: &[AtomizedRequirement]) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("title: {}\n", yaml_escape(&doc.title)));
    out.push_str(&format!("company: {}\n", yaml_escape(&doc.company)));
    out.push_str(&format!("status: {}\n", doc.status));
    if let Some(v) = &doc.work_mode {
        out.push_str(&format!("work_mode: {v}\n"));
    }
    if let Some(v) = &doc.employment_type {
        out.push_str(&format!("employment_type: {v}\n"));
    }
    if let Some(v) = &doc.seniority {
        out.push_str(&format!("seniority: {v}\n"));
    }
    if let Some(v) = &doc.salary {
        out.push_str(&format!("salary: {}\n", yaml_escape(v)));
    }
    if let Some(v) = &doc.apply_url {
        out.push_str(&format!("apply_url: {v}\n"));
    }
    if let Some(v) = &doc.posted_at {
        out.push_str(&format!("posted_at: {v}\n"));
    }
    if let Some(v) = &doc.closes_at {
        out.push_str(&format!("closes_at: {v}\n"));
    }
    out.push_str(&format!("extraction_partial: {}\n", doc.extraction_partial));
    out.push_str(&format!("content_hash: {}\n", doc.content_hash));
    out.push_str("---\n\n");
    out.push_str(&format!("# {} — {}\n\n", doc.title, doc.company));

    let required: Vec<_> = requirements.iter().filter(|r| r.necessity.as_str() == "required" || r.necessity.as_str() == "implied").collect();
    let preferred: Vec<_> = requirements.iter().filter(|r| r.necessity.as_str() != "required" && r.necessity.as_str() != "implied").collect();
    if !required.is_empty() {
        out.push_str("## Required\n\n");
        for r in required {
            out.push_str(&format!("- {}  *({}: {})*\n", r.text, r.kind, r.normalized_text));
        }
        out.push('\n');
    }
    if !preferred.is_empty() {
        out.push_str("## Preferred\n\n");
        for r in preferred {
            out.push_str(&format!("- {}  *({}: {})*\n", r.text, r.kind, r.normalized_text));
        }
        out.push('\n');
    }
    if !body.trim().is_empty() {
        out.push_str("## Description\n\n");
        out.push_str(&tidy_markdown(body));
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

pub fn render_job_json(job: &ExtractedJob) -> Result<Vec<u8>> {
    to_stable_json(job)
}

fn yaml_escape(s: &str) -> String {
    if s.chars().any(|c| matches!(c, ':' | '#' | '{' | '}' | '[' | ']' | ',' | '"' | '\'')) {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobseeker_core::domain::enums::{Necessity, RequirementKind};

    fn atom(text: &str, necessity: Necessity) -> AtomizedRequirement {
        AtomizedRequirement {
            text: text.into(),
            normalized_text: text.to_lowercase(),
            kind: RequirementKind::Skill,
            necessity,
            min_years: None,
            max_years: None,
            education_level: None,
            is_blocker: false,
            quantity_raw: None,
            source_span: None,
        }
    }

    #[test]
    fn markdown_round_trips_the_fields_a_human_would_grep_for() {
        let md = render_markdown(
            &JobDocument {
                title: "Senior Platform Engineer".into(),
                company: "Acme Robotics".into(),
                status: "open".into(),
                work_mode: Some("remote".into()),
                employment_type: Some("full_time".into()),
                seniority: Some("senior".into()),
                salary: Some("$185,000–$225,000/yr".into()),
                apply_url: Some("https://boards.greenhouse.io/acme/jobs/1".into()),
                posted_at: Some("2026-09-02".into()),
                closes_at: None,
                content_hash: "b3:abc".into(),
                extraction_partial: false,
            },
            "Own the ingestion platform.",
            &[
                atom("Production Rust", Necessity::Required),
                atom("Robotics experience", Necessity::NiceToHave),
            ],
        );
        assert!(md.starts_with("---\n"));
        assert!(md.contains("title: Senior Platform Engineer"));
        assert!(md.contains("## Required"));
        assert!(md.contains("Production Rust"));
        assert!(md.contains("## Preferred"));
        assert!(md.contains("Robotics experience"));
        assert!(md.contains("Own the ingestion platform."));
    }

    #[test]
    fn values_that_would_break_yaml_are_quoted() {
        let md = render_markdown(
            &JobDocument {
                title: "Engineer: Platform".into(),
                company: "Acme".into(),
                status: "open".into(),
                work_mode: None,
                employment_type: None,
                seniority: None,
                salary: None,
                apply_url: None,
                posted_at: None,
                closes_at: None,
                content_hash: "b3:x".into(),
                extraction_partial: true,
            },
            "",
            &[],
        );
        assert!(md.contains("title: \"Engineer: Platform\""), "got {md}");
    }
}
