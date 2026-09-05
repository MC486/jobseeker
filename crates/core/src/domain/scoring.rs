//! Match scores.
//!
//! The product rule is that a score is never a single opaque number: it is a weighted sum of
//! named subscores, each traceable to a requirement and a piece of evidence. These types
//! encode that so an unexplainable score is not representable.

use serde::{Deserialize, Serialize};

use crate::config::Weights;
use crate::ids::{AccomplishmentId, JobId, MatchScoreId, ProfileId, RequirementId, SkillId};
use crate::time::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictStatus {
    Met,
    Partial,
    Gap,
    /// We could not tell. Excluded from denominators — never silently counted as a pass or a
    /// fail.
    Unknown,
}

impl VerdictStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            VerdictStatus::Met => "met",
            VerdictStatus::Partial => "partial",
            VerdictStatus::Gap => "gap",
            VerdictStatus::Unknown => "unknown",
        }
    }

    pub fn counts_toward_coverage(self) -> bool {
        self != VerdictStatus::Unknown
    }
}

impl std::str::FromStr for VerdictStatus {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        Ok(match s {
            "met" => VerdictStatus::Met,
            "partial" => VerdictStatus::Partial,
            "gap" => VerdictStatus::Gap,
            "unknown" => VerdictStatus::Unknown,
            other => {
                return Err(crate::Error::BadRequest(format!(
                    "unknown verdict: {other}"
                )))
            }
        })
    }
}

