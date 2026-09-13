//! Deterministic normalization.
//!
//! Everything in this crate is a pure function over strings. That is deliberate: these are
//! the rules that turn `"$185k - $225k/yr DOE"` into an integer range, and they are the
//! rules most likely to be wrong on real-world input. Keeping them free of I/O means every
//! one of them is testable from a fixture string, and the eval harness
//! (`docs/15-testing.md`) can measure them without a network or a model.
//!
//! Stage 5 of extraction (`Provenance::Rules`) is built almost entirely from this crate.

pub mod date;
pub mod location;
pub mod requirement;
pub mod salary;
pub mod seniority;
pub mod skill;
pub mod text;
pub mod url;

pub use date::parse_posted_date;
pub use location::parse_location;
pub use salary::parse_salary;
pub use seniority::{
    detect_clearance, detect_clearance_demand, infer_employment_type, infer_seniority,
    infer_work_mode, ClearanceDemand,
};
pub use url::{canonicalize, detect_source, CanonicalUrl};
