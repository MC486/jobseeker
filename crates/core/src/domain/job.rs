//! The canonical job posting, and the intermediate shape extraction produces.

use serde::{Deserialize, Serialize};

use crate::domain::enums::*;
use crate::domain::location::RawLocation;
use crate::domain::salary::{RawSalary, Salary};
use crate::ids::{CompanyId, JobId, ListingId};
use crate::provenance::Sourced;
use crate::time::{PartialDate, Timestamp};

/// One real posting, deduplicated across every site that cross-posted it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub company_id: CompanyId,
    pub slug: String,
    pub title: String,
    pub title_normalized: String,

    pub seniority: Seniority,
    pub employment_type: EmploymentType,
    pub work_mode: WorkMode,
    pub work_mode_detail: Option<String>,
    pub department: Option<String>,
    pub team: Option<String>,

    /// Normalized Markdown with navigation and legal boilerplate removed.
    pub description_md: String,
    /// Plain-text projection, used for full-text search and embeddings.
    pub description_text: String,
    pub summary: Option<String>,
    pub responsibilities_md: Option<String>,
    pub benefits_md: Option<String>,

    pub salary: Salary,
    pub equity_offered: bool,
    pub comp_notes: Option<String>,

    pub posted_at: Option<PartialDate>,
    pub closes_at: Option<PartialDate>,
    pub first_seen_at: Timestamp,
    pub last_seen_at: Timestamp,
    pub closed_at: Option<Timestamp>,
    pub status: JobStatus,

    pub apply_url: Option<String>,
    pub apply_kind: ApplyKind,
    pub canonical_listing_id: Option<ListingId>,

    /// Clearance *type* the posting names (`secret`, `ts_sci`). Not the same as
    /// "must hold it on day one" — see `clearance_required_to_start`.
    pub requires_clearance: Option<String>,
    /// `Some(true)` = must already hold it to start. `Some(false)` = type is
    /// stated but not required to start (ability to obtain is the bar).
    pub clearance_required_to_start: Option<bool>,
    pub visa_sponsorship: Tristate,
    pub travel_pct: Option<i32>,
    pub education_min: EducationLevel,
    pub years_experience_min: Option<f32>,
    pub years_experience_max: Option<f32>,

    /// BLAKE3 over the normalized record; drives change detection and score staleness.
    pub content_hash: String,
    pub extraction_model: Option<String>,
    pub extracted_at: Option<Timestamp>,
    pub extraction_confidence: Option<f32>,
    /// True when the model stage was skipped or failed, so deterministic stages alone
    /// produced this record.
    pub extraction_partial: bool,

    /// Data-dir-relative directory holding this job's files.
    pub file_path: Option<String>,
    pub user_rating: Option<i32>,
    pub user_notes_md: Option<String>,
    pub is_archived: bool,

    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Job {
    /// Days until applications close, when the posting says.
    pub fn days_until_close(&self, now: Timestamp) -> Option<i64> {
        self.closes_at.map(|d| (d.at - now).num_days())
    }

    /// Closing within `days` and still open — the thing worth nagging about.
    pub fn is_closing_soon(&self, now: Timestamp, days: i64) -> bool {
        self.status == JobStatus::Open
            && self
                .days_until_close(now)
                .is_some_and(|d| (0..=days).contains(&d))
    }

    pub fn is_actionable(&self) -> bool {
        self.status == JobStatus::Open && !self.is_archived
    }
}

/// FR-T-04: close date within this many days is "closing soon".
pub const CLOSING_SOON_DAYS: i64 = 7;

/// Open posting, close date in `[now, now+days]`, and not yet applied.
///
/// "No application" here means no row, or still `interested` / `preparing`.
/// Applied-or-later (including rejected / withdrawn / ghosted) is not nagged.
pub fn is_closing_soon_unapplied(
    job_status: &str,
    closes_at: Option<&str>,
    application_status: Option<&str>,
) -> bool {
    is_closing_soon_unapplied_within(
        job_status,
        closes_at,
        application_status,
        crate::time::now(),
        CLOSING_SOON_DAYS,
    )
}

