//! Indeed: prefer `window._initialData` over the DOM. Salary is usually an estimate.

use jobseeker_core::domain::enums::SourceKind;
use jobseeker_core::domain::job::ExtractedJob;
use serde_json::Value;

use super::{
    first_attr, first_inner_html, first_text, js_assignment_object, json_text, set_company,
    set_description_html, set_location, set_salary, set_source_job_id, set_title, CaptureView,
    SiteAdapter,
};

pub struct Indeed;

impl SiteAdapter for Indeed {
    fn id(&self) -> &'static str {
        "indeed"
    }

    fn matches(&self, url: &str) -> bool {
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        let Some(host) = parsed.host_str() else {
            return false;
        };
        let lowered = host.to_ascii_lowercase();
        let host = lowered.strip_prefix("www.").unwrap_or(lowered.as_str());
        host == "indeed.com"
            || host.starts_with("indeed.")
            || host.ends_with(".indeed.com")
            || host.contains(".indeed.")
    }

    fn extract(&self, job: &mut ExtractedJob, cap: &CaptureView<'_>) {
        if let Some(data) = js_assignment_object(cap.html, "_initialData")
            .or_else(|| js_assignment_object(cap.html, "mosaic.providerData"))
        {
            apply_json(job, &data);
        }
        set_title(
            job,
            first_text(
                cap.document,
                &[
                    "h1.jobsearch-JobInfoHeader-title",
                    "h1.icl-u-xs-mb--xs",
                    "h1",
                ],
            )
            .unwrap_or_default(),
        );
        set_company(
            job,
            first_text(
                cap.document,
                &[
                    "[data-company-name='true']",
                    ".jobsearch-InlineCompanyRating",
                    ".jobsearch-CompanyInfoContainer",
                ],
            )
            .unwrap_or_default(),
        );
        set_location(
            job,
            first_text(
                cap.document,
                &[
                    "[data-testid='inlineHeader-companyLocation']",
                    ".jobsearch-JobInfoHeader-subtitle",
                ],
            )
            .unwrap_or_default(),
        );
        if let Some(html) = first_inner_html(
            cap.document,
            &["#jobDescriptionText", ".jobsearch-jobDescriptionText"],
        ) {
            set_description_html(job, &html);
        }
        if let Some(jk) = first_attr(cap.document, &["[data-jk]"], "data-jk")
            .or_else(|| jk_from_url(cap.url.unwrap_or("")))
        {
            set_source_job_id(job, jk);
        }
    }
}

fn apply_json(job: &mut ExtractedJob, data: &Value) {
    let posting = data
        .get("jobInfoWrapperModel")
        .and_then(|v| v.get("jobInfoModel"))
        .unwrap_or(data);
    set_title(
        job,
        json_text(posting, &["jobTitle", "title"]).unwrap_or_default(),
    );
    set_company(
        job,
        json_text(posting, &["companyName", "company"]).unwrap_or_default(),
    );
    set_location(
        job,
        json_text(posting, &["jobLocation", "formattedLocation", "location"]).unwrap_or_default(),
    );
    if let Some(salary) = json_text(
        posting,
        &[
            "salaryText",
            "salarySnippet",
            "estimatedSalary",
            "compensation",
        ],
    ) {
        set_salary(job, salary, SourceKind::Indeed);
    }
    if let Some(html) = json_text(
        posting,
        &["jobDescription", "description", "sanitizedJobDescription"],
    ) {
        set_description_html(job, &html);
    }
    if let Some(jk) = json_text(posting, &["jobkey", "jobKey", "jk"]) {
        set_source_job_id(job, jk);
    }
}

fn jk_from_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "jk" || k == "vjk")
        .map(|(_, v)| v.into_owned())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{apply, CaptureView};
    use crate::html;
    use jobseeker_core::provenance::Provenance;

    const HTML: &str = r#"<html><body data-jk="a1b2c3d4e5f6">
<script>
window._initialData = {
  "jobTitle": "Senior SRE",
  "companyName": "Plaidline",
  "jobLocation": "Remote",
  "salaryText": "Estimated $160,000 - $190,000 a year",
  "jobDescription": "<h2>Requirements</h2><ul><li>7+ years Rust</li><li>Kubernetes on-call</li></ul>",
  "jobkey": "a1b2c3d4e5f6",
  "unused": undefined
};
</script>
</body></html>"#;

    #[test]
    fn initial_data_json_is_preferred_to_the_dom() {
        let doc = html::parse(HTML);
        let mut job = ExtractedJob::default();
        apply(
            &mut job,
            &CaptureView {
                url: Some("https://www.indeed.com/viewjob?jk=a1b2c3d4e5f6"),
                html: HTML,
                document: &doc,
                page_meta: None,
            },
        );
        assert_eq!(job.title.as_ref().unwrap().value, "Senior SRE");
        assert_eq!(job.title.as_ref().unwrap().provenance, Provenance::Adapter);
        assert_eq!(job.company_name.as_ref().unwrap().value, "Plaidline");
        assert_eq!(job.source_job_id.as_ref().unwrap().value, "a1b2c3d4e5f6");
        assert!(job
            .salary
            .as_ref()
            .unwrap()
            .value
            .text
            .to_ascii_lowercase()
            .contains("estimated"));
        assert!(job
            .description_md
            .as_ref()
            .is_some_and(|d| d.value.contains("Rust")));
    }

    #[test]
    fn country_tlds_match_without_a_loose_substring() {
        assert!(Indeed.matches("https://uk.indeed.com/viewjob?jk=abc"));
        assert!(Indeed.matches("https://www.indeed.co.uk/viewjob?jk=abc"));
        assert!(Indeed.matches("https://indeed.ca/viewjob?jk=abc"));
        assert!(!Indeed.matches("https://example.com/indeed.com/jobs"));
        assert!(!Indeed.matches("https://boards.greenhouse.io/acme/jobs/1"));
    }
}
