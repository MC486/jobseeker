//! The experience bank: your history, structured in the same vocabulary as the postings.
//!
//! A resume is a *projection* of this onto one job. Storing the projection instead of the
//! source is what forces you to redo the work for every application.

use serde::{Deserialize, Serialize};

use crate::domain::enums::{EmploymentType, SkillLevel, WorkMode};
use crate::ids::{AccomplishmentId, ExperienceId, ProfileId, SkillId};
use crate::time::Timestamp;

/// A persona. One experience bank can target "Staff Engineer" and "Engineering Manager"
/// differently without duplicating history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: ProfileId,
    pub name: String,
    pub full_name: Option<String>,
    pub headline: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub location: Option<String>,
    pub links: Vec<Link>,
    pub summary_md: Option<String>,
    pub target_titles: Vec<String>,
    pub target_comp_min_cents: Option<i64>,
    pub target_locations: Vec<String>,
    pub work_auth: Option<String>,
    /// `us` or `other`. Unset means we have not been told — matching must not invent it.
    pub citizenship: Option<String>,
    /// Normalized clearance the profile actually holds (`secret`, `ts_sci`), if any.
    pub clearance_held: Option<String>,
    /// Eligible to obtain (US citizen, no stated background issue). Distinct from holding.
    pub can_obtain_clearance: Option<bool>,
    pub willing_to_relocate: bool,
    pub accepts_remote: bool,
    pub is_default: bool,
    /// Bumped on any edit to the profile or its children; part of a match score's
    /// `inputs_hash`, so an edit invalidates exactly the affected scores.
    pub revision: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceKind {
    Role,
    Project,
    Education,
    Certification,
    Award,
    Publication,
    Oss,
    Volunteer,
    Course,
}

impl ExperienceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ExperienceKind::Role => "role",
            ExperienceKind::Project => "project",
            ExperienceKind::Education => "education",
            ExperienceKind::Certification => "certification",
            ExperienceKind::Award => "award",
            ExperienceKind::Publication => "publication",
            ExperienceKind::Oss => "oss",
            ExperienceKind::Volunteer => "volunteer",
            ExperienceKind::Course => "course",
        }
    }

    /// Kinds that contribute to years-of-experience arithmetic.
    pub fn counts_as_work(self) -> bool {
        matches!(
            self,
            ExperienceKind::Role | ExperienceKind::Oss | ExperienceKind::Project
        )
    }
}