pub fn is_closing_soon_unapplied_within(
    job_status: &str,
    closes_at: Option<&str>,
    application_status: Option<&str>,
    now: crate::time::Timestamp,
    days: i64,
) -> bool {
    if job_status != JobStatus::Open.as_str() {
        return false;
    }
    if !still_unapplied(application_status) {
        return false;
    }
    let Some(closes) = closes_at.and_then(|s| crate::time::parse_rfc3339(s).ok()) else {
        return false;
    };
    let window_end = now + chrono::Duration::days(days);
    closes >= now && closes <= window_end
}

fn still_unapplied(application_status: Option<&str>) -> bool {
    matches!(application_status, None | Some("interested" | "preparing"))
}

/// What extraction produces: every field optional and provenance-tagged, so nothing is ever
/// guessed into a non-null value (FR-E-04).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractedJob {
    pub title: Option<Sourced<String>>,
    pub company_name: Option<Sourced<String>>,
    pub description_html: Option<Sourced<String>>,
    pub description_md: Option<Sourced<String>>,
    pub summary: Option<Sourced<String>>,
    pub responsibilities_md: Option<Sourced<String>>,
    pub benefits_md: Option<Sourced<String>>,

    pub locations: Vec<Sourced<RawLocation>>,
    pub work_mode: Option<Sourced<WorkMode>>,
    pub work_mode_detail: Option<Sourced<String>>,
    pub employment_type: Option<Sourced<EmploymentType>>,
    pub seniority: Option<Sourced<Seniority>>,
    pub department: Option<Sourced<String>>,

    pub salary: Option<Sourced<RawSalary>>,
    pub posted_at: Option<Sourced<PartialDate>>,
    pub closes_at: Option<Sourced<PartialDate>>,

    pub apply_url: Option<Sourced<String>>,
    pub apply_kind: Option<Sourced<ApplyKind>>,
    pub source_job_id: Option<Sourced<String>>,

    pub requires_clearance: Option<Sourced<String>>,
    pub clearance_required_to_start: Option<Sourced<bool>>,
    pub visa_sponsorship: Option<Sourced<Tristate>>,
    pub travel_pct: Option<Sourced<i32>>,
    pub education_min: Option<Sourced<EducationLevel>>,
    pub years_experience_min: Option<Sourced<f32>>,

    /// Anything a source gave us that has no column yet. Never discarded — a field we do not
    /// model today may be exactly what a future adapter needs.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub raw_fields: serde_json::Map<String, serde_json::Value>,
}

impl ExtractedJob {
    /// Fields that must be present for the record to be worth persisting as a job.
    pub fn has_minimum_viable_fields(&self) -> bool {
        self.title.is_some()
            && self.company_name.is_some()
            && (self.description_md.is_some() || self.description_html.is_some())
    }

    /// Which of the required fields are still missing — this is what the LLM stage is asked
    /// about, so that a mostly-extracted posting costs almost no tokens.
    pub fn missing_fields(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.title.is_none() {
            missing.push("title");
        }
        if self.company_name.is_none() {
            missing.push("company_name");
        }
        if self.description_md.is_none() && self.description_html.is_none() {
            missing.push("description");
        }
        if self.locations.is_empty()
            && self.work_mode.as_ref().map(|w| w.value) != Some(WorkMode::Remote)
        {
            missing.push("locations");
        }
        if self.work_mode.is_none() {
            missing.push("work_mode");
        }
        if self.employment_type.is_none() {
            missing.push("employment_type");
        }
        if self.salary.is_none() {
            missing.push("salary");
        }
        if self.posted_at.is_none() {
            missing.push("posted_at");
        }
        if self.apply_url.is_none() {
            missing.push("apply_url");
        }
        missing
    }

