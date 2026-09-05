//! The model stage: fill only the fields deterministic stages left empty.
//!
//! A schema violation is worth one repair attempt. A provider outage leaves the record
//! standing with `extraction_partial = true` rather than failing the job
//! (`docs/06-extraction.md` §2).

use jobseeker_core::domain::job::ExtractedJob;
use jobseeker_core::domain::location::RawLocation;
use jobseeker_core::domain::salary::RawSalary;
use jobseeker_core::provenance::{merge_field, Provenance, Sourced};
use jobseeker_core::time::now;
use jobseeker_core::Result;
use jobseeker_llm::{prompt, LlmClient, Request};
use jobseeker_normalize::date::parse_posted_date;
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
struct LlmFields {
    title: Option<String>,
    company_name: Option<String>,
    work_mode: Option<String>,
    employment_type: Option<String>,
    seniority: Option<String>,
    locations: Option<Vec<String>>,
    salary_text: Option<String>,
    posted_at: Option<String>,
    closes_at: Option<String>,
    apply_url: Option<String>,
    requires_clearance: Option<String>,
    visa_sponsorship: Option<String>,
    education_min: Option<String>,
    years_experience_min: Option<f32>,
    department: Option<String>,
    confidence: Option<f32>,
}

pub async fn fill_gaps(
    job: &mut ExtractedJob,
    description: &str,
    client: &dyn LlmClient,
    max_chars: usize,
) -> Result<()> {
    if job.missing_fields().is_empty() {
        return Ok(());
    }
    let request = prompt::extract_fields(job, description, max_chars);
    let parsed = complete_validated(client, &request).await?;
    apply_fields(job, parsed);
    Ok(())
}

async fn complete_validated(client: &dyn LlmClient, request: &Request) -> Result<LlmFields> {
    match client.complete(request).await?.parse::<LlmFields>() {
        Ok(fields) => Ok(fields),
        Err(first) => {
            // One repair: show the model its own invalid output and the error. A second
            // failure is reported as a schema violation so the pipeline can mark partial
            // rather than retry forever.
            let repair = Request::new(
                request.purpose,
                request.system.clone(),
                format!(
                    "{}\n\nYour previous answer was not valid JSON ({first}). Reply with JSON only.",
                    request.user
                ),
            )
            .with_schema(request.schema.clone().unwrap_or(serde_json::json!({})));
            client.complete(&repair).await?.parse::<LlmFields>()
        }
    }
}

