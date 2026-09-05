//! Employers.

use serde::{Deserialize, Serialize};

use crate::ids::CompanyId;
use crate::time::Timestamp;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Company {
    pub id: CompanyId,
    pub name: String,
    pub slug: String,
    /// Legal suffixes stripped and lowercased; the key used to fold `"Acme, Inc."` and
    /// `"ACME"` into one employer.
    pub name_normalized: String,
    pub website: Option<String>,
    pub careers_url: Option<String>,
    pub linkedin_url: Option<String>,
    pub industry: Option<String>,
    pub size_bucket: Option<String>,
    pub hq_location: Option<String>,
    pub funding_stage: Option<String>,
    pub is_public: Option<bool>,
    pub notes_md: Option<String>,
    /// Your own interest, 0–5.
    pub rating: Option<i32>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// Headcount buckets. Stored as text so the exported files stay readable.
pub const SIZE_BUCKETS: &[&str] = &[
    "1-10",
    "11-50",
    "51-200",
    "201-500",
    "501-1000",
    "1001-5000",
    "5001-10000",
    "10001+",
    "unknown",
];

pub fn is_valid_size_bucket(bucket: &str) -> bool {
    SIZE_BUCKETS.contains(&bucket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_buckets_are_validated() {
        assert!(is_valid_size_bucket("51-200"));
        assert!(is_valid_size_bucket("unknown"));
        assert!(!is_valid_size_bucket("medium-ish"));
    }
}
