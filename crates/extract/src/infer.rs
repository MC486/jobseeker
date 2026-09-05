//! The weakest stage: guesses from other fields (`Provenance::Inferred`).
//!
//! Used only when nothing stated the field outright. Each inference returns `None` rather
//! than a default, so "we do not know" stays distinguishable from "the posting said so".

use jobseeker_core::domain::job::ExtractedJob;
use jobseeker_core::provenance::{merge_field, Provenance, Sourced};
use jobseeker_normalize::seniority::{infer_employment_type, infer_seniority, infer_work_mode};

pub fn apply(job: &mut ExtractedJob) {
    if job.seniority.is_none() {
        if let Some(title) = job.title.as_ref() {
            if let Some(level) = infer_seniority(&title.value) {
                merge_field(&mut job.seniority, Sourced::new(level, Provenance::Inferred));
            }
        }
    }
    if job.work_mode.is_none() {
        let mut blob = String::new();
        if let Some(title) = &job.title {
            blob.push_str(&title.value);
            blob.push(' ');
        }
        for loc in &job.locations {
            blob.push_str(&loc.value.text);
            blob.push(' ');
        }
        if let Some(mode) = infer_work_mode(&blob) {
            merge_field(&mut job.work_mode, Sourced::new(mode, Provenance::Inferred));
        }
    }
    if job.employment_type.is_none() {
        if let Some(title) = job.title.as_ref() {
            if let Some(kind) = infer_employment_type(&title.value) {
                merge_field(&mut job.employment_type, Sourced::new(kind, Provenance::Inferred));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobseeker_core::domain::enums::Seniority;
    use jobseeker_core::domain::location::RawLocation;

    #[test]
    fn seniority_is_inferred_from_the_title_only_when_unset() {
        let mut job = ExtractedJob::default();
        job.title = Some(Sourced::new("Staff Engineer".into(), Provenance::Jsonld));
        apply(&mut job);
        assert_eq!(job.seniority.as_ref().unwrap().value, Seniority::Staff);
        assert_eq!(job.seniority.as_ref().unwrap().provenance, Provenance::Inferred);

        job.seniority = Some(Sourced::new(Seniority::Senior, Provenance::Jsonld));
        apply(&mut job);
        assert_eq!(
            job.seniority.unwrap().value,
            Seniority::Senior,
            "a stated level must not be overwritten by the title"
        );
    }

    #[test]
    fn a_remote_location_string_implies_remote_work() {
        let mut job = ExtractedJob::default();
        job.locations
            .push(Sourced::new(RawLocation::new("Remote - US"), Provenance::Jsonld));
        apply(&mut job);
        assert_eq!(
            job.work_mode.unwrap().value,
            jobseeker_core::domain::enums::WorkMode::Remote
        );
    }
}
