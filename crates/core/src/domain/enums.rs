//! Closed vocabularies.
//!
//! Each of these is stored as `TEXT` with a `CHECK` constraint: readable in `sqlite3` and in
//! the exported files, and typo-proof at the database boundary. The `str_enum!` macro keeps
//! the Rust and SQL spellings in exactly one place.

use serde::{Deserialize, Serialize};

macro_rules! str_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $( $(#[$vmeta:meta])* $variant:ident => $text:literal ),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $( $name::$variant => $text, )+ }
            }

            /// Every legal value, in declaration order. Used to generate `CHECK` clauses and
            /// to enumerate facets in the UI.
            pub const ALL: &'static [$name] = &[ $( $name::$variant, )+ ];
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::Error;
            fn from_str(s: &str) -> $crate::Result<Self> {
                match s {
                    $( $text => Ok($name::$variant), )+
                    other => Err($crate::Error::BadRequest(format!(
                        concat!("unknown ", stringify!($name), ": {}"), other
                    ))),
                }
            }
        }
    };
}

str_enum!(
    /// Lifecycle of a posting. Closure never deletes data — a closed posting is the evidence
    /// of what you applied to.
    JobStatus {
        Open => "open",
        Closed => "closed",
        Filled => "filled",
        Expired => "expired",
        Removed => "removed",
        Unknown => "unknown",
    }
);

str_enum!(
    WorkMode {
        Remote => "remote",
        Hybrid => "hybrid",
        Onsite => "onsite",
        Unknown => "unknown",
    }
);

str_enum!(
    EmploymentType {
        FullTime => "full_time",
        PartTime => "part_time",
        Contract => "contract",
        ContractToHire => "contract_to_hire",
        Internship => "internship",
        Temporary => "temporary",
        Volunteer => "volunteer",
        Unknown => "unknown",
    }
);

str_enum!(
    /// Ordered from least to most senior; `Seniority::rank` is what `seniority_fit` compares.
    Seniority {
        Intern => "intern",
        Entry => "entry",
        Junior => "junior",
        Mid => "mid",
        Senior => "senior",
        Staff => "staff",
        Principal => "principal",
        Lead => "lead",
        Manager => "manager",
        Director => "director",
        Vp => "vp",
        Exec => "exec",
        Unknown => "unknown",
    }
);

impl Seniority {
    /// Position on the individual-contributor ladder. Management titles are given ranks that
    /// place them alongside comparable IC levels; `None` means "not comparable".
    pub fn rank(self) -> Option<u8> {
        Some(match self {
            Seniority::Intern => 0,
            Seniority::Entry => 1,
            Seniority::Junior => 2,
            Seniority::Mid => 3,
            Seniority::Senior => 4,
            Seniority::Staff => 5,
            Seniority::Lead => 5,
            Seniority::Principal => 6,
            Seniority::Manager => 5,
            Seniority::Director => 7,
            Seniority::Vp => 8,
            Seniority::Exec => 9,
            Seniority::Unknown => return None,
        })
    }

    /// Typical years-of-experience band, used when a posting states years but not a level.
    pub fn from_years(years: f32) -> Seniority {
        match years {
            y if y < 1.0 => Seniority::Entry,
            y if y < 2.0 => Seniority::Junior,
            y if y < 5.0 => Seniority::Mid,
            y if y < 9.0 => Seniority::Senior,
            _ => Seniority::Staff,
        }
    }
}

str_enum!(
    ApplyKind {
        Ats => "ats",
        External => "external",
        Email => "email",
        EasyApply => "easy_apply",
        Unknown => "unknown",
    }
);

str_enum!(
    /// Ordered so `EducationLevel::rank` can compare "bachelor required" against what you
    /// have.
    EducationLevel {
        None_ => "none",
        Hs => "hs",
        Associate => "associate",
        Bachelor => "bachelor",
        Master => "master",
        Doctorate => "doctorate",
        Unknown => "unknown",
    }
);

