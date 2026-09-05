//! Immutable raw acquisition artifacts.
//!
//! A capture is written to disk and recorded *before* any parsing happens (FR-A-03), so an
//! extraction bug is always recoverable: fix the adapter, re-run over historical captures.

use serde::{Deserialize, Serialize};

use crate::ids::{CaptureId, ListingId};
use crate::time::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMethod {
    /// Server-side HTTP fetch of a public page.
    Http,
    /// The browser extension shipped a rendered DOM from a page the user was viewing.
    Extension,
    /// Opt-in local headless browser (off by default; see ADR-0004).
    Browser,
    /// A site's structured JSON API.
    Api,
    /// The user pasted text or HTML.
    Paste,
    /// Read from a local file (fixtures, bulk import).
    File,
}

impl CaptureMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureMethod::Http => "http",
            CaptureMethod::Extension => "extension",
            CaptureMethod::Browser => "browser",
            CaptureMethod::Api => "api",
            CaptureMethod::Paste => "paste",
            CaptureMethod::File => "file",
        }
    }

    /// Did this capture come from the user's own authenticated browser session? Such
    /// captures cannot be refreshed server-side; refresh has to prompt the user instead.
    pub fn is_user_mediated(self) -> bool {
        matches!(self, CaptureMethod::Extension | CaptureMethod::Paste)
    }
}

impl std::str::FromStr for CaptureMethod {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        Ok(match s {
            "http" => CaptureMethod::Http,
            "extension" => CaptureMethod::Extension,
            "browser" => CaptureMethod::Browser,
            "api" => CaptureMethod::Api,
            "paste" => CaptureMethod::Paste,
            "file" => CaptureMethod::File,
            other => {
                return Err(crate::Error::BadRequest(format!(
                    "unknown capture method: {other}"
                )))
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capture {
    pub id: CaptureId,
    pub listing_id: Option<ListingId>,
    pub url: Option<String>,
    pub method: CaptureMethod,
    pub http_status: Option<u16>,
    pub content_type: Option<String>,
    pub byte_len: i64,
    /// BLAKE3 of the raw body. Identical captures share one blob on disk.
    pub content_hash: String,
    /// Data-dir-relative path to the (zstd-compressed) body.
    pub storage_path: String,
    pub screenshot_path: Option<String>,
    pub captured_at: Timestamp,
    pub user_agent: Option<String>,
    pub client_version: Option<String>,
    pub extract_status: ExtractStatus,
    pub extract_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractStatus {
    Pending,
    Ok,
    Failed,
}

impl ExtractStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ExtractStatus::Pending => "pending",
            ExtractStatus::Ok => "ok",
            ExtractStatus::Failed => "failed",
        }
    }
}

impl std::str::FromStr for ExtractStatus {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        Ok(match s {
            "pending" => ExtractStatus::Pending,
            "ok" => ExtractStatus::Ok,
            "failed" => ExtractStatus::Failed,
            other => {
                return Err(crate::Error::BadRequest(format!(
                    "unknown extract status: {other}"
                )))
            }
        })
    }
}

/// What the extension POSTs. Deliberately dumb: it ships the page and lets the server think,
/// so improving extraction never requires shipping a new extension.
#[derive(Debug, Clone, Deserialize)]
pub struct CaptureSubmission {
    pub url: String,
    pub html: String,
    /// `innerText` of the detected job container, as a fallback when DOM parsing fails.
    #[serde(default)]
    pub text: Option<String>,
    /// The user's selection, which wins over the auto-detected container when present.
    #[serde(default)]
    pub selected_html: Option<String>,
    #[serde(default)]
    pub screenshot_png_b64: Option<String>,
    #[serde(default)]
    pub source_hint: Option<String>,
    #[serde(default)]
    pub captured_at: Option<Timestamp>,
    #[serde(default)]
    pub client: Option<ClientInfo>,
    /// JSON-LD blocks the extension already found, so a server-side parse failure does not
    /// lose the best available signal.
    #[serde(default)]
    pub page_meta: Option<serde_json::Value>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub browser: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn methods_round_trip() {
        for m in [
            CaptureMethod::Http,
            CaptureMethod::Extension,
            CaptureMethod::Browser,
            CaptureMethod::Api,
            CaptureMethod::Paste,
            CaptureMethod::File,
        ] {
            assert_eq!(CaptureMethod::from_str(m.as_str()).unwrap(), m);
        }
    }

    #[test]
    fn extension_captures_cannot_be_refreshed_server_side() {
        assert!(CaptureMethod::Extension.is_user_mediated());
        assert!(!CaptureMethod::Http.is_user_mediated());
        assert!(!CaptureMethod::Api.is_user_mediated());
    }
}
