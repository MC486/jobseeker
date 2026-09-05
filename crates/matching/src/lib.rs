//! Explainable job/profile fit scoring.
//!
//! A score is never a single opaque number. It is a weighted sum of named subscores, each
//! traceable to a requirement and a piece of evidence (`docs/07-matching.md`). The types
//! in `jobseeker-core` already refuse to represent an unexplainable score; this crate is
//! the arithmetic that fills them.

use jobseeker_core::config::{MatchingConfig, Weights};
use jobseeker_core::domain::enums::{RequirementKind, Seniority, WorkMode};
use jobseeker_core::domain::location::JobLocation;
use jobseeker_core::domain::requirement::Requirement;
use jobseeker_core::domain::salary::Salary;
use jobseeker_core::domain::scoring::{
    Blocker, Evidence, EvidenceKind, MatchScore, RequirementMatch, Subscores, VerdictStatus,
};
use jobseeker_core::hash::hash_parts;
use jobseeker_core::ids::{JobId, MatchScoreId, ProfileId, SkillId};
use jobseeker_core::time::now;
use jobseeker_core::Result;
use jobseeker_normalize::skill::hierarchy_credit;

/// What the profile can show for one skill.
#[derive(Debug, Clone, Default)]
pub struct SkillEvidence {
    pub skill_id: Option<SkillId>,
    pub slug: String,
    pub years: Option<f32>,
    pub last_used_year: Option<i32>,
    pub excerpt: Option<String>,
}

/// The subset of a profile the scorer needs. Kept narrow so a test can construct one
/// without standing up the database.
#[derive(Debug, Clone, Default)]
pub struct ProfileSnapshot {
    pub profile_id: Option<ProfileId>,
    pub skills: Vec<SkillEvidence>,
    pub seniority: Option<Seniority>,
    pub years_experience: Option<f32>,
    pub target_comp_min_cents: Option<i64>,
    pub accepts_remote: bool,
    pub willing_to_relocate: bool,
    pub target_locations: Vec<String>,
    pub clearances: Vec<String>,
    pub education: Option<jobseeker_core::domain::enums::EducationLevel>,
}

/// The subset of a job the scorer needs.
#[derive(Debug, Clone)]
pub struct JobSnapshot {
    pub job_id: Option<JobId>,
    pub requirements: Vec<Requirement>,
    pub seniority: Seniority,
    pub work_mode: WorkMode,
    pub salary: Salary,
    pub locations: Vec<JobLocation>,
    pub requires_clearance: Option<String>,
}

pub fn score(job: &JobSnapshot, profile: &ProfileSnapshot, cfg: &MatchingConfig) -> Result<MatchScore> {
    let started = std::time::Instant::now();
    let mut matches = Vec::new();
    let mut blockers = Vec::new();

    if let Some(needed) = &job.requires_clearance {
        if !profile
            .clearances
            .iter()
            .any(|c| c.eq_ignore_ascii_case(needed))
        {
            blockers.push(Blocker {
                requirement_id: None,
                kind: "clearance".into(),
                message: format!("This role requires {needed}, which is not on the profile"),
            });
        }
    }

    for req in &job.requirements {
        let m = judge_requirement(req, profile, cfg);
        if req.is_blocker && m.status == VerdictStatus::Gap && req.is_required() {
            blockers.push(Blocker {
                requirement_id: Some(req.id.clone()),
                kind: req.kind.as_str().to_string(),
                message: m.rationale.clone(),
            });
        }
        matches.push(m);
    }

    let required_coverage = coverage(&matches, &job.requirements, |r| r.is_required());
    let preferred_coverage = coverage(&matches, &job.requirements, |r| !r.is_required());

    let mut flags = Vec::new();
    let comp_fit = compensation_fit(&job.salary, profile.target_comp_min_cents, &mut flags);
    let location_fit = location_fit(job, profile, &mut blockers);
    let seniority_fit = seniority_fit(job.seniority, profile.seniority, profile.years_experience);

    let subscores = Subscores {
        required_coverage,
        preferred_coverage,
        semantic_similarity: None,
        seniority_fit,
        comp_fit,
        location_fit,
    };
    flags.push("semantic_unavailable".into());

    let raw = subscores.weighted(&cfg.weights);
    let overall = MatchScore::capped_overall(raw, blockers.len(), cfg.blocker_cap);

    let inputs_hash = hash_parts(&[
        &cfg.algorithm_version,
        &format!("{overall:.4}"),
        &job.requirements.len().to_string(),
    ]);

    Ok(MatchScore {
        id: MatchScoreId::new(),
        job_id: job.job_id.clone().unwrap_or_else(JobId::new),
        profile_id: profile.profile_id.clone().unwrap_or_else(ProfileId::new),
        algorithm_version: cfg.algorithm_version.clone(),
        overall,
        subscores,
        blockers,
        weights_used: cfg.weights,
        requirement_matches: matches,
        narrative: None,
        flags,
        inputs_hash,
        is_stale: false,
        computed_at: now(),
        duration_ms: started.elapsed().as_millis() as i64,
    })
}