impl EducationLevel {
    pub fn rank(self) -> Option<u8> {
        Some(match self {
            EducationLevel::None_ => 0,
            EducationLevel::Hs => 1,
            EducationLevel::Associate => 2,
            EducationLevel::Bachelor => 3,
            EducationLevel::Master => 4,
            EducationLevel::Doctorate => 5,
            EducationLevel::Unknown => return None,
        })
    }
}

str_enum!(
    /// What kind of demand a requirement expresses. Drives which matcher runs and how the
    /// requirement is weighted.
    RequirementKind {
        Skill => "skill",
        Tool => "tool",
        Experience => "experience",
        Education => "education",
        Certification => "certification",
        Clearance => "clearance",
        Language => "language",
        SoftSkill => "soft_skill",
        Domain => "domain",
        Responsibility => "responsibility",
        Logistics => "logistics",
        Other => "other",
    }
);

impl RequirementKind {
    /// Base weight in `required_coverage`. Soft skills are unfalsifiable, so they are
    /// deliberately near-irrelevant to the score.
    pub fn base_weight(self) -> f32 {
        match self {
            RequirementKind::Skill => 1.0,
            RequirementKind::Experience => 1.0,
            RequirementKind::Clearance => 1.0,
            RequirementKind::Tool => 0.9,
            RequirementKind::Language => 0.9,
            RequirementKind::Education => 0.8,
            RequirementKind::Certification => 0.8,
            RequirementKind::Domain => 0.8,
            RequirementKind::Logistics => 0.7,
            RequirementKind::SoftSkill => 0.3,
            RequirementKind::Other => 0.5,
            // Responsibilities describe the job, not a bar to clear; they inform resume
            // targeting and interview prep instead of fit.
            RequirementKind::Responsibility => 0.0,
        }
    }

    pub fn is_scored(self) -> bool {
        self.base_weight() > 0.0
    }
}

str_enum!(
    Necessity {
        Required => "required",
        Preferred => "preferred",
        NiceToHave => "nice_to_have",
        /// Not stated outright but clearly implied (e.g. a Rust role implies Rust).
        Implied => "implied",
    }
);

str_enum!(
    SkillLevel {
        Exposure => "exposure",
        Working => "working",
        Proficient => "proficient",
        Expert => "expert",
    }
);

impl SkillLevel {
    pub fn rank(self) -> u8 {
        match self {
            SkillLevel::Exposure => 0,
            SkillLevel::Working => 1,
            SkillLevel::Proficient => 2,
            SkillLevel::Expert => 3,
        }
    }
}

str_enum!(
    /// Where a listing came from. `fidelity` decides which source wins a merge.
    SourceKind {
        LinkedIn => "linkedin",
        Indeed => "indeed",
        Glassdoor => "glassdoor",
        ZipRecruiter => "ziprecruiter",
        Dice => "dice",
        Greenhouse => "greenhouse",
        Lever => "lever",
        Ashby => "ashby",
        Workday => "workday",
        SmartRecruiters => "smartrecruiters",
        Workable => "workable",
        Recruitee => "recruitee",
        CompanySite => "company_site",
        Manual => "manual",
        Email => "email",
        Other => "other",
    }
);

impl SourceKind {
    /// Higher wins when two listings describe the same job. An employer's own ATS is more
    /// trustworthy than an aggregator's reconstruction of it.
    pub fn fidelity(self) -> u8 {
        match self {
            SourceKind::Manual => 100,
            SourceKind::Greenhouse
            | SourceKind::Lever
            | SourceKind::Ashby
            | SourceKind::SmartRecruiters
            | SourceKind::Workable
            | SourceKind::Recruitee => 90,
            SourceKind::Workday => 75,
            SourceKind::CompanySite => 70,
            SourceKind::LinkedIn => 45,
            SourceKind::Indeed => 40,
            SourceKind::Dice => 35,
            SourceKind::Glassdoor | SourceKind::ZipRecruiter => 30,
            SourceKind::Email => 20,
            SourceKind::Other => 10,
        }
    }

    /// Sites whose salary figures are frequently the site's own estimate rather than the
    /// employer's posted band.
    pub fn estimates_salary(self) -> bool {
        matches!(
            self,
            SourceKind::Indeed
                | SourceKind::Glassdoor
                | SourceKind::ZipRecruiter
                | SourceKind::LinkedIn
        )
    }

