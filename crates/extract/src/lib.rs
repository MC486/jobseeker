//! Raw captures → a structured, provenance-tagged [`ExtractedJob`].
//!
//! Stages run in a fixed order and only fill fields that are still empty. A weaker stage
//! cannot overwrite a stronger one (`docs/06-extraction.md`). The pipeline is required to
//! produce a usable record with deterministic stages alone; the model fills gaps when it
//! is configured and is skipped, without failing the job, when it is not.

pub mod adapter;
pub mod html;
pub mod infer;
pub mod jsonld;
pub mod llm_stage;
pub mod rules;

use jobseeker_core::domain::capture::CaptureMethod;
use jobseeker_core::domain::enums::SourceKind;
use jobseeker_core::domain::job::ExtractedJob;
use jobseeker_core::provenance::{merge_field, Provenance, Sourced};
use jobseeker_core::Result;
use jobseeker_llm::LlmClient;
use jobseeker_normalize::requirement::{atomize, AtomizedRequirement};
use jobseeker_normalize::text::{markdown_to_text, tidy_markdown};

/// Everything a capture knows about itself that extraction is allowed to use.
#[derive(Debug, Clone)]
pub struct ExtractInput {
    pub url: Option<String>,
    pub body: String,
    pub content_type: Option<String>,
    pub method: CaptureMethod,
    pub source: SourceKind,
    /// JSON-LD the extension already harvested, so a server-side parse failure does not
    /// lose the best available signal.
    pub page_meta: Option<serde_json::Value>,
}

/// What extraction produced, plus the atomized requirements and whether the model stage
/// was skipped or failed.
#[derive(Debug, Clone)]
pub struct Extraction {
    pub job: ExtractedJob,
    pub requirements: Vec<AtomizedRequirement>,
    pub description_md: String,
    pub description_text: String,
    pub partial: bool,
    pub model: Option<String>,
}

impl Extraction {
    pub fn mean_confidence(&self) -> Option<f32> {
        self.job.mean_confidence()
    }
}

/// Run every deterministic stage, then optionally the model, then inference.
///
/// Deterministic HTML parsing is a separate `fn` so `scraper::Html` (`!Send`) never
/// appears in the async state machine and workers can run on the multi-thread runtime.
pub async fn extract(
    input: &ExtractInput,
    llm: Option<&dyn LlmClient>,
    max_llm_chars: usize,
) -> Result<Extraction> {
    let (mut job, description_md, description_text) = extract_deterministic(input);

    let mut partial = false;
    let mut model = None;
    if let Some(client) = llm {
        match llm_stage::fill_gaps(&mut job, &description_text, client, max_llm_chars).await {
            Ok(()) => model = Some(client.model().to_string()),
            Err(e) if e.code() == "llm_unavailable" || e.code() == "schema_violation" => {
                tracing::warn!(error = %e, "model stage skipped; the record will be marked partial");
                partial = true;
            }
            Err(e) => return Err(e),
        }
    } else if !job.missing_fields().is_empty() {
        partial = true;
    }

    infer::apply(&mut job);

    let requirements = if description_md.trim().is_empty() {
        Vec::new()
    } else {
        atomize(&description_md)
    };

    Ok(Extraction {
        job,
        requirements,
        description_md,
        description_text,
        partial,
        model,
    })
}

fn extract_deterministic(input: &ExtractInput) -> (ExtractedJob, String, String) {
    let mut job = ExtractedJob::default();
    let is_json = input
        .content_type
        .as_deref()
        .is_some_and(|c| c.contains("json"))
        || input.body.trim_start().starts_with('{')
        || input.body.trim_start().starts_with('[');

    if is_json {
        jsonld::apply_json_value(&mut job, &parse_json(&input.body), Provenance::Api);
    }

    let document = html::parse(&input.body);
    jsonld::apply_from_html(&mut job, &document, input.page_meta.as_ref());
    adapter::apply(
        &mut job,
        &adapter::CaptureView {
            url: input.url.as_deref(),
            html: &input.body,
            document: &document,
            page_meta: input.page_meta.as_ref(),
        },
    );
    if let Some(url) = &input.url {
        rules::apply_url_hints(&mut job, url, input.source);
    }
    rules::apply_html(&mut job, &document, input.source);

    let description_md = job
        .description_md
        .as_ref()
        .map(|s| tidy_markdown(&s.value))
        .or_else(|| {
            job.description_html
                .as_ref()
                .map(|s| html::to_markdown(&s.value))
        })
        .unwrap_or_else(|| html::to_markdown(&input.body));
    if job.description_md.is_none() && !description_md.trim().is_empty() {
        merge_field(
            &mut job.description_md,
            Sourced::new(description_md.clone(), Provenance::Rules),
        );
    }
    let description_text = markdown_to_text(&description_md);
    (job, description_md, description_text)
}

fn parse_json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobseeker_core::domain::enums::{EmploymentType, WorkMode};
    use jobseeker_llm::mock::MockClient;
    use jobseeker_llm::Purpose;

    const GREENHOUSE_HTML: &str = r#"<!doctype html>
