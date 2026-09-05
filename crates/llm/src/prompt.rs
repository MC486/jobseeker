//! Prompt construction.
//!
//! The prompts live here, not next to the pipeline stages, so a wording change is a
//! one-file review and so every caller asks the model the same question. The JSON Schema
//! travels with the prompt: a provider that supports constrained decoding uses it as a
//! decoder constraint, and the rest get it appended and validated on return.

use jobseeker_core::domain::job::ExtractedJob;
use serde_json::json;

use crate::{Purpose, Request};

/// JSON Schema for the field-filling stage. Every property is optional: the model is asked
/// only about the gaps, and inventing a field the posting did not state is worse than
/// leaving it empty.
pub fn extract_fields_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "title": { "type": ["string", "null"] },
            "company_name": { "type": ["string", "null"] },
            "work_mode": { "type": ["string", "null"], "enum": ["remote", "hybrid", "onsite", "unknown", null] },
            "employment_type": {
                "type": ["string", "null"],
                "enum": ["full_time", "part_time", "contract", "contract_to_hire", "internship", "temporary", "volunteer", "unknown", null]
            },
            "seniority": {
                "type": ["string", "null"],
                "enum": ["intern", "entry", "junior", "mid", "senior", "staff", "principal", "lead", "manager", "director", "vp", "exec", "unknown", null]
            },
            "locations": { "type": "array", "items": { "type": "string" } },
            "salary_text": { "type": ["string", "null"] },
            "posted_at": { "type": ["string", "null"] },
            "closes_at": { "type": ["string", "null"] },
            "apply_url": { "type": ["string", "null"] },
            "requires_clearance": { "type": ["string", "null"] },
            "visa_sponsorship": { "type": ["string", "null"], "enum": ["yes", "no", "unspecified", null] },
            "education_min": { "type": ["string", "null"], "enum": ["none", "hs", "associate", "bachelor", "master", "doctorate", "unknown", null] },
            "years_experience_min": { "type": ["number", "null"] },
            "department": { "type": ["string", "null"] },
            "confidence": { "type": "number", "minimum": 0, "maximum": 1 }
        }
    })
}

pub fn atomize_requirements_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["requirements"],
        "properties": {
            "requirements": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["text", "kind", "necessity"],
                    "properties": {
                        "text": { "type": "string" },
                        "kind": {
                            "type": "string",
                            "enum": ["skill", "tool", "experience", "education", "certification", "clearance", "language", "soft_skill", "domain", "responsibility", "logistics", "other"]
                        },
                        "necessity": { "type": "string", "enum": ["required", "preferred", "nice_to_have", "implied"] },
                        "skill": { "type": ["string", "null"] },
                        "min_years": { "type": ["number", "null"] },
                        "is_blocker": { "type": "boolean" },
                        "confidence": { "type": "number", "minimum": 0, "maximum": 1 }
                    }
                }
            }
        }
    })
}

const EXTRACT_SYSTEM: &str = "\
You extract structured fields from a job posting. You never invent a value the posting did \
not state. If a field is not stated, return null. Salary must be the employer's own figure \
when both an estimate and a posted band appear. Return JSON only.";

const ATOMIZE_SYSTEM: &str = "\
You break a job description into individually addressable requirements. Split compound \
bullets (\"Python and Kubernetes\" is two requirements). Necessity comes from the heading \
when one is present. Do not invent years, skills or blockers the text does not support. \
Return JSON only.";

const PHRASE_SYSTEM: &str = "\
You rephrase an existing accomplishment for a target job. You may change wording, length \
and emphasis. You must not add facts, metrics, tools, titles or employers that are not in \
the original. If you cannot improve the bullet without inventing, return it unchanged \
with changed=false.";

/// Ask the model only about the fields that are still empty, so a mostly-extracted posting
/// costs almost no tokens (`docs/06-extraction.md` §2).
pub fn extract_fields(job: &ExtractedJob, description: &str, max_chars: usize) -> Request {
    let missing = job.missing_fields();
    let body = truncate(description, max_chars);
    let asked = if missing.is_empty() {
        "Confirm any remaining gaps; every field may stay null.".to_string()
    } else {
        format!(
            "Fill only these missing fields, leave the rest null: {}.",
            missing.join(", ")
        )
    };
    Request::new(
        Purpose::ExtractFields,
        EXTRACT_SYSTEM,
        format!("{asked}\n\n---\n{body}"),
    )
    .with_schema(extract_fields_schema())
}