/// Why a requirement got its verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub accomplishment_id: Option<AccomplishmentId>,
    pub skill_id: Option<SkillId>,
    pub similarity: Option<f32>,
    /// Snippet shown in the UI so the user sees the actual claim, not just an id.
    pub excerpt: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Accomplishment,
    ProfileSkill,
    /// Credit inherited up the taxonomy (React evidence for a JavaScript requirement).
    SkillHierarchy,
    Semantic,
    Tenure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementMatch {
    pub requirement_id: RequirementId,
    pub status: VerdictStatus,
    /// `0.0..=1.0` — partial credit within the verdict.
    pub score: f32,
    pub weight: f32,
    pub evidence: Vec<Evidence>,
    pub years_have: Option<f32>,
    pub years_needed: Option<f32>,
    /// One human-readable line explaining the verdict.
    pub rationale: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Subscores {
    pub required_coverage: Option<f32>,
    pub preferred_coverage: Option<f32>,
    pub semantic_similarity: Option<f32>,
    pub seniority_fit: Option<f32>,
    pub comp_fit: Option<f32>,
    pub location_fit: Option<f32>,
}

impl Subscores {
    /// Weighted mean over the subscores that are actually available, renormalizing so a
    /// missing component (no embeddings, no salary) neither helps nor hurts.
    pub fn weighted(&self, w: &Weights) -> f32 {
        let pairs = [
            (self.required_coverage, w.required_coverage),
            (self.preferred_coverage, w.preferred_coverage),
            (self.semantic_similarity, w.semantic),
            (self.seniority_fit, w.seniority),
            (self.comp_fit, w.comp),
            (self.location_fit, w.location),
        ];
        let mut num = 0.0;
        let mut den = 0.0;
        for (value, weight) in pairs {
            if let Some(v) = value {
                num += v.clamp(0.0, 1.0) * weight;
                den += weight;
            }
        }
        if den <= 0.0 {
            0.0
        } else {
            (num / den).clamp(0.0, 1.0)
        }
    }

    pub fn available_count(&self) -> usize {
        [
            self.required_coverage,
            self.preferred_coverage,
            self.semantic_similarity,
            self.seniority_fit,
            self.comp_fit,
            self.location_fit,
        ]
        .iter()
        .filter(|v| v.is_some())
        .count()
    }
}

/// A hard disqualifier, reported separately from graded gaps because it is categorically
/// different: no amount of skill overlap fixes a missing clearance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blocker {
    pub requirement_id: Option<RequirementId>,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchScore {
    pub id: MatchScoreId,
    pub job_id: JobId,
    pub profile_id: ProfileId,
    pub algorithm_version: String,
    pub overall: f32,
    pub subscores: Subscores,
    pub blockers: Vec<Blocker>,
    pub weights_used: Weights,
    pub requirement_matches: Vec<RequirementMatch>,
    /// Optional model-written prose. The numbers never come from a model.
    pub narrative: Option<String>,
    pub flags: Vec<String>,
    /// Hash of (job content, profile revision, weights, algorithm version). Changing any
    /// input marks the score stale rather than deleting it, so the UI can show the last
    /// known value while recomputing.
    pub inputs_hash: String,
    pub is_stale: bool,
    pub computed_at: Timestamp,
    pub duration_ms: i64,
}

impl MatchScore {
    /// Apply the blocker cap. Kept as an explicit, testable step because it is a product
    /// decision that users will (rightly) ask about.
    pub fn capped_overall(raw: f32, blocker_count: usize, cap: f32) -> f32 {
        if blocker_count > 0 {
            raw.min(cap)
        } else {
            raw
        }
    }

    pub fn counts(&self) -> VerdictCounts {
        let mut counts = VerdictCounts::default();
        for m in &self.requirement_matches {
            match m.status {
                VerdictStatus::Met => counts.met += 1,
                VerdictStatus::Partial => counts.partial += 1,
                VerdictStatus::Gap => counts.gap += 1,
                VerdictStatus::Unknown => counts.unknown += 1,
            }
        }
        counts
    }

    /// A blunt, honest label. Requirement inflation is real, so a 0.7 is described as worth
    /// applying to rather than as a failure.
    pub fn verdict_label(&self) -> &'static str {
        if !self.blockers.is_empty() {
            return "blocked";
        }
        match self.overall {
            s if s >= 0.85 => "strong fit",
            s if s >= 0.70 => "good fit with gaps",
            s if s >= 0.55 => "stretch — worth applying",
            s if s >= 0.40 => "significant gaps",
            _ => "poor fit",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct VerdictCounts {
    pub met: u32,
    pub partial: u32,
    pub gap: u32,
    pub unknown: u32,
}

/// An actionable gap, aggregated across the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillGap {
    pub skill_id: SkillId,
    pub skill_name: String,
    /// How many saved jobs demand it.
    pub frequency: i64,
    /// How many demand it as *required* — the count that actually blocks you.
    pub blocking_count: i64,
    pub years_short: Option<f32>,
    /// Expected total score improvement across the pipeline if this gap were closed,
    /// weighted by how much you want each job. Usually a better guide than raw frequency.
    pub leverage: f32,
    pub unlocks_job_ids: Vec<JobId>,
    pub suggested_action: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subscores_all(v: f32) -> Subscores {
        Subscores {
            required_coverage: Some(v),
            preferred_coverage: Some(v),
            semantic_similarity: Some(v),
            seniority_fit: Some(v),
            comp_fit: Some(v),
            location_fit: Some(v),
        }
    }

    #[test]
    fn uniform_subscores_yield_that_value() {
        let w = Weights::default();
        assert!((subscores_all(0.8).weighted(&w) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn missing_subscores_are_renormalized_not_treated_as_zero() {
        let w = Weights::default();
        let mut s = subscores_all(0.8);
        s.semantic_similarity = None;
        s.comp_fit = None;
        assert!(
            (s.weighted(&w) - 0.8).abs() < 1e-6,
            "an unavailable component must not drag the score down"
        );
        assert_eq!(s.available_count(), 4);
    }

    #[test]
    fn weighting_follows_the_configured_weights() {
        let w = Weights {
            required_coverage: 1.0,
            preferred_coverage: 0.0,
            semantic: 0.0,
            seniority: 0.0,
            comp: 0.0,
            location: 0.0,
        };
        let s = Subscores {
            required_coverage: Some(0.5),
            preferred_coverage: Some(1.0),
            ..Default::default()
        };
        assert!((s.weighted(&w) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn no_available_subscores_is_zero_not_a_panic() {
        assert_eq!(Subscores::default().weighted(&Weights::default()), 0.0);
    }

    #[test]
    fn blockers_cap_the_overall_score() {
        assert_eq!(MatchScore::capped_overall(0.95, 1, 0.45), 0.45);
        assert_eq!(MatchScore::capped_overall(0.95, 0, 0.45), 0.95);
        assert_eq!(
            MatchScore::capped_overall(0.30, 2, 0.45),
            0.30,
            "the cap is a ceiling, not a floor"
        );
    }

    #[test]
    fn unknown_verdicts_are_excluded_from_coverage() {
        assert!(!VerdictStatus::Unknown.counts_toward_coverage());
        assert!(VerdictStatus::Gap.counts_toward_coverage());
        assert!(VerdictStatus::Met.counts_toward_coverage());
    }
}