    /// Mean confidence across the fields we actually got, for the UI's quality badge.
    pub fn mean_confidence(&self) -> Option<f32> {
        let mut sum = 0.0;
        let mut n = 0u32;
        macro_rules! account {
            ($($field:ident),+) => { $(
                if let Some(s) = &self.$field { sum += s.confidence.get(); n += 1; }
            )+ };
        }
        account!(
            title,
            company_name,
            description_md,
            work_mode,
            employment_type,
            seniority,
            salary,
            posted_at,
            apply_url
        );
        for l in &self.locations {
            sum += l.confidence.get();
            n += 1;
        }
        (n > 0).then(|| sum / n as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::Provenance;

    #[test]
    fn minimum_viable_requires_title_company_and_description() {
        let mut e = ExtractedJob::default();
        assert!(!e.has_minimum_viable_fields());
        e.title = Some(Sourced::new("Engineer".into(), Provenance::Jsonld));
        e.company_name = Some(Sourced::new("Acme".into(), Provenance::Jsonld));
        assert!(
            !e.has_minimum_viable_fields(),
            "description is still missing"
        );
        e.description_md = Some(Sourced::new("# Role".into(), Provenance::Jsonld));
        assert!(e.has_minimum_viable_fields());
    }

    #[test]
    fn missing_fields_shrink_as_stages_fill_them() {
        let mut e = ExtractedJob::default();
        let before = e.missing_fields().len();
        e.title = Some(Sourced::new("Engineer".into(), Provenance::Jsonld));
        e.salary = Some(Sourced::new(
            RawSalary {
                text: "$1".into(),
                country_hint: None,
                source_kind: None,
            },
            Provenance::Jsonld,
        ));
        let after = e.missing_fields();
        assert_eq!(after.len(), before - 2);
        assert!(!after.contains(&"title"));
        assert!(!after.contains(&"salary"));
    }

    #[test]
    fn a_fully_remote_job_does_not_need_a_location() {
        let mut e = ExtractedJob::default();
        e.work_mode = Some(Sourced::new(WorkMode::Remote, Provenance::Jsonld));
        assert!(!e.missing_fields().contains(&"locations"));
    }

    #[test]
    fn closing_soon_unapplied_only_for_open_unapplied_in_window() {
        let now = crate::time::parse_rfc3339("2026-09-13T12:00:00Z").unwrap();
        let soon = Some("2026-09-16T12:00:00Z");
        let later = Some("2026-10-13T12:00:00Z");
        let past = Some("2026-09-01T12:00:00Z");
        assert!(is_closing_soon_unapplied_within("open", soon, None, now, 7));
        assert!(is_closing_soon_unapplied_within(
            "open",
            soon,
            Some("interested"),
            now,
            7
        ));
        assert!(is_closing_soon_unapplied_within(
            "open",
            soon,
            Some("preparing"),
            now,
            7
        ));
        assert!(!is_closing_soon_unapplied_within(
            "open",
            soon,
            Some("applied"),
            now,
            7
        ));
        assert!(!is_closing_soon_unapplied_within(
            "open",
            soon,
            Some("rejected"),
            now,
            7
        ));
        assert!(!is_closing_soon_unapplied_within(
            "closed", soon, None, now, 7
        ));
        assert!(!is_closing_soon_unapplied_within(
            "open", later, None, now, 7
        ));
        assert!(!is_closing_soon_unapplied_within(
            "open", past, None, now, 7
        ));
        assert!(!is_closing_soon_unapplied_within(
            "open", None, None, now, 7
        ));
    }

    #[test]
    fn mean_confidence_is_none_when_nothing_was_extracted() {
        assert_eq!(ExtractedJob::default().mean_confidence(), None);
        let mut e = ExtractedJob::default();
        e.title = Some(Sourced::with_confidence("t".into(), Provenance::Llm, 0.5));
        e.company_name = Some(Sourced::with_confidence("c".into(), Provenance::Api, 1.0));
        assert_eq!(e.mean_confidence(), Some(0.75));
    }
}
