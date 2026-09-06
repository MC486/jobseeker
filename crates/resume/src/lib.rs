//! Experience-bank-grounded document generation.
//!
//! A resume is a *projection* of the experience bank onto one job. Selection is
//! deterministic. The model may rephrase; it may not invent. A bullet that introduces a
//! fact the original did not contain is rejected (`docs/08-resume.md`).

pub mod import;

use jobseeker_core::domain::profile::Accomplishment;
use jobseeker_core::domain::requirement::Requirement;
use jobseeker_normalize::skill::extract_all;
use jobseeker_normalize::text::{jaccard, tokenize};

/// One selected bullet, with why it was chosen.
#[derive(Debug, Clone)]
pub struct SelectedBullet {
    pub accomplishment: Accomplishment,
    pub text: String,
    pub covers: Vec<String>,
    pub quality: f32,
}

/// Pick the accomplishments that cover the most required skills, preferring quantified
/// and verified bullets, without exceeding `budget` items.
pub fn select(
    accomplishments: &[Accomplishment],
    requirements: &[Requirement],
    budget: usize,
) -> Vec<SelectedBullet> {
    let wanted: Vec<String> = requirements
        .iter()
        .filter(|r| r.is_required() && r.kind.is_scored())
        .map(|r| r.text.clone())
        .collect();

    let mut ranked: Vec<SelectedBullet> = accomplishments
        .iter()
        .map(|a| {
            let covers = coverage(&a.text, &wanted);
            SelectedBullet {
                quality: a.quality_score() + 0.15 * covers.len() as f32,
                text: a.text.clone(),
                covers,
                accomplishment: a.clone(),
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.quality
            .partial_cmp(&a.quality)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut chosen = Vec::new();
    let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
    for bullet in ranked {
        if chosen.len() >= budget {
            break;
        }
        let new_cover = bullet.covers.iter().any(|c| covered.insert(c.clone()));
        if new_cover || chosen.len() < budget / 2 {
            chosen.push(bullet);
        }
    }
    chosen
}

fn coverage(text: &str, wanted: &[String]) -> Vec<String> {
    let have = extract_all(text);
    wanted
        .iter()
        .filter(|w| {
            have.iter()
                .any(|h| w.to_lowercase().contains(&h.to_lowercase()))
                || jaccard(text, w) >= 0.25
        })
        .cloned()
        .collect()
}

/// Tokens in `proposed` that are not in `original` and are not stopwords. A non-empty
/// result means the model invented something.
pub fn invented_tokens(original: &str, proposed: &str) -> Vec<String> {
    let orig: std::collections::HashSet<String> = tokenize(original).into_iter().collect();
    tokenize(proposed)
        .into_iter()
        .filter(|t| {
            !orig.contains(t) && t.chars().any(|c| c.is_ascii_digit())
                || is_proper_invention(&orig, t)
        })
        .filter(|t| !orig.contains(t))
        .collect()
}

fn is_proper_invention(orig: &std::collections::HashSet<String>, token: &str) -> bool {
    // A new number or a new technology name is a fact. A synonym of an existing word is not.
    if orig.contains(token) {
        return false;
    }
    token.chars().any(|c| c.is_ascii_digit())
        || jobseeker_normalize::skill::resolve(token).is_some()
}

/// True when `proposed` does not introduce facts, metrics or tools that `original` lacked.
pub fn no_new_facts(original: &str, proposed: &str) -> bool {
    invented_tokens(original, proposed).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobseeker_core::domain::enums::{Necessity, RequirementKind};
    use jobseeker_core::ids::{AccomplishmentId, ExperienceId, JobId, RequirementId};
    use jobseeker_core::provenance::{Confidence, Provenance};
    use jobseeker_core::time::now;
    use std::collections::BTreeMap;

    fn acc(text: &str, strength: i32, metric: bool) -> Accomplishment {
        Accomplishment {
            id: AccomplishmentId::new(),
            experience_item_id: ExperienceId::new(),
            text: text.into(),
            variants: BTreeMap::new(),
            situation: None,
            action: None,
            result: None,
            impact_metric: metric.then(|| "latency".into()),
            impact_value: metric.then_some(85.0),
            impact_unit: metric.then(|| "%".into()),
            impact_direction: None,
            strength,
            verified: metric,
            evidence_url: None,
            evidence_note: None,
            scope: Default::default(),
            skill_ids: vec![],
            ordinal: 0,
            created_at: now(),
            updated_at: now(),
        }
    }

    fn req(text: &str) -> Requirement {
        Requirement {
            id: RequirementId::new(),
            job_id: JobId::new(),
            ordinal: 0,
            text: text.into(),
            normalized_text: text.to_lowercase(),
            kind: RequirementKind::Skill,
            necessity: Necessity::Required,
            skill_id: None,
            min_years: None,
            max_years: None,
            level: None,
            education_level: None,
            field_of_study: None,
            is_blocker: false,
            quantity_raw: None,
            source_span: None,
            confidence: Confidence::CERTAIN,
            provenance: Provenance::Manual,
            created_at: now(),
            updated_at: now(),
        }
    }

    #[test]
    fn selection_prefers_bullets_that_cover_required_skills() {
        let chosen = select(
            &[
                acc("Mentored three junior engineers", 5, false),
                acc(
                    "Rebuilt the ingestion pipeline in Rust, cutting p95 latency 85%",
                    4,
                    true,
                ),
            ],
            &[req("Production Rust")],
            3,
        );
        assert_eq!(
            chosen[0].text.contains("Rust"),
            true,
            "the covering bullet comes first"
        );
        assert!(!chosen[0].covers.is_empty());
    }

    #[test]
    fn rephrasing_without_new_facts_is_accepted() {
        let original = "Rebuilt the ingestion pipeline in Rust, cutting p95 latency 85%";
        assert!(no_new_facts(
            original,
            "Rebuilt ingestion in Rust and cut p95 latency by 85%"
        ));
    }

    #[test]
    fn inventing_a_tool_or_a_metric_is_rejected() {
        let original = "Rebuilt the ingestion pipeline in Rust";
        assert!(
            !no_new_facts(
                original,
                "Rebuilt the ingestion pipeline in Rust and Kubernetes"
            ),
            "Kubernetes was not in the original"
        );
        assert!(
            !no_new_facts(
                original,
                "Rebuilt the ingestion pipeline in Rust, cutting latency 85%"
            ),
            "85 is a new fact"
        );
    }

    #[test]
    fn an_empty_bank_selects_nothing() {
        assert!(select(&[], &[req("Rust")], 5).is_empty());
    }
}