pub fn atomize_requirements(description: &str, max_chars: usize) -> Request {
    Request::new(
        Purpose::AtomizeRequirements,
        ATOMIZE_SYSTEM,
        format!(
            "Atomize the requirements in this posting:\n\n{}",
            truncate(description, max_chars)
        ),
    )
    .with_schema(atomize_requirements_schema())
}

pub fn phrase_bullet(
    original: &str,
    job_title: &str,
    company: &str,
    target_skills: &[String],
) -> Request {
    Request::new(
        Purpose::PhraseBullet,
        PHRASE_SYSTEM,
        format!(
            "Job: {job_title} at {company}\nRelevant skills: {}\n\n{original}",
            target_skills.join(", ")
        ),
    )
    .with_schema(json!({
        "type": "object",
        "required": ["text", "changed"],
        "properties": {
            "text": { "type": "string" },
            "changed": { "type": "boolean" }
        }
    }))
    .with_temperature(0.4)
}

/// Truncate on a paragraph boundary so a long posting is not cut mid-requirement. The last
/// incomplete paragraph is dropped rather than half-kept, because a half-kept bullet is
/// worse than an omitted one.
pub fn truncate(input: &str, max_chars: usize) -> String {
    if input.len() <= max_chars {
        return input.to_string();
    }
    let end = input.floor_char_boundary(max_chars);
    let slice = &input[..end];
    match slice.rfind("\n\n") {
        Some(i) if i > max_chars / 2 => format!("{}\n\n[truncated]", &slice[..i]),
        _ => {
            let word = slice.rfind(char::is_whitespace).unwrap_or(slice.len());
            format!("{}…", slice[..word].trim_end())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobseeker_core::domain::job::ExtractedJob;
    use jobseeker_core::provenance::{Provenance, Sourced};

    #[test]
    fn extract_asks_only_about_the_gaps() {
        let mut job = ExtractedJob::default();
        job.title = Some(Sourced::new("Engineer".into(), Provenance::Jsonld));
        let request = extract_fields(&job, "a long posting about the role", 16_000);
        assert!(
            request.user.contains("company_name"),
            "still missing: {}",
            request.user
        );
        assert!(
            !request
                .user
                .contains("Fill only these missing fields, leave the rest null: title,"),
            "a filled field must not be re-asked: {}",
            request.user
        );
        assert_eq!(request.purpose, Purpose::ExtractFields);
        assert!(request.schema.is_some());
        assert_eq!(request.temperature, 0.0);
    }

    #[test]
    fn a_fully_extracted_job_still_produces_a_valid_request() {
        // The pipeline may still consult the model for a confidence check; the prompt must
        // not panic or ask it to invent fields.
        let job = ExtractedJob::default();
        let request = extract_fields(&job, "text", 100);
        assert!(request.user.contains("title"));
        assert!(request.schema.is_some());
    }

    #[test]
    fn truncation_drops_an_incomplete_paragraph_rather_than_a_half_bullet() {
        let text = "First paragraph is long enough to keep.\n\nSecond paragraph is much longer and would be cut mid-sentence if we sliced blindly.\n\nThird.";
        let out = truncate(text, 70);
        assert!(
            out.contains("First paragraph is long enough to keep"),
            "got {out}"
        );
        assert!(
            !out.contains("Second paragraph is much"),
            "the incomplete paragraph must be dropped: {out}"
        );
        assert!(out.contains("[truncated]"));
    }

    #[test]
    fn short_input_is_left_alone() {
        assert_eq!(truncate("hello", 100), "hello");
    }

    #[test]
    fn phrasing_names_the_target_without_restating_the_original_as_instruction() {
        let request = phrase_bullet(
            "Cut p95 latency by 85%",
            "Senior Platform Engineer",
            "Acme",
            &["rust".into(), "kubernetes".into()],
        );
        assert!(request.user.contains("Cut p95 latency by 85%"));
        assert!(request.user.contains("Acme"));
        assert!(request.system.contains("must not add facts"));
        assert!(
            request.temperature > 0.0,
            "phrasing is the one creative task, and a zero temperature flattens it"
        );
    }

    #[test]
    fn schemas_are_objects_with_closed_additional_properties() {
        let extract = extract_fields_schema();
        assert_eq!(extract["additionalProperties"], false);
        let atomize = atomize_requirements_schema();
        assert_eq!(atomize["required"][0], "requirements");
    }
}
