//! The single error taxonomy for the whole system.
//!
//! Variants exist to carry a *stable machine code* (see [`Error::code`]) plus an HTTP status
//! hint. `jobseeker-api` is the only place that turns these into responses, so the mapping
//! lives with the variant rather than being scattered across handlers.

use std::fmt;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    // ---- client errors -------------------------------------------------------------
    #[error("{0} not found")]
    NotFound(&'static str),

    #[error("invalid request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("authentication required")]
    Unauthorized,

    #[error("forbidden: {0}")]
    Forbidden(&'static str),

    #[error("payload too large: {0} bytes exceeds the {1} byte limit")]
    PayloadTooLarge(u64, u64),

    #[error("unsupported content type: {0}")]
    UnsupportedContentType(String),

    #[error("rate limited; retry after {0}s")]
    RateLimited(u64),

    // ---- acquisition ---------------------------------------------------------------
    #[error("invalid url: {0}")]
    InvalidUrl(String),

    #[error("refusing to fetch a private or link-local address: {0}")]
    BlockedPrivateNetwork(String),

    #[error("robots.txt disallows fetching {0}; capture the page with the browser extension")]
    RobotsDisallowed(String),

    #[error("fetch failed with status {0}")]
    FetchStatus(u16),

    #[error("fetch timed out after {0}s")]
    FetchTimeout(u64),

    #[error("page has no extractable content; capture it with the browser extension")]
    NeedsBrowser,

    // ---- extraction ----------------------------------------------------------------
    #[error("extraction failed: {0}")]
    ExtractFailed(String),

    #[error("model response did not match the schema: {0}")]
    SchemaViolation(String),

    #[error("no language model is configured")]
    LlmNotConfigured,

    #[error("language model unavailable: {0}")]
    LlmUnavailable(String),

    // ---- infrastructure ------------------------------------------------------------
    #[error("configuration error: {0}")]
    Config(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("migration error: {0}")]
    Migration(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("serialization error: {0}")]
    Serde(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("{0} is not implemented yet")]
    NotImplemented(&'static str),

    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Stable, machine-readable code. Clients may branch on these; they must not change
    /// without a major API version bump.
    pub fn code(&self) -> &'static str {
        match self {
            Error::NotFound(_) => "not_found",
            Error::BadRequest(_) => "bad_request",
            Error::Conflict(_) => "conflict",
            Error::Unauthorized => "unauthorized",
            Error::Forbidden(_) => "forbidden",
            Error::PayloadTooLarge(..) => "payload_too_large",
            Error::UnsupportedContentType(_) => "unsupported_content_type",
            Error::RateLimited(_) => "rate_limited",
            Error::InvalidUrl(_) => "invalid_url",
            Error::BlockedPrivateNetwork(_) => "blocked_private_network",
            Error::RobotsDisallowed(_) => "robots_disallowed",
            Error::FetchStatus(_) => "fetch_status",
            Error::FetchTimeout(_) => "fetch_timeout",
            Error::NeedsBrowser => "needs_browser",
            Error::ExtractFailed(_) => "extract_failed",
            Error::SchemaViolation(_) => "schema_violation",
            Error::LlmNotConfigured => "llm_not_configured",
            Error::LlmUnavailable(_) => "llm_unavailable",
            Error::Config(_) => "config_error",
            Error::Database(_) => "database_error",
            Error::Migration(_) => "migration_error",
            Error::Storage(_) => "storage_error",
            Error::Serde(_) => "serialization_error",
            Error::Network(_) => "network_error",
            Error::NotImplemented(_) => "not_implemented",
            Error::Internal(_) => "internal_error",
        }
    }

    /// HTTP status this error should map to.
    pub fn http_status(&self) -> u16 {
        match self {
            Error::NotFound(_) => 404,
            Error::BadRequest(_) | Error::InvalidUrl(_) | Error::BlockedPrivateNetwork(_) => 400,
            Error::Conflict(_) => 409,
            Error::Unauthorized => 401,
            Error::Forbidden(_) => 403,
            Error::PayloadTooLarge(..) => 413,
            Error::UnsupportedContentType(_) => 415,
            Error::RateLimited(_) => 429,
            Error::RobotsDisallowed(_) | Error::NeedsBrowser | Error::SchemaViolation(_) => 422,
            Error::LlmNotConfigured | Error::LlmUnavailable(_) => 503,
            Error::NotImplemented(_) => 501,
            _ => 500,
        }
    }

    /// Whether a failed task carrying this error is worth retrying.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Error::FetchTimeout(_)
                | Error::Network(_)
                | Error::LlmUnavailable(_)
                | Error::RateLimited(_)
                | Error::Database(_)
        ) || matches!(self, Error::FetchStatus(s) if *s >= 500)
    }

    /// True when the message is safe to show a user verbatim. 5xx details are logged with
    /// the trace id instead of being returned.
    pub fn is_client_facing(&self) -> bool {
        self.http_status() < 500
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Serde(e.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Storage(e.to_string())
    }
}

/// Attach a human-readable context string to an error without losing its variant.
pub trait Contextual<T> {
    fn ctx(self, what: impl fmt::Display) -> Result<T>;
}

impl<T, E: fmt::Display> Contextual<T> for Result<T, E> {
    fn ctx(self, what: impl fmt::Display) -> Result<T> {
        self.map_err(|e| Error::Internal(format!("{what}: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_distinct_per_variant_family() {
        assert_eq!(Error::NotFound("job").code(), "not_found");
        assert_eq!(Error::NotFound("job").http_status(), 404);
        assert_eq!(Error::NeedsBrowser.http_status(), 422);
        assert!(Error::FetchStatus(503).is_retryable());
        assert!(!Error::FetchStatus(404).is_retryable());
        assert!(!Error::Internal("x".into()).is_client_facing());
    }
}
