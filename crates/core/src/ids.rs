//! Typed identifiers.
//!
//! All ids are UUIDv7 rendered lowercase-dashed: time-ordered (good B-tree locality),
//! globally unique (two installations can be merged), and opaque. Distinct newtypes make
//! it impossible to pass a `ProfileId` where a `JobId` belongs.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

macro_rules! typed_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Mint a new time-ordered id.
            pub fn new() -> Self {
                Self(uuid::Uuid::now_v7().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }

            /// Short form for display and file paths: the last 8 hex characters.
            pub fn short(&self) -> &str {
                let s = &self.0;
                &s[s.len().saturating_sub(8)..]
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = crate::Error;
            fn from_str(s: &str) -> crate::Result<Self> {
                let uuid = uuid::Uuid::parse_str(s)
                    .map_err(|_| crate::Error::BadRequest(format!("malformed id: {s}")))?;
                Ok(Self(uuid.to_string()))
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

    };
}

typed_id!(
    /// A canonical job posting, deduplicated across every site that cross-posted it.
    JobId
);
typed_id!(CompanyId);
typed_id!(
    /// One posting as seen at one URL; many listings may point at one [`JobId`].
    ListingId
);
typed_id!(
    /// An immutable raw acquisition artifact.
    CaptureId
);
typed_id!(RequirementId);
typed_id!(SkillId);
typed_id!(ProfileId);
typed_id!(ExperienceId);
typed_id!(AccomplishmentId);
typed_id!(MatchScoreId);
typed_id!(ApplicationId);
typed_id!(DocumentId);
typed_id!(ContactId);
typed_id!(TaskId);
typed_id!(UserId);
typed_id!(TagId);
typed_id!(SourceId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_time_ordered() {
        let a = JobId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = JobId::new();
        assert!(a < b, "uuid v7 ids must sort by creation time: {a} !< {b}");
    }

    #[test]
    fn short_form_is_eight_chars() {
        assert_eq!(JobId::new().short().len(), 8);
    }

    #[test]
    fn round_trips_through_string() {
        let id = JobId::new();
        assert_eq!(JobId::from_str(id.as_str()).unwrap(), id);
        assert!(JobId::from_str("not-a-uuid").is_err());
    }
}
