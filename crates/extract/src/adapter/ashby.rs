//! Ashby HTML fallback. Prefer the posting-api when fetching.

use jobseeker_core::domain::job::ExtractedJob;

use super::{
    first_inner_html, first_text, host_matches, set_company, set_description_html, set_location,
    set_source_job_id, set_title, CaptureView, SiteAdapter,
};

pub struct Ashby;

impl SiteAdapter for Ashby {
    fn id(&self) -> &'static str {
        "ashby"
    }

    fn matches(&self, url: &str) -> bool {
        host_matches(url, &["ashbyhq.com"])
    }

    fn extract(&self, job: &mut ExtractedJob, cap: &CaptureView<'_>) {
        set_title(
            job,
            first_text(cap.document, &["h1", "[class*='JobHeading']"]).unwrap_or_default(),
        );
        set_company(
            job,
            first_text(cap.document, &["[class*='CompanyName']", "a[href='/']"])
                .or_else(|| ashby_board(cap.url.unwrap_or("")))
                .unwrap_or_default(),
        );
        set_location(
            job,
            first_text(
                cap.document,
                &["[class*='Location']", "[class*='job-location']"],
            )
            .unwrap_or_default(),
        );
        if let Some(html) = first_inner_html(
            cap.document,
            &[
                "[class*='JobDescription']",
                "[class*='ashby-job-posting-description']",
                "article",
            ],
        ) {
            set_description_html(job, &html);
        }
        if let Some(id) = ashby_id(cap.url.unwrap_or("")) {
            set_source_job_id(job, id);
        }
    }
}

fn ashby_board(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed.path_segments()?.next().map(str::to_string)
}

fn ashby_id(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let mut segs = parsed.path_segments()?;
    segs.next()?;
    segs.next().map(str::to_string).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{apply, CaptureView};
    use crate::html;
    use jobseeker_core::provenance::Provenance;

    const HTML: &str = r#"<html><body>
<h1>Infrastructure Engineer</h1>
<div class="CompanyName">Harbor Labs</div>
<div class="Location">Remote</div>
<article class="JobDescription">
  <h2>Requirements</h2>
  <ul><li>6+ years Rust</li></ul>
</article>
</body></html>"#;

    #[test]
    fn ashby_html_fills_core_fields() {
        let doc = html::parse(HTML);
        let mut job = ExtractedJob::default();
        apply(
            &mut job,
            &CaptureView {
                url: Some("https://jobs.ashbyhq.com/harbor/infra-lead-99"),
                html: HTML,
                document: &doc,
                page_meta: None,
            },
        );
        assert_eq!(job.title.as_ref().unwrap().value, "Infrastructure Engineer");
        assert_eq!(job.title.as_ref().unwrap().provenance, Provenance::Adapter);
        assert_eq!(job.company_name.as_ref().unwrap().value, "Harbor Labs");
        assert_eq!(job.source_job_id.as_ref().unwrap().value, "infra-lead-99");
    }
}