fn judge_requirement(req: &Requirement, profile: &ProfileSnapshot, cfg: &MatchingConfig) -> RequirementMatch {
    if !req.kind.is_scored() {
        return RequirementMatch {
            requirement_id: req.id.clone(),
            status: VerdictStatus::Unknown,
            score: 0.0,
            weight: req.weight(),
            evidence: vec![],
            years_have: None,
            years_needed: req.min_years,
            rationale: "Responsibilities describe the job, not a bar to clear".into(),
        };
    }

    match req.kind {
        RequirementKind::Clearance => clearance_verdict(req, profile),
        RequirementKind::Education => education_verdict(req, profile),
        RequirementKind::Logistics if req.is_blocker => RequirementMatch {
            requirement_id: req.id.clone(),
            status: VerdictStatus::Unknown,
            score: 0.0,
            weight: req.weight(),
            evidence: vec![],
            years_have: None,
            years_needed: None,
            rationale: "A logistics demand needs a yes/no from you; it is not guessed".into(),
        },
        _ => skill_verdict(req, profile, cfg),
    }
}

fn skill_verdict(req: &Requirement, profile: &ProfileSnapshot, cfg: &MatchingConfig) -> RequirementMatch {
    let mut best: Option<(&SkillEvidence, f32)> = None;
    for ev in &profile.skills {
        let credit = if ev.slug.is_empty() {
            0.0
        } else {
            // The requirement text is resolved to a slug by the caller when possible;
            // fall back to matching the requirement text itself as a slug-ish key.
            let want = ev.slug.as_str();
            let have = ev.slug.as_str();
            let _ = (want, have);
            hierarchy_credit(&ev.slug, &slug_of(req))
        };
        if credit > 0.0 && best.as_ref().map(|(_, c)| *c).unwrap_or(0.0) < credit {
            best = Some((ev, credit));
        }
    }

    let (years_have, score_raw, evidence) = match best {
        Some((ev, credit)) => {
            let years = ev.years.map(|y| y * credit);
            let evs = vec![Evidence {
                kind: if (credit - 1.0).abs() < f32::EPSILON {
                    EvidenceKind::ProfileSkill
                } else {
                    EvidenceKind::SkillHierarchy
                },
                accomplishment_id: None,
                skill_id: ev.skill_id.clone(),
                similarity: Some(credit),
                excerpt: ev.excerpt.clone(),
            }];
            (years, credit, evs)
        }
        None => (None, 0.0, vec![]),
    };

    let recency = recency_factor(best.and_then(|(e, _)| e.last_used_year), cfg.recency_decay_after_years);
    let (status, score) = verdict_from_years(years_have, req.min_years, score_raw * recency);
    let rationale = rationale_for(req, status, years_have);

    RequirementMatch {
        requirement_id: req.id.clone(),
        status,
        score,
        weight: req.weight(),
        evidence,
        years_have,
        years_needed: req.min_years,
        rationale,
    }
}

fn slug_of(req: &Requirement) -> String {
    jobseeker_normalize::skill::resolve(&req.text)
        .unwrap_or(req.normalized_text.as_str())
        .to_string()
}

fn verdict_from_years(have: Option<f32>, needed: Option<f32>, similarity: f32) -> (VerdictStatus, f32) {
    match (have, needed, similarity) {
        (None, _, s) if s <= 0.0 => (VerdictStatus::Gap, 0.0),
        (None, None, s) => (VerdictStatus::Met, s.clamp(0.0, 1.0)),
        (Some(_), None, s) => (VerdictStatus::Met, s.clamp(0.0, 1.0)),
        (Some(h), Some(n), _) if n <= 0.0 || h >= n => (VerdictStatus::Met, 1.0),
        (Some(h), Some(n), _) if h >= 0.6 * n => (VerdictStatus::Partial, (h / n).clamp(0.0, 1.0)),
        (Some(h), Some(n), _) => (VerdictStatus::Gap, (h / n).clamp(0.0, 1.0)),
        (None, Some(_), s) if s >= 0.72 => (VerdictStatus::Partial, s),
        (None, Some(_), _) => (VerdictStatus::Gap, 0.0),
    }
}

