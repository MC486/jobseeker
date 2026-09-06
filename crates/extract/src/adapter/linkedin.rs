//! LinkedIn hydrated DOM. The server never fetches these pages (`NeedsBrowser`);
//! this adapter runs on extension/paste HTML only.

use jobseeker_core::domain::enums::SourceKind;
use jobseeker_core::domain::job::ExtractedJob;

use super::{
    first_attr, first_inner_html, first_text, host_matches, set_company, set_description_html,
    set_location, set_salary, set_source_job_id, set_title, CaptureView, SiteAdapter,
};

pub struct LinkedIn;

impl SiteAdapter for LinkedIn {
    fn id(&self) -> &'static str {
        "linkedin"
    }

    fn matches(&self, url: &str) -> bool {
        host_matches(url, &["linkedin.com"])
    }

    fn extract(&self, job: &mut ExtractedJob, cap: &CaptureView<'_>) {
        set_title(
            job,
            first_text(
                cap.document,
                &[
                    ".job-details-jobs-unified-top-card__job-title",
                    "h1.t-24",
                    "h1.jobs-unified-top-card__job-title",
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
                    ".job-details-jobs-unified-top-card__company-name",
                    ".jobs-unified-top-card__company-name",
                    "a.jobs-unified-top-card__company-name",
                ],
            )
            .unwrap_or_default(),
        );
        set_location(
            job,
            first_text(
                cap.document,
                &[
                    ".job-details-jobs-unified-top-card__primary-description-container",
                    ".job-details-jobs-unified-top-card__bullet",
                    ".jobs-unified-top-card__bullet",
                ],
            )
            .unwrap_or_default(),
        );
        if let Some(comp) = first_text(
            cap.document,
            &[
                ".job-details-jobs-unified-top-card__job-insight",
                "[class*='compensation']",
                ".salary-main-rail__data-amount",
            ],
        ) {
            if looks_like_comp(&comp) {
                set_salary(job, comp, SourceKind::LinkedIn);
            }
        }
        if let Some(html) = first_inner_html(
            cap.document,
            &[
                ".jobs-description__content",
                ".jobs-box__html-content",
                "#job-details",
                ".jobs-description-content__text",
            ],
        ) {
            set_description_html(job, &html);
        }
        if let Some(id) = first_attr(cap.document, &["[data-job-id]"], "data-job-id")
            .or_else(|| {
                first_attr(cap.document, &["[data-entity-urn]"], "data-entity-urn")
                    .and_then(|urn| linkedin_id_from_urn(&urn))
            })
            .or_else(|| linkedin_id_from_url(cap.url.unwrap_or("")))
        {
            set_source_job_id(job, id);
        }
    }
}

fn looks_like_comp(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    (lower.contains('$') || lower.contains("€") || lower.contains("£"))
        && (lower.contains("estimate")
            || lower.contains("/yr")
            || lower.contains("year")
            || lower.contains("hour")
            || lower.contains("/hr"))
}

fn linkedin_id_from_urn(urn: &str) -> Option<String> {
    let token = urn.rsplit(':').next()?;
    if token.chars().all(|c| c.is_ascii_digit()) && token.len() >= 5 {
        Some(token.to_string())
    } else {
        None
    }
}

fn linkedin_id_from_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if let Some(id) = parsed
        .query_pairs()
        .find(|(k, _)| k == "currentJobId")
        .map(|(_, v)| v.into_owned())
    {
        if id.chars().all(|c| c.is_ascii_digit()) {
            return Some(id);
        }
    }
    let path = parsed.path();
    let marker = "/jobs/view/";
    let rest = path.split(marker).nth(1)?;
    let token = rest.split('/').next()?.split('-').next_back()?;
    if token.chars().all(|c| c.is_ascii_digit()) && token.len() >= 5 {
        Some(token.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{apply, CaptureView};
    use crate::html;
    use jobseeker_core::provenance::Provenance;

    const HTML: &str = r#"<html><body>
<div class="jobs-unified-top-card" data-job-id="4123456789">
  <h1 class="job-details-jobs-unified-top-card__job-title">Staff Rust Engineer</h1>
  <div class="job-details-jobs-unified-top-card__company-name">Northwind</div>
  <div class="job-details-jobs-unified-top-card__primary-description-container">Remote · United States</div>
  <div class="job-details-jobs-unified-top-card__job-insight">Estimated $180,000 - $220,000/yr</div>
</div>
<div class="jobs-description__content">
  <h2>Requirements</h2>
  <ul><li>8+ years Rust</li><li>Production Kubernetes</li></ul>
</div>
</body></html>"#;

    #[test]
    fn hydrated_dom_fills_title_company_estimate_and_id() {
        let doc = html::parse(HTML);
        let mut job = ExtractedJob::default();
        apply(
            &mut job,
            &CaptureView {
                url: Some("https://www.linkedin.com/jobs/view/4123456789"),
                html: HTML,
                document: &doc,
                page_meta: None,
            },
        );
        assert_eq!(job.title.as_ref().unwrap().value, "Staff Rust Engineer");
        assert_eq!(job.title.as_ref().unwrap().provenance, Provenance::Adapter);
        assert_eq!(job.company_name.as_ref().unwrap().value, "Northwind");
        assert_eq!(job.source_job_id.as_ref().unwrap().value, "4123456789");
        let salary = &job.salary.as_ref().unwrap().value.text;
        assert!(
            salary.to_ascii_lowercase().contains("estimated"),
            "{salary}"
        );
        assert!(job
            .description_md
            .as_ref()
            .is_some_and(|d| d.value.contains("Kubernetes")));
        assert_eq!(
            job.work_mode.as_ref().unwrap().value,
            jobseeker_core::domain::enums::WorkMode::Remote
        );
    }

    #[test]
    fn missing_selectors_do_not_fail() {
        let html = "<html><body><p>logged out</p></body></html>";
        let doc = html::parse(html);
        let mut job = ExtractedJob::default();
        LinkedIn.extract(
            &mut job,
            &CaptureView {
                url: Some("https://www.linkedin.com/jobs/view/1"),
                html,
                document: &doc,
                page_meta: None,
            },
        );
        assert!(job.title.is_none());
        assert!(job.description_md.is_none());
    }

    #[test]
    fn entity_urn_yields_the_numeric_id() {
        assert_eq!(
            linkedin_id_from_urn("urn:li:jobPosting:4123456789").as_deref(),
            Some("4123456789")
        );
        assert_eq!(linkedin_id_from_urn("urn:li:company:123"), None);
    }
}
