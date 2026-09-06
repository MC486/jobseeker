//! Lever HTML fallback. Prefer `api.lever.co` when fetching.

use jobseeker_core::domain::job::ExtractedJob;

use super::{
    first_attr, first_inner_html, first_text, host_matches, set_company, set_description_html,
    set_location, set_source_job_id, set_title, CaptureView, SiteAdapter,
};

pub struct Lever;

impl SiteAdapter for Lever {
    fn id(&self) -> &'static str {
        "lever"
    }

    fn matches(&self, url: &str) -> bool {
        host_matches(url, &["lever.co"])
    }

    fn extract(&self, job: &mut ExtractedJob, cap: &CaptureView<'_>) {
        set_title(
            job,
            first_text(
                cap.document,
                &[".posting-headline h2", ".posting-title", "h2"],
            )
            .unwrap_or_default(),
        );
        set_company(
            job,
            first_attr(
                cap.document,
                &[".main-header-logo img", "img.main-header-logo"],
                "alt",
            )
            .or_else(|| first_text(cap.document, &[".company-name"]))
            .or_else(|| lever_board(cap.url.unwrap_or("")))
            .unwrap_or_default(),
        );
        set_location(
            job,
            first_text(
                cap.document,
                &[
                    ".posting-categories .location",
                    ".posting-category.location",
                ],
            )
            .unwrap_or_default(),
        );
        if let Some(html) = first_inner_html(
            cap.document,
            &[
                ".posting-description",
                ".section-wrapper",
                "[data-qa='job-description']",
            ],
        ) {
            set_description_html(job, &html);
        }
        if let Some(id) = lever_id(cap.url.unwrap_or("")) {
            set_source_job_id(job, id);
        }
    }
}

fn lever_board(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed.path_segments()?.next().map(str::to_string)
}

fn lever_id(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let mut segs = parsed.path_segments()?;
    segs.next()?; // board
    segs.next()
        .map(str::to_string)
        .filter(|s| !s.is_empty() && s != "apply")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{apply, CaptureView};
    use crate::html;
    use jobseeker_core::provenance::Provenance;

    const HTML: &str = r#"<html><body>
<div class="posting-headline"><h2>Backend Engineer</h2></div>
<div class="posting-categories"><div class="location">San Francisco, CA</div></div>
<div class="posting-description">
  <h3>Requirements</h3>
  <ul><li>Go or Rust</li><li>Distributed systems</li></ul>
</div>
</body></html>"#;

    #[test]
    fn lever_html_fills_title_and_description() {
        let doc = html::parse(HTML);
        let mut job = ExtractedJob::default();
        apply(
            &mut job,
            &CaptureView {
                url: Some("https://jobs.lever.co/acme/1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed"),
                html: HTML,
                document: &doc,
                page_meta: None,
            },
        );
        assert_eq!(job.title.as_ref().unwrap().value, "Backend Engineer");
        assert_eq!(job.title.as_ref().unwrap().provenance, Provenance::Adapter);
        assert_eq!(
            job.source_job_id.as_ref().unwrap().value,
            "1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed"
        );
        assert!(job
            .description_md
            .as_ref()
            .is_some_and(|d| d.value.contains("Distributed")));
    }
}