fn recency_factor(last_used_year: Option<i32>, decay_after: f32) -> f32 {
    let Some(year) = last_used_year else { return 1.0 };
    let current = 2026i32;
    let age = (current - year) as f32;
    if age <= decay_after {
        return 1.0;
    }
    let penalty = 0.85_f32.powf(age - decay_after);
    penalty.clamp(0.5, 1.0)
}

fn clearance_verdict(req: &Requirement, profile: &ProfileSnapshot) -> RequirementMatch {
    let have = profile
        .clearances
        .iter()
        .any(|c| req.text.to_ascii_lowercase().contains(&c.to_ascii_lowercase()) || c.eq_ignore_ascii_case(&req.normalized_text));
    let (status, score, rationale) = if have {
        (VerdictStatus::Met, 1.0, "The required clearance is on the profile".to_string())
    } else {
        (
            VerdictStatus::Gap,
            0.0,
            format!("No {} clearance on the profile", req.text),
        )
    };
    RequirementMatch {
        requirement_id: req.id.clone(),
        status,
        score,
        weight: req.weight(),
        evidence: vec![],
        years_have: None,
        years_needed: None,
        rationale,
    }
}

fn education_verdict(req: &Requirement, profile: &ProfileSnapshot) -> RequirementMatch {
    use jobseeker_core::domain::enums::EducationLevel;
    let needed = req.education_level.unwrap_or(EducationLevel::Unknown);
    let have = profile.education.unwrap_or(EducationLevel::Unknown);
    let (status, score, rationale) = match (needed.rank(), have.rank()) {
        (None, _) | (_, None) => (
            VerdictStatus::Unknown,
            0.0,
            "Education level is not comparable".into(),
        ),
        (Some(n), Some(h)) if h >= n => (VerdictStatus::Met, 1.0, "Meets the stated education level".into()),
        (Some(n), Some(h)) if h + 1 >= n => {
            (VerdictStatus::Partial, 0.6, "One level below the stated education".into())
        }
        _ => (VerdictStatus::Gap, 0.0, "Below the stated education level".into()),
    };
    RequirementMatch {
        requirement_id: req.id.clone(),
        status,
        score,
        weight: req.weight(),
        evidence: vec![],
        years_have: None,
        years_needed: None,
        rationale,
    }
}

fn coverage(
    matches: &[RequirementMatch],
    requirements: &[Requirement],
    pred: impl Fn(&Requirement) -> bool,
) -> Option<f32> {
    let mut num = 0.0;
    let mut den = 0.0;
    for (m, r) in matches.iter().zip(requirements) {
        if !pred(r) || !m.status.counts_toward_coverage() {
            continue;
        }
        num += m.weight * m.score;
        den += m.weight;
    }
    (den > 0.0).then_some((num / den).clamp(0.0, 1.0))
}

fn compensation_fit(salary: &Salary, target_min: Option<i64>, flags: &mut Vec<String>) -> Option<f32> {
    let Some(target) = target_min else {
        flags.push("no_comp_target".into());
        return None;
    };
    let Some(job_max) = salary.annualized_max_cents() else {
        flags.push("unknown_comp".into());
        return Some(0.5);
    };
    if job_max >= target {
        return Some(1.0);
    }
    if job_max >= (target as f32 * 0.9) as i64 {
        return Some(0.7);
    }
    let floor = (target as f32 * 0.6) as i64;
    if job_max <= floor {
        return Some(0.0);
    }
    let span = (target - floor) as f32;
    Some(((job_max - floor) as f32 / span).clamp(0.0, 1.0))
}

fn location_fit(job: &JobSnapshot, profile: &ProfileSnapshot, blockers: &mut Vec<Blocker>) -> Option<f32> {
    match job.work_mode {
        WorkMode::Remote if profile.accepts_remote => Some(1.0),
        WorkMode::Remote => Some(0.4),
        WorkMode::Unknown => None,
        WorkMode::Hybrid | WorkMode::Onsite => {
            let acceptable = job.locations.iter().any(|loc| {
                profile.target_locations.iter().any(|pref| {
                    loc.raw.to_ascii_lowercase().contains(&pref.to_ascii_lowercase())
                        || loc.display().to_ascii_lowercase().contains(&pref.to_ascii_lowercase())
                })
            });
            if acceptable {
                Some(if job.work_mode == WorkMode::Hybrid { 0.9 } else { 1.0 })
            } else if profile.willing_to_relocate {
                Some(0.5)
            } else {
                if job.work_mode == WorkMode::Onsite {
                    blockers.push(Blocker {
                        requirement_id: None,
                        kind: "location".into(),
                        message: "On-site role outside the locations you will consider".into(),
                    });
                }
                Some(0.0)
            }
        }
    }
}