<html><head>
<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "JobPosting",
  "title": "Senior Platform Engineer",
  "hiringOrganization": {"@type": "Organization", "name": "Acme Robotics"},
  "datePosted": "2026-09-02",
  "validThrough": "2026-10-15",
  "employmentType": "FULL_TIME",
  "jobLocationType": "TELECOMMUTE",
  "applicantLocationRequirements": {"@type": "Country", "name": "US"},
  "baseSalary": {
    "@type": "MonetaryAmount",
    "currency": "USD",
    "value": {"@type": "QuantitativeValue", "minValue": 185000, "maxValue": 225000, "unitText": "YEAR"}
  },
  "description": "<h2>Requirements</h2><ul><li>5+ years building distributed systems</li><li>3+ years operating Kubernetes</li><li>Production Rust</li></ul>",
  "url": "https://boards.greenhouse.io/acmerobotics/jobs/5512034"
}
</script>
</head><body><main><h1>Senior Platform Engineer</h1></main></body></html>"#;

    fn input(body: &str) -> ExtractInput {
        ExtractInput {
            url: Some("https://boards.greenhouse.io/acmerobotics/jobs/5512034".into()),
            body: body.to_string(),
            content_type: Some("text/html".into()),
            method: CaptureMethod::Http,
            source: SourceKind::Greenhouse,
            page_meta: None,
        }
    }

    #[tokio::test]
    async fn json_ld_alone_produces_a_minimum_viable_job() {
        let out = extract(&input(GREENHOUSE_HTML), None, 16_000)
            .await
            .unwrap();
        assert!(out.job.has_minimum_viable_fields());
        assert_eq!(
            out.job.title.as_ref().unwrap().value,
            "Senior Platform Engineer"
        );
        assert_eq!(
            out.job.company_name.as_ref().unwrap().value,
            "Acme Robotics"
        );
        assert_eq!(
            out.job.title.as_ref().unwrap().provenance,
            Provenance::Jsonld
        );
        assert_eq!(out.job.work_mode.as_ref().unwrap().value, WorkMode::Remote);
        assert_eq!(
            out.job.employment_type.as_ref().unwrap().value,
            EmploymentType::FullTime
        );
        assert!(
            !out.job.locations.is_empty(),
            "remote-US must become a location"
        );
        assert!(out.job.salary.is_some());
        assert!(!out.requirements.is_empty(), "bullets must be atomized");
        assert!(
            out.requirements
                .iter()
                .any(|r| r.text.contains("Kubernetes")),
            "got {:?}",
            out.requirements.iter().map(|r| &r.text).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn the_model_is_not_consulted_when_json_ld_already_filled_the_gaps() {
        let mock = MockClient::new("mock");
        let out = extract(&input(GREENHOUSE_HTML), Some(&mock), 16_000)
            .await
            .unwrap();
        assert_eq!(
            mock.calls_for(Purpose::ExtractFields),
            0,
            "a complete JSON-LD posting must not cost tokens: missing {:?}",
            out.job.missing_fields()
        );
        assert!(!out.partial);
    }

    #[tokio::test]
    async fn a_bare_html_page_still_lands_via_heuristics() {
        let html = r#"<html><body>
            <h1>Staff Engineer</h1>
            <p>Acme Robotics is hiring a staff engineer to lead the platform team.</p>
            <p>Location: Remote - United States</p>
            <p>Salary: $185,000 - $225,000 per year</p>
            <h2>Requirements</h2>
            <ul><li>8+ years of systems programming</li></ul>
        </body></html>"#;
        let out = extract(&input(html), None, 16_000).await.unwrap();
        assert_eq!(out.job.title.as_ref().unwrap().value, "Staff Engineer");
        assert_eq!(
            out.job.seniority.as_ref().unwrap().value,
            jobseeker_core::domain::enums::Seniority::Staff
        );
        assert!(
            out.job.salary.is_some(),
            "the salary string must be captured"
        );
        assert!(
            out.partial,
            "without a model and without JSON-LD, several fields stay empty"
        );
    }

    #[tokio::test]
    async fn a_model_failure_does_not_lose_the_deterministic_fields() {
        let html = "<html><body><h1>Engineer</h1><p>Come work at Acme.</p></body></html>";
        let mock = MockClient::new("mock");
        mock.fail_once();
        let out = extract(&input(html), Some(&mock), 16_000).await.unwrap();
        assert_eq!(out.job.title.as_ref().unwrap().value, "Engineer");
        assert!(out.partial, "the record must be flagged, not failed");
    }

    #[tokio::test]
    async fn a_json_api_body_is_read_as_api_provenance() {
        let body = r#"{"title":"Platform Engineer","hiringOrganization":{"name":"Acme"},"description":"<p>Build things</p>"}"#;
        let mut inp = input(body);
        inp.content_type = Some("application/json".into());
        let out = extract(&inp, None, 16_000).await.unwrap();
        assert_eq!(out.job.title.as_ref().unwrap().provenance, Provenance::Api);
        assert_eq!(out.job.company_name.as_ref().unwrap().value, "Acme");
    }

    #[tokio::test]
    async fn linkedin_capture_html_is_tagged_adapter() {
        let html = include_str!("../../../fixtures/linkedin-job-capture.html");
        let inp = ExtractInput {
            url: Some("https://www.linkedin.com/jobs/view/4294967296".into()),
            body: html.to_string(),
            content_type: Some("text/html".into()),
            method: CaptureMethod::Extension,
            source: SourceKind::LinkedIn,
            page_meta: None,
        };
        let out = extract(&inp, None, 16_000).await.unwrap();
        assert_eq!(
            out.job.title.as_ref().unwrap().value,
            "Staff Platform Engineer"
        );
        assert_eq!(
            out.job.title.as_ref().unwrap().provenance,
            Provenance::Adapter
        );
        assert_eq!(
            out.job.company_name.as_ref().unwrap().value,
            "Acme LinkedIn"
        );
        assert_eq!(out.job.source_job_id.as_ref().unwrap().value, "4294967296");
        assert!(
            out.requirements
                .iter()
                .any(|r| r.text.contains("Kubernetes")),
            "got {:?}",
            out.requirements.iter().map(|r| &r.text).collect::<Vec<_>>()
        );
        assert_eq!(out.job.work_mode.as_ref().unwrap().value, WorkMode::Remote);
    }

    #[tokio::test]
    async fn workday_cxs_json_fills_both_location_options() {
        let body = include_str!("../../../fixtures/workday-cxs-job.json");
        let inp = ExtractInput {
            url: Some(
                "https://zillow.wd5.myworkdayjobs.com/en-US/Zillow_Group_External/job/Remote-USA/Data-Scientist_P751219-2"
                    .into(),
            ),
            body: body.to_string(),
            content_type: Some("application/json".into()),
            method: CaptureMethod::Api,
            source: SourceKind::Workday,
            page_meta: None,
        };
        let out = extract(&inp, None, 16_000).await.unwrap();
        assert_eq!(out.job.title.as_ref().unwrap().value, "Data Scientist");
        assert_eq!(out.job.title.as_ref().unwrap().provenance, Provenance::Api);
        assert_eq!(
            out.job.company_name.as_ref().unwrap().value,
            "Acme Robotics"
        );
        assert_eq!(out.job.source_job_id.as_ref().unwrap().value, "P751219-2");
        assert_eq!(out.job.work_mode.as_ref().unwrap().value, WorkMode::Remote);
        assert_eq!(out.job.locations.len(), 2, "Remote-USA and Seattle");
        assert!(out
            .requirements
            .iter()
            .any(|r| r.text.contains("Python") || r.text.contains("Snowflake")));
    }

    #[tokio::test]
    async fn workday_html_without_cxs_uses_the_adapter() {
        let html = include_str!("../../../fixtures/workday-job-capture.html");
        let inp = ExtractInput {
            url: Some(
                "https://acme.wd1.myworkdayjobs.com/en-US/acme_careers/job/Remote-USA/Data-Scientist_P751219-2"
                    .into(),
            ),
            body: html.to_string(),
            content_type: Some("text/html".into()),
            method: CaptureMethod::Extension,
            source: SourceKind::Workday,
            page_meta: None,
        };
        let out = extract(&inp, None, 16_000).await.unwrap();
        assert_eq!(out.job.title.as_ref().unwrap().value, "Data Scientist");
        assert_eq!(
            out.job.title.as_ref().unwrap().provenance,
            Provenance::Adapter
        );
        assert_eq!(
            out.job.company_name.as_ref().unwrap().value,
            "Acme Robotics"
        );
        assert_eq!(out.job.locations.len(), 2);
        assert_eq!(out.job.source_job_id.as_ref().unwrap().value, "P751219-2");
    }

    #[tokio::test]
    async fn json_ld_outranks_the_greenhouse_html_adapter() {
        let out = extract(&input(GREENHOUSE_HTML), None, 16_000)
            .await
            .unwrap();
        assert_eq!(
            out.job.title.as_ref().unwrap().provenance,
            Provenance::Jsonld,
            "stage 2 must beat the boards.greenhouse.io HTML adapter"
        );
    }

    #[tokio::test]
    async fn greenhouse_html_without_json_ld_uses_the_adapter() {
        let html = include_str!("../../../fixtures/greenhouse-board-job.html");
        let out = extract(&input(html), None, 16_000).await.unwrap();
        assert_eq!(out.job.title.as_ref().unwrap().value, "Platform Engineer");
        assert_eq!(
            out.job.title.as_ref().unwrap().provenance,
            Provenance::Adapter
        );
        assert_eq!(out.job.company_name.as_ref().unwrap().value, "Contoso");
        assert_eq!(out.job.source_job_id.as_ref().unwrap().value, "5512034");
        assert!(out
            .requirements
            .iter()
            .any(|r| r.text.contains("Kubernetes")));
    }
}