impl std::str::FromStr for ExperienceKind {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        Ok(match s {
            "role" => ExperienceKind::Role,
            "project" => ExperienceKind::Project,
            "education" => ExperienceKind::Education,
            "certification" => ExperienceKind::Certification,
            "award" => ExperienceKind::Award,
            "publication" => ExperienceKind::Publication,
            "oss" => ExperienceKind::Oss,
            "volunteer" => ExperienceKind::Volunteer,
            "course" => ExperienceKind::Course,
            other => {
                return Err(crate::Error::BadRequest(format!(
                    "unknown experience kind: {other}"
                )))
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceItem {
    pub id: ExperienceId,
    pub profile_id: ProfileId,
    pub kind: ExperienceKind,
    pub org: String,
    pub org_normalized: String,
    pub title: Option<String>,
    pub location: Option<String>,
    pub work_mode: Option<WorkMode>,
    pub employment_type: Option<EmploymentType>,
    /// `YYYY-MM` or `YYYY-MM-DD`; resumes rarely have day precision.
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub is_current: bool,
    pub description_md: Option<String>,
    pub url: Option<String>,
    pub ordinal: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl ExperienceItem {
    /// Tenure in years, from `YYYY-MM`-ish dates. `None` when dates are unusable.
    pub fn duration_years(&self, today: &str) -> Option<f32> {
        let start = parse_year_month(self.start_date.as_deref()?)?;
        let end = if self.is_current {
            parse_year_month(today)?
        } else {
            parse_year_month(self.end_date.as_deref()?)?
        };
        let months = (end.0 - start.0) * 12 + (end.1 as i32 - start.1 as i32);
        (months >= 0).then(|| months as f32 / 12.0)
    }
}

fn parse_year_month(s: &str) -> Option<(i32, u32)> {
    let mut parts = s.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next().and_then(|m| m.parse().ok()).unwrap_or(1);
    (1..=12).contains(&month).then_some((year, month))
}

/// A bullet-level atom: the raw material for resume generation, match evidence, an
/// interview story, and a skill-years data point, all from one record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Accomplishment {
    pub id: AccomplishmentId,
    pub experience_item_id: ExperienceId,
    pub text: String,
    /// Named phrasings (`short`, `leadership`, `ic`) so tailoring is selection, not
    /// invention.
    pub variants: std::collections::BTreeMap<String, String>,
    pub situation: Option<String>,
    pub action: Option<String>,
    pub result: Option<String>,
    pub impact_metric: Option<String>,
    pub impact_value: Option<f64>,
    pub impact_unit: Option<String>,
    pub impact_direction: Option<ImpactDirection>,
    /// Your own 1–5 rating of how strong this bullet is.
    pub strength: i32,
    /// You can defend it with evidence.
    pub verified: bool,
    pub evidence_url: Option<String>,
    pub evidence_note: Option<String>,
    /// Team size, budget, users affected — used by scope-aware requirement matching.
    pub scope: serde_json::Map<String, serde_json::Value>,
    pub skill_ids: Vec<SkillId>,
    pub ordinal: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Accomplishment {
    /// A quantified bullet beats an unquantified one; the UI nudges toward filling these in.
    pub fn has_metric(&self) -> bool {
        self.impact_metric.is_some() && self.impact_value.is_some()
    }

    pub fn variant<'a>(&'a self, name: &str) -> &'a str {
        self.variants
            .get(name)
            .map(String::as_str)
            .unwrap_or(&self.text)
    }

    /// Selection preference: strong, quantified, verified bullets first. The components are
    /// budgeted to sum to exactly 1.0 so a maximally-rated bullet without a metric never
    /// ties one with a metric.
    pub fn quality_score(&self) -> f32 {
        let mut score = 0.7 * (self.strength.clamp(0, 5) as f32 / 5.0);
        if self.has_metric() {
            score += 0.2;
        }
        if self.verified {
            score += 0.1;
        }
        score.clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpactDirection {
    Increase,
    Decrease,
    Maintain,
}

/// A self-asserted skill. Recency matters: a skill last used in 2016 is not the same as one
/// used last year, and matching applies a decay for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSkill {
    pub profile_id: ProfileId,
    pub skill_id: SkillId,
    pub years: Option<f32>,
    pub level: Option<SkillLevel>,
    pub last_used_year: Option<i32>,
    pub is_primary: bool,
    pub self_rating: Option<i32>,
    /// Count of accomplishments tagged with this skill; derived, and the honest counterweight
    /// to self-assertion.
    pub evidence_count: i64,
    pub notes: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::now;
    use std::collections::BTreeMap;

    fn item(start: &str, end: Option<&str>, current: bool) -> ExperienceItem {
        ExperienceItem {
            id: ExperienceId::new(),
            profile_id: ProfileId::new(),
            kind: ExperienceKind::Role,
            org: "Acme".into(),
            org_normalized: "acme".into(),
            title: Some("Engineer".into()),
            location: None,
            work_mode: None,
            employment_type: None,
            start_date: Some(start.into()),
            end_date: end.map(Into::into),
            is_current: current,
            description_md: None,
            url: None,
            ordinal: 0,
            created_at: now(),
            updated_at: now(),
        }
    }

    #[test]
    fn tenure_is_computed_from_partial_dates() {
        assert_eq!(
            item("2021-03", Some("2024-08"), false).duration_years("2026-09"),
            Some(41.0 / 12.0)
        );
        assert_eq!(
            item("2024-09", None, true).duration_years("2026-09"),
            Some(2.0)
        );
        assert_eq!(
            item("2021", Some("2024"), false).duration_years("2026-09"),
            Some(3.0)
        );
    }

    #[test]
    fn unusable_dates_yield_none_rather_than_zero() {
        assert_eq!(item("2021-03", None, false).duration_years("2026-09"), None);
        assert_eq!(
            item("not-a-date", Some("2024-08"), false).duration_years("2026-09"),
            None
        );
        // An end before the start is data entry error, not negative tenure.
        assert_eq!(
            item("2024-08", Some("2021-03"), false).duration_years("2026-09"),
            None
        );
    }

    fn acc(strength: i32, metric: bool, verified: bool) -> Accomplishment {
        Accomplishment {
            id: AccomplishmentId::new(),
            experience_item_id: ExperienceId::new(),
            text: "Rebuilt the ingestion pipeline in Rust".into(),
            variants: BTreeMap::from([(
                "short".to_string(),
                "Rebuilt ingestion (Rust)".to_string(),
            )]),
            situation: None,
            action: None,
            result: None,
            impact_metric: metric.then(|| "p95 latency".to_string()),
            impact_value: metric.then_some(85.0),
            impact_unit: metric.then(|| "%".to_string()),
            impact_direction: Some(ImpactDirection::Decrease),
            strength,
            verified,
            evidence_url: None,
            evidence_note: None,
            scope: Default::default(),
            skill_ids: vec![],
            ordinal: 0,
            created_at: now(),
            updated_at: now(),
        }
    }

    #[test]
    fn quality_prefers_quantified_and_verified_bullets() {
        assert!(acc(5, true, true).quality_score() > acc(5, false, false).quality_score());
        assert!(acc(3, true, true).quality_score() > acc(3, true, false).quality_score());
        assert_eq!(acc(5, true, true).quality_score(), 1.0, "capped at 1.0");
    }

    #[test]
    fn variants_fall_back_to_the_canonical_text() {
        let a = acc(4, true, true);
        assert_eq!(a.variant("short"), "Rebuilt ingestion (Rust)");
        assert_eq!(a.variant("leadership"), a.text);
    }

    #[test]
    fn only_work_like_kinds_count_toward_years() {
        assert!(ExperienceKind::Role.counts_as_work());
        assert!(ExperienceKind::Oss.counts_as_work());
        assert!(!ExperienceKind::Education.counts_as_work());
        assert!(!ExperienceKind::Award.counts_as_work());
    }
}