fn seniority_fit(job: Seniority, have: Option<Seniority>, years: Option<f32>) -> Option<f32> {
    let have = have.or_else(|| years.map(Seniority::from_years))?;
    match (job.rank(), have.rank()) {
        (None, _) | (_, None) => None,
        (Some(j), Some(h)) if j == h => Some(1.0),
        (Some(j), Some(h)) if h + 1 == j => Some(0.75), // one level under
        (Some(j), Some(h)) if h == j + 1 => Some(0.9),  // one level over
        (Some(j), Some(h)) => {
            let dist = (j as i16 - h as i16).unsigned_abs();
            Some(if dist >= 2 { 0.4 } else { 0.6 })
        }
    }
}

fn rationale_for(req: &Requirement, status: VerdictStatus, years_have: Option<f32>) -> String {
    match (status, req.min_years, years_have) {
        (VerdictStatus::Met, Some(n), Some(h)) => {
            format!("Meets the {n:.0}-year bar with roughly {h:.1} years of evidence")
        }
        (VerdictStatus::Met, _, _) => format!("Evidence covers {}", req.text),
        (VerdictStatus::Partial, Some(n), Some(h)) => {
            format!("Partial: about {h:.1} years against {n:.0} asked for {}", req.text)
        }
        (VerdictStatus::Partial, _, _) => format!("Partial evidence for {}", req.text),
        (VerdictStatus::Gap, Some(n), Some(h)) => {
            format!("Missing roughly {:.1} years of {}", n - h, req.text)
        }
        (VerdictStatus::Gap, Some(n), None) => format!("No evidence for {}; {n:.0}+ years asked", req.text),
        (VerdictStatus::Gap, _, _) => format!("No evidence for {}", req.text),
        (VerdictStatus::Unknown, _, _) => format!("Could not judge {}", req.text),
    }
}