fn apply_fields(job: &mut ExtractedJob, fields: LlmFields) {
    let confidence = fields
        .confidence
        .unwrap_or(Provenance::Llm.baseline_confidence());
    let put = |slot: &mut Option<Sourced<String>>, value: Option<String>| {
        if let Some(v) = value.filter(|s| !s.is_empty()) {
            merge_field(
                slot,
                Sourced::with_confidence(v, Provenance::Llm, confidence),
            );
        }
    };
    put(&mut job.title, fields.title);
    put(&mut job.company_name, fields.company_name);
    put(&mut job.apply_url, fields.apply_url);
    put(&mut job.department, fields.department);
    put(&mut job.requires_clearance, fields.requires_clearance);

    if let Some(raw) = fields.work_mode.and_then(|s| s.parse().ok()) {
        merge_field(
            &mut job.work_mode,
            Sourced::with_confidence(raw, Provenance::Llm, confidence),
        );
    }
    if let Some(raw) = fields.employment_type.and_then(|s| s.parse().ok()) {
        merge_field(
            &mut job.employment_type,
            Sourced::with_confidence(raw, Provenance::Llm, confidence),
        );
    }
    if let Some(raw) = fields.seniority.and_then(|s| s.parse().ok()) {
        merge_field(
            &mut job.seniority,
            Sourced::with_confidence(raw, Provenance::Llm, confidence),
        );
    }
    if let Some(raw) = fields.visa_sponsorship.and_then(|s| s.parse().ok()) {
        merge_field(
            &mut job.visa_sponsorship,
            Sourced::with_confidence(raw, Provenance::Llm, confidence),
        );
    }
    if let Some(raw) = fields.education_min.and_then(|s| s.parse().ok()) {
        merge_field(
            &mut job.education_min,
            Sourced::with_confidence(raw, Provenance::Llm, confidence),
        );
    }
    if let Some(years) = fields.years_experience_min {
        merge_field(
            &mut job.years_experience_min,
            Sourced::with_confidence(years, Provenance::Llm, confidence),
        );
    }
    if let Some(text) = fields.salary_text.filter(|s| !s.is_empty()) {
        merge_field(
            &mut job.salary,
            Sourced::with_confidence(
                RawSalary {
                    text,
                    country_hint: None,
                    source_kind: None,
                },
                Provenance::Llm,
                confidence,
            ),
        );
    }
    if job.locations.is_empty() {
        for loc in fields.locations.unwrap_or_default() {
            if !loc.is_empty() {
                job.locations.push(Sourced::with_confidence(
                    RawLocation::new(loc),
                    Provenance::Llm,
                    confidence,
                ));
            }
        }
    }
    let captured = now();
    if let Some(raw) = fields.posted_at {
        if let Some(d) = parse_posted_date(&raw, captured) {
            merge_field(
                &mut job.posted_at,
                Sourced::with_confidence(d, Provenance::Llm, confidence),
            );
        }
    }
    if let Some(raw) = fields.closes_at {
        if let Some(d) = parse_posted_date(&raw, captured) {
            merge_field(
                &mut job.closes_at,
                Sourced::with_confidence(d, Provenance::Llm, confidence),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobseeker_core::domain::enums::WorkMode;
    use jobseeker_llm::mock::MockClient;

    #[tokio::test]
    async fn a_complete_job_does_not_call_the_model() {
        let mut job = ExtractedJob::default();
        job.title = Some(Sourced::new("E".into(), Provenance::Jsonld));
        job.company_name = Some(Sourced::new("C".into(), Provenance::Jsonld));
        job.description_md = Some(Sourced::new("D".into(), Provenance::Jsonld));
        job.work_mode = Some(Sourced::new(WorkMode::Remote, Provenance::Jsonld));
        job.employment_type = Some(Sourced::new(
            jobseeker_core::domain::enums::EmploymentType::FullTime,
            Provenance::Jsonld,
        ));
        job.salary = Some(Sourced::new(
            RawSalary {
                text: "$1".into(),
                country_hint: None,
                source_kind: None,
            },
            Provenance::Jsonld,
        ));
        job.posted_at = Some(Sourced::new(
            parse_posted_date("2026-09-02", now()).unwrap(),
            Provenance::Jsonld,
        ));
        job.apply_url = Some(Sourced::new("https://x".into(), Provenance::Jsonld));
        let mock = MockClient::new("m");
        fill_gaps(&mut job, "desc", &mock, 1000).await.unwrap();
        assert_eq!(mock.call_count(), 0);
    }

    #[tokio::test]
    async fn canned_mock_fields_are_merged_without_clobbering_stronger_provenance() {
        let mut job = ExtractedJob::default();
        job.title = Some(Sourced::new("Keep Me".into(), Provenance::Jsonld));
        let mock = MockClient::new("m");
        fill_gaps(&mut job, "a posting", &mock, 1000).await.unwrap();
        assert_eq!(
            job.title.unwrap().value,
            "Keep Me",
            "JSON-LD outranks the model"
        );
        assert_eq!(
            job.company_name.as_ref().map(|s| s.value.as_str()),
            Some("Acme Robotics"),
            "the gap the mock fills"
        );
        assert_eq!(
            job.company_name.as_ref().map(|s| s.provenance),
            Some(Provenance::Llm)
        );
    }
}
