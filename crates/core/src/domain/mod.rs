//! The domain model. These types are storage- and transport-agnostic; `jobseeker-db` maps
//! them to rows and `jobseeker-api` maps them to DTOs.

pub mod capture;
pub mod company;
pub mod enums;
pub mod event;
pub mod job;
pub mod location;
pub mod profile;
pub mod requirement;
pub mod salary;
pub mod scoring;
pub mod task;

pub use capture::{Capture, CaptureMethod};
pub use company::Company;
pub use enums::{
    is_possibly_ghosted, ApplicationStatus, ApplyKind, EducationLevel, EmploymentType, JobStatus,
    Necessity, RequirementKind, Seniority, SkillLevel, SourceKind, Tristate, WorkMode,
    GHOST_IDLE_DAYS,
};
pub use event::DomainEvent;
pub use job::{is_closing_soon_unapplied, ExtractedJob, Job, CLOSING_SOON_DAYS};
pub use location::{JobLocation, RawLocation};
pub use profile::{Accomplishment, ExperienceItem, ExperienceKind, Profile, ProfileSkill};
pub use requirement::{Requirement, Skill};
pub use salary::{RawSalary, Salary, SalaryPeriod};
pub use scoring::{MatchScore, RequirementMatch, Subscores, VerdictStatus};
pub use task::{Task, TaskKind, TaskStatus};
