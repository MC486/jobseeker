//! Greenhouse HTML fallback. Prefer the published JSON API (stage 1) when fetching.

use jobseeker_core::domain::job::ExtractedJob;

use super::{
    first_inner_html, first_text, host_matches, set_company, set_description_html, set_location,
    set_source_job_id, set_title, CaptureView, SiteAdapter,
};

pub struct Greenhouse;

impl SiteAdapter for Greenhouse {
    fn id(&self) -> &'static str {
        "greenhouse"
    }

    fn matches(&self, url: &str) -> bool {
        host_matches(url, &["greenhouse.io"])
    }

    fn extract(&self, job: &mut ExtractedJob, cap: &CaptureView<'_>) {
        set_title(
            job,
            first_text(cap.document, &["h1.app-title", ".app-title", "h1"]).unwrap_or_default(),
        );
        set_company(
            job,
            first_text(
                cap.document,
                &[
                    ".company-name",
                    "#header .company-name",
                    "span.company-name",
                ],
            )
            .unwrap_or_default(),
        );
        set_location(
            job,
            first_text(cap.document, &[".location", "#header .location"]).unwrap_or_default(),
        );
        if let Some(html) = first_inner_html(
            cap.document,
            &["#content", ".job__description", "#app_body"],
        ) {
            set_description_html(job, &html);
        }
        if let Some(id) = greenhouse_id(cap.url.unwrap_or("")) {
            set_source_job_id(job, id);
        }
    }
}

fn greenhouse_id(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let mut segs = parsed.path_segments()?;
    while let Some(seg) = segs.next() {
        if seg == "jobs" {
            return segs.next().map(str::to_string).filter(|s| !s.is_empty());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{apply, CaptureView};
    use crate::html;
    use jobseeker_core::provenance::Provenance;

    const HTML: &str = r#"<html><body>
<div id="header">
  <h1 class="app-title">Platform Engineer</h1>
  <span class="company-name">Acme Robotics</span>
  <div class="location">Remote - US</div>
</div>
<div id="content">
  <h2>Requirements</h2>
  <ul><li>5+ years Rust</li><li>Kubernetes</li></ul>
</div>
</body></html>"#;

    #[test]
    fn html_fallback_without_json_ld() {
        let doc = html::parse(HTML);
        let mut job = ExtractedJob::default();
        apply(
            &mut job,
            &CaptureView {
                url: Some("https://boards.greenhouse.io/acmerobotics/jobs/5512034"),
                html: HTML,
                document: &doc,
                page_meta: None,
            },
        );
        assert_eq!(job.title.as_ref().unwrap().value, "Platform Engineer");
        assert_eq!(job.title.as_ref().unwrap().provenance, Provenance::Adapter);
        assert_eq!(job.company_name.as_ref().unwrap().value, "Acme Robotics");
        assert_eq!(job.source_job_id.as_ref().unwrap().value, "5512034");
        assert!(job
            .description_md
            .as_ref()
            .is_some_and(|d| d.value.contains("Kubernetes")));
    }
}
