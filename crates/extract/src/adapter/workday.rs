//! Workday HTML fallback. Prefer the CXS JSON API (stage 1) when fetching.

use jobseeker_core::domain::job::ExtractedJob;
use jobseeker_normalize::url::canonicalize;

use super::{
    add_location, all_texts, first_inner_html, first_text, host_matches, set_company,
    set_description_html, set_source_job_id, set_title, CaptureView, SiteAdapter,
};

pub struct Workday;

impl SiteAdapter for Workday {
    fn id(&self) -> &'static str {
        "workday"
    }

    fn matches(&self, url: &str) -> bool {
        host_matches(url, &["myworkdayjobs.com", "myworkday.com"])
    }

    fn extract(&self, job: &mut ExtractedJob, cap: &CaptureView<'_>) {
        set_title(
            job,
            first_text(
                cap.document,
                &[
                    "[data-automation-id='jobPostingHeader']",
                    "[data-automation-id='jobPostingTitle']",
                    "h2[data-automation-id='jobPostingHeader']",
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
                    "[data-automation-id='logoTitle']",
                    "[data-automation-id='company']",
                    "[data-automation-id='subtitle']",
                ],
            )
            .or_else(|| tenant_as_company(cap.url.unwrap_or("")))
            .unwrap_or_default(),
        );
        for loc in all_texts(
            cap.document,
            &[
                "[data-automation-id='locations']",
                "[data-automation-id='location']",
                "[data-automation-id='locatedIn']",
                "div[data-automation-id='subtitle'] dd",
            ],
        ) {
            add_location(job, loc);
        }
        if let Some(html) = first_inner_html(
            cap.document,
            &[
                "[data-automation-id='jobPostingDescription']",
                "[data-automation-id='job-posting-details']",
                "section[data-automation-id='job-posting']",
            ],
        ) {
            set_description_html(job, &html);
        }
        if let Some(id) = workday_req_id(cap.url.unwrap_or("")) {
            set_source_job_id(job, id);
        }
    }
}

fn workday_req_id(url: &str) -> Option<String> {
    canonicalize(url).ok().and_then(|c| c.source_job_id)
}

fn tenant_as_company(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let tenant = host.split('.').next()?;
    if tenant.is_empty() || tenant == "www" {
        return None;
    }
    let mut chars = tenant.chars();
    let first = chars.next()?.to_ascii_uppercase();
    Some(format!("{first}{}", chars.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{apply, CaptureView};
    use crate::html;
    use jobseeker_core::domain::enums::WorkMode;
    use jobseeker_core::provenance::Provenance;

    const HTML: &str = r#"<html><body>
<h2 data-automation-id="jobPostingHeader">Data Scientist</h2>
<div data-automation-id="logoTitle">Zillow</div>
<div data-automation-id="locations">Remote-USA</div>
<div data-automation-id="locations">Seattle, WA</div>
<div data-automation-id="jobPostingDescription">
  <h2>Requirements</h2>
  <ul><li>Python</li><li>SQL</li><li>Snowflake</li></ul>
</div>
</body></html>"#;

    #[test]
    fn html_fallback_reads_automation_ids_and_both_locations() {
        let doc = html::parse(HTML);
        let mut job = ExtractedJob::default();
        apply(
            &mut job,
            &CaptureView {
                url: Some(
                    "https://zillow.wd5.myworkdayjobs.com/en-US/Zillow_Group_External/job/Remote-USA/Data-Scientist_P751219-2",
                ),
                html: HTML,
                document: &doc,
                page_meta: None,
            },
        );
        assert_eq!(job.title.as_ref().unwrap().value, "Data Scientist");
        assert_eq!(job.title.as_ref().unwrap().provenance, Provenance::Adapter);
        assert_eq!(job.company_name.as_ref().unwrap().value, "Zillow");
        assert_eq!(job.source_job_id.as_ref().unwrap().value, "P751219-2");
        assert_eq!(job.locations.len(), 2);
        assert_eq!(job.locations[0].value.text, "Remote-USA");
        assert_eq!(job.locations[1].value.text, "Seattle, WA");
        assert_eq!(job.work_mode.as_ref().unwrap().value, WorkMode::Remote);
        assert!(job
            .description_md
            .as_ref()
            .is_some_and(|d| d.value.contains("Snowflake")));
    }
}