    /// Sites that require a login, so the extension is the right acquisition path.
    pub fn requires_authentication(self) -> bool {
        matches!(
            self,
            SourceKind::LinkedIn | SourceKind::Indeed | SourceKind::Glassdoor
        )
    }
}

str_enum!(
    ApplicationStatus {
        Interested => "interested",
        Preparing => "preparing",
        Applied => "applied",
        Screening => "screening",
        Interviewing => "interviewing",
        Offer => "offer",
        Accepted => "accepted",
        Rejected => "rejected",
        Withdrawn => "withdrawn",
        Ghosted => "ghosted",
    }
);

impl ApplicationStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ApplicationStatus::Accepted
                | ApplicationStatus::Rejected
                | ApplicationStatus::Withdrawn
                | ApplicationStatus::Ghosted
        )
    }

    /// Kanban column order.
    pub fn ordinal(self) -> u8 {
        match self {
            ApplicationStatus::Interested => 0,
            ApplicationStatus::Preparing => 1,
            ApplicationStatus::Applied => 2,
            ApplicationStatus::Screening => 3,
            ApplicationStatus::Interviewing => 4,
            ApplicationStatus::Offer => 5,
            ApplicationStatus::Accepted => 6,
            ApplicationStatus::Rejected => 7,
            ApplicationStatus::Withdrawn => 8,
            ApplicationStatus::Ghosted => 9,
        }
    }
}

str_enum!(
    /// For fields where "not stated" is genuinely different from "no".
    Tristate {
        Yes => "yes",
        No => "no",
        Unspecified => "unspecified",
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn all_variants_round_trip_through_their_sql_spelling() {
        for v in JobStatus::ALL {
            assert_eq!(JobStatus::from_str(v.as_str()).unwrap(), *v);
        }
        for v in RequirementKind::ALL {
            assert_eq!(RequirementKind::from_str(v.as_str()).unwrap(), *v);
        }
        for v in SourceKind::ALL {
            assert_eq!(SourceKind::from_str(v.as_str()).unwrap(), *v);
        }
        for v in Seniority::ALL {
            assert_eq!(Seniority::from_str(v.as_str()).unwrap(), *v);
        }
        for v in ApplicationStatus::ALL {
            assert_eq!(ApplicationStatus::from_str(v.as_str()).unwrap(), *v);
        }
    }

    #[test]
    fn unknown_values_are_rejected_not_defaulted() {
        assert!(JobStatus::from_str("kinda_open").is_err());
        assert!(RequirementKind::from_str("vibes").is_err());
    }

    #[test]
    fn ats_sources_outrank_aggregators() {
        assert!(SourceKind::Greenhouse.fidelity() > SourceKind::LinkedIn.fidelity());
        assert!(SourceKind::CompanySite.fidelity() > SourceKind::Indeed.fidelity());
        assert!(SourceKind::Manual.fidelity() > SourceKind::Greenhouse.fidelity());
    }

    #[test]
    fn aggregators_are_flagged_as_estimating_salary() {
        assert!(SourceKind::Indeed.estimates_salary());
        assert!(!SourceKind::Greenhouse.estimates_salary());
    }

    #[test]
    fn responsibilities_do_not_affect_fit() {
        assert!(!RequirementKind::Responsibility.is_scored());
        assert!(RequirementKind::Skill.is_scored());
        assert!(
            RequirementKind::SoftSkill.base_weight() < RequirementKind::Skill.base_weight(),
            "soft skills must stay near-irrelevant to the score"
        );
    }

    #[test]
    fn seniority_bands_follow_years() {
        assert_eq!(Seniority::from_years(0.5), Seniority::Entry);
        assert_eq!(Seniority::from_years(3.0), Seniority::Mid);
        assert_eq!(Seniority::from_years(6.0), Seniority::Senior);
        assert_eq!(Seniority::from_years(12.0), Seniority::Staff);
        assert!(Seniority::Senior.rank() > Seniority::Mid.rank());
        assert_eq!(Seniority::Unknown.rank(), None);
    }
}