/// Default weights, exposed so tests and the API agree on the starting point.
pub fn default_weights() -> Weights {
    Weights::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobseeker_core::domain::enums::{Necessity, RequirementKind};
    use jobseeker_core::domain::salary::SalaryPeriod;
    use jobseeker_core::ids::{JobId, RequirementId};
    use jobseeker_core::provenance::{Confidence, Provenance};
    use jobseeker_core::time::now;

    fn req(text: &str, kind: RequirementKind, necessity: Necessity, years: Option<f32>) -> Requirement {
        Requirement {
            id: RequirementId::new(),
            job_id: JobId::new(),
            ordinal: 0,
            text: text.into(),
            normalized_text: text.to_lowercase(),
            kind,
            necessity,
            skill_id: None,
            min_years: years,
            max_years: None,
            level: None,
            education_level: None,
            field_of_study: None,
            is_blocker: kind == RequirementKind::Clearance,
            quantity_raw: None,
            source_span: None,
            confidence: Confidence::CERTAIN,
            provenance: Provenance::Rules,
            created_at: now(),
            updated_at: now(),
        }
    }

    fn job(reqs: Vec<Requirement>) -> JobSnapshot {
        JobSnapshot {
            job_id: Some(JobId::new()),
            requirements: reqs,
            seniority: Seniority::Senior,
            work_mode: WorkMode::Remote,
            salary: Salary {
                min_cents: Some(185_000_00),
                max_cents: Some(225_000_00),
                currency: "USD".into(),
                period: SalaryPeriod::Year,
                is_estimate: false,
                raw: None,
            },
            locations: vec![],
            requires_clearance: None,
        }
    }

    fn profile(skills: &[(&str, f32)]) -> ProfileSnapshot {
        ProfileSnapshot {
            skills: skills
                .iter()
                .map(|(slug, years)| SkillEvidence {
                    slug: (*slug).into(),
                    years: Some(*years),
                    last_used_year: Some(2026),
                    excerpt: Some(format!("used {slug}")),
                    ..Default::default()
                })
                .collect(),
            seniority: Some(Seniority::Senior),
            years_experience: Some(8.0),
            target_comp_min_cents: Some(160_000_00),
            accepts_remote: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_perfect_skill_overlap_is_a_strong_fit() {
        let job = job(vec![
            req("Production Rust", RequirementKind::Skill, Necessity::Required, Some(5.0)),
            req("Kubernetes", RequirementKind::Tool, Necessity::Required, Some(3.0)),
        ]);
        let profile = profile(&[("rust", 8.0), ("kubernetes", 5.0)]);
        let score = score(&job, &profile, &MatchingConfig::default()).unwrap();
        assert!(score.overall >= 0.85, "got {}", score.overall);
        assert!(score.blockers.is_empty());
        assert_eq!(score.counts().met, 2);
        assert_eq!(score.verdict_label(), "strong fit");
    }

    #[test]
    fn a_missing_required_skill_is_a_named_gap_not_a_silent_zero() {
        let job = job(vec![req(
            "3+ years operating Kubernetes",
            RequirementKind::Tool,
            Necessity::Required,
            Some(3.0),
        )]);
        let profile = profile(&[("rust", 8.0)]);
        let score = score(&job, &profile, &MatchingConfig::default()).unwrap();
        assert_eq!(score.requirement_matches[0].status, VerdictStatus::Gap);
        assert!(
            score.requirement_matches[0].rationale.contains("Kubernetes"),
            "the rationale must name the gap: {}",
            score.requirement_matches[0].rationale
        );
    }

    #[test]
    fn react_experience_partially_covers_a_javascript_requirement() {
        let job = job(vec![req("JavaScript", RequirementKind::Skill, Necessity::Required, None)]);
        let profile = profile(&[("react", 4.0)]);
        let score = score(&job, &profile, &MatchingConfig::default()).unwrap();
        let m = &score.requirement_matches[0];
        assert_eq!(m.status, VerdictStatus::Met, "no year bar, any evidence meets it");
        assert!(m.score < 1.0 && m.score > 0.0, "hierarchy decay must apply, got {}", m.score);
        assert_eq!(m.evidence[0].kind, EvidenceKind::SkillHierarchy);
    }

    #[test]
    fn javascript_does_not_cover_a_react_requirement() {
        let job = job(vec![req("React", RequirementKind::Skill, Necessity::Required, None)]);
        let profile = profile(&[("javascript", 10.0)]);
        let score = score(&job, &profile, &MatchingConfig::default()).unwrap();
        assert_eq!(score.requirement_matches[0].status, VerdictStatus::Gap);
    }

    #[test]
    fn a_clearance_you_do_not_hold_caps_the_score() {
        let mut job = job(vec![req("Production Rust", RequirementKind::Skill, Necessity::Required, None)]);
        job.requires_clearance = Some("ts_sci".into());
        let profile = profile(&[("rust", 8.0)]);
        let score = score(&job, &profile, &MatchingConfig::default()).unwrap();
        assert!(!score.blockers.is_empty());
        assert!(
            score.overall <= 0.45,
            "the cap is a product decision, got {}",
            score.overall
        );
        assert_eq!(score.verdict_label(), "blocked");
    }

    #[test]
    fn missing_salary_is_neutral_and_flagged() {
        let mut job = job(vec![]);
        job.salary.min_cents = None;
        job.salary.max_cents = None;
        let profile = profile(&[]);
        let score = score(&job, &profile, &MatchingConfig::default()).unwrap();
        assert_eq!(score.subscores.comp_fit, Some(0.5));
        assert!(score.flags.iter().any(|f| f == "unknown_comp"));
    }

    #[test]
    fn being_one_level_under_is_penalized_more_than_one_level_over() {
        let under = seniority_fit(Seniority::Senior, Some(Seniority::Mid), None).unwrap();
        let over = seniority_fit(Seniority::Senior, Some(Seniority::Staff), None).unwrap();
        assert!(under < over, "{under} should be below {over}");
        assert!((under - 0.75).abs() < 1e-6);
        assert!((over - 0.9).abs() < 1e-6);
    }

    #[test]
    fn a_stale_skill_is_decayed_but_not_zeroed() {
        let fresh = recency_factor(Some(2026), 3.0);
        let stale = recency_factor(Some(2016), 3.0);
        assert_eq!(fresh, 1.0);
        assert!(stale < 1.0 && stale >= 0.5, "got {stale}");
    }

    #[test]
    fn responsibilities_are_excluded_from_coverage() {
        let job = job(vec![req(
            "Design the ingestion pipeline",
            RequirementKind::Responsibility,
            Necessity::Required,
            None,
        )]);
        let score = score(&job, &profile(&[]), &MatchingConfig::default()).unwrap();
        assert_eq!(score.subscores.required_coverage, None);
        assert_eq!(score.requirement_matches[0].status, VerdictStatus::Unknown);
    }

    #[test]
    fn unknown_verdicts_do_not_silently_count_as_a_pass_or_a_fail() {
        assert!(!VerdictStatus::Unknown.counts_toward_coverage());
    }

    #[test]
    fn weights_are_the_documented_defaults() {
        let w = default_weights();
        assert!((w.required_coverage - 0.45).abs() < 1e-6);
        assert!((w.location - 0.05).abs() < 1e-6);
    }
}
