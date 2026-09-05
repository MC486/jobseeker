//! Atomized requirements and the skill taxonomy.
//!
//! Breaking a prose posting into typed, individually addressable requirements is the
//! transform the rest of the product is built on: matching, gap analysis and resume
//! targeting are all set operations over these rows.

use serde::{Deserialize, Serialize};

use crate::domain::enums::{EducationLevel, Necessity, RequirementKind, SkillLevel};
use crate::ids::{JobId, RequirementId, SkillId};
use crate::provenance::{Confidence, Provenance};
use crate::time::Timestamp;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Requirement {
    pub id: RequirementId,
    pub job_id: JobId,
    pub ordinal: i64,

    /// Verbatim from the posting, so the UI can quote it and the user can judge it.
    pub text: String,
    /// Lowercased, stopworded comparison key used for in-job dedup.
    pub normalized_text: String,

    pub kind: RequirementKind,
    pub necessity: Necessity,
    pub skill_id: Option<SkillId>,

    pub min_years: Option<f32>,
    pub max_years: Option<f32>,
    pub level: Option<SkillLevel>,
    pub education_level: Option<EducationLevel>,
    pub field_of_study: Option<String>,

    /// A categorical disqualifier (clearance, licence, work authorization) rather than a
    /// graded gap. Caps the overall score — see `docs/07-matching.md`.
    pub is_blocker: bool,
    /// The quantity phrase as written: `"5+ years"`, `"at least two"`.
    pub quantity_raw: Option<String>,
    /// Character offsets into `job.description_md`, so the UI can highlight the origin.
    pub source_span: Option<(usize, usize)>,

    pub confidence: Confidence,
    pub provenance: Provenance,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Requirement {
    /// Weight in `required_coverage`: the kind's base weight, nudged up for requirements
    /// that demand more years (a 10-year demand matters more than a 1-year one).
    pub fn weight(&self) -> f32 {
        let base = self.kind.base_weight();
        let years = self.min_years.unwrap_or(0.0).clamp(0.0, 10.0);
        base * (1.0 + 0.15 * years / 10.0)
    }

    pub fn is_required(&self) -> bool {
        matches!(self.necessity, Necessity::Required | Necessity::Implied)
    }

    /// A short label for dense tables: `"Kubernetes (3+ yrs)"`.
    pub fn label(&self) -> String {
        match (self.min_years, self.max_years) {
            (Some(min), Some(max)) if (max - min).abs() > f32::EPSILON => {
                format!("{} ({:.0}–{:.0} yrs)", self.text, min, max)
            }
            (Some(min), _) => format!("{} ({:.0}+ yrs)", self.text, min),
            _ => self.text.clone(),
        }
    }
}

/// A taxonomy node. The hierarchy is what lets React experience give partial credit for a
/// JavaScript requirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: SkillId,
    pub name: String,
    pub slug: String,
    pub kind: SkillKind,
    pub parent_id: Option<SkillId>,
    pub description: Option<String>,
    /// Denormalized count of requirements referencing this skill; refreshed by a task.
    pub usage_count: i64,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillKind {
    Language,
    Framework,
    Library,
    Tool,
    Platform,
    Database,
    Concept,
    Domain,
    Methodology,
    Soft,
    Certification,
    /// A human language, as opposed to a programming language.
    LanguageHuman,
}

impl SkillKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SkillKind::Language => "language",
            SkillKind::Framework => "framework",
            SkillKind::Library => "library",
            SkillKind::Tool => "tool",
            SkillKind::Platform => "platform",
            SkillKind::Database => "database",
            SkillKind::Concept => "concept",
            SkillKind::Domain => "domain",
            SkillKind::Methodology => "methodology",
            SkillKind::Soft => "soft",
            SkillKind::Certification => "certification",
            SkillKind::LanguageHuman => "language_human",
        }
    }
}

impl std::str::FromStr for SkillKind {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        Ok(match s {
            "language" => SkillKind::Language,
            "framework" => SkillKind::Framework,
            "library" => SkillKind::Library,
            "tool" => SkillKind::Tool,
            "platform" => SkillKind::Platform,
            "database" => SkillKind::Database,
            "concept" => SkillKind::Concept,
            "domain" => SkillKind::Domain,
            "methodology" => SkillKind::Methodology,
            "soft" => SkillKind::Soft,
            "certification" => SkillKind::Certification,
            "language_human" => SkillKind::LanguageHuman,
            other => return Err(crate::Error::BadRequest(format!("unknown skill kind: {other}"))),
        })
    }
}

/// Evidence propagates *up* the taxonomy with this decay per hop, never down: React implies
/// some JavaScript, but JavaScript does not imply React.
pub const HIERARCHY_DECAY_PER_HOP: f32 = 0.7;

/// An unrecognized skill phrase awaiting promotion into the taxonomy. Unknown skills are
/// queued for review rather than dropped, so the taxonomy grows to fit the user's market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCandidate {
    pub normalized_text: String,
    pub occurrences: i64,
    pub example_requirement_id: Option<RequirementId>,
    pub first_seen_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::now;

    fn req(kind: RequirementKind, necessity: Necessity, min_years: Option<f32>) -> Requirement {
        Requirement {
            id: RequirementId::new(),
            job_id: JobId::new(),
            ordinal: 0,
            text: "Kubernetes".into(),
            normalized_text: "kubernetes".into(),
            kind,
            necessity,
            skill_id: None,
            min_years,
            max_years: None,
            level: None,
            education_level: None,
            field_of_study: None,
            is_blocker: false,
            quantity_raw: None,
            source_span: None,
            confidence: Confidence::CERTAIN,
            provenance: Provenance::Llm,
            created_at: now(),
            updated_at: now(),
        }
    }

    #[test]
    fn longer_year_demands_weigh_more() {
        let one = req(RequirementKind::Skill, Necessity::Required, Some(1.0));
        let ten = req(RequirementKind::Skill, Necessity::Required, Some(10.0));
        assert!(ten.weight() > one.weight());
        // ...but not by a landslide: the kind still dominates.
        assert!(ten.weight() < one.weight() * 1.2);
    }

    #[test]
    fn soft_skills_weigh_far_less_than_hard_ones() {
        let soft = req(RequirementKind::SoftSkill, Necessity::Required, None);
        let hard = req(RequirementKind::Skill, Necessity::Required, None);
        assert!(soft.weight() < hard.weight() / 3.0);
    }

    #[test]
    fn implied_requirements_count_as_required() {
        assert!(req(RequirementKind::Skill, Necessity::Implied, None).is_required());
        assert!(!req(RequirementKind::Skill, Necessity::Preferred, None).is_required());
    }

    #[test]
    fn labels_render_year_ranges() {
        let mut r = req(RequirementKind::Skill, Necessity::Required, Some(3.0));
        assert_eq!(r.label(), "Kubernetes (3+ yrs)");
        r.max_years = Some(5.0);
        assert_eq!(r.label(), "Kubernetes (3–5 yrs)");
        let plain = req(RequirementKind::Skill, Necessity::Required, None);
        assert_eq!(plain.label(), "Kubernetes");
    }
}
