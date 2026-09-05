//! Domain model, typed identifiers, error taxonomy and configuration.
//!
//! This crate is the bottom of the dependency graph: it knows nothing about SQL, HTTP or
//! the filesystem. Everything above it speaks in these types.

pub mod config;
pub mod domain;
pub mod error;
pub mod hash;
pub mod ids;
pub mod provenance;
pub mod slug;
pub mod telemetry;
pub mod time;

pub use error::{Error, Result};
pub use provenance::{Confidence, Provenance, Sourced};

/// Version of this build, surfaced by `/api/v1/meta`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
