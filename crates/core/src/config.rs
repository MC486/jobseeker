//! Configuration: layered defaults → file → environment, validated once at boot.
//!
//! Unknown keys are rejected: a typo in a config file should fail loudly at startup rather
//! than silently leaving a setting at its default (NFR-O-02).

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub server: ServerConfig,
    pub data: DataConfig,
    pub auth: AuthConfig,
    pub worker: WorkerConfig,
    pub acquire: AcquireConfig,
    pub llm: LlmConfig,
    pub matching: MatchingConfig,
    pub resume: ResumeConfig,
    pub log: LogConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfig {
    /// Defaults to loopback: exposing the service must be a deliberate act (NFR-S-06).
    pub bind: String,
    /// Absolute URL the UI is reached at; used for links and the extension CORS allowlist.
    pub public_url: Option<String>,
    /// Extra origins allowed to call the ingest endpoints (browser extension ids).
    pub extra_allowed_origins: Vec<String>,
    pub request_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DataConfig {
    pub dir: PathBuf,
    /// Defaults to `<dir>/jobseeker.db` when unset.
    pub db_path: Option<PathBuf>,
    pub run_migrations_on_start: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// No authentication. Only appropriate on a trusted-network bind; warned about at boot.
    None,
    Token,
    Password,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AuthConfig {
    pub mode: AuthMode,
    pub session_ttl_days: u32,
    pub login_rate_limit_per_15min: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WorkerConfig {
    /// `0` means "derive from the core count", clamped to 2..=8.
    pub concurrency: usize,
    pub lease_seconds: u64,
    pub max_attempts: u32,
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AcquireConfig {
    pub user_agent: String,
    pub respect_robots: bool,
    pub per_host_delay_ms: u64,
    pub timeout_seconds: u64,
    /// SSRF guard escape hatch, for people self-hosting the board they are ingesting.
    pub allow_private_networks: bool,
    pub max_body_bytes: u64,
    pub max_screenshot_bytes: u64,
    /// Opt-in headless browser acquisition. Off by default; see ADR-0004.
    pub browser_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    None,
    Ollama,
    OpenAi,
    Anthropic,
    Mock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LlmConfig {
    pub provider: LlmProvider,
    pub base_url: Option<String>,
    pub model: String,
    pub embedding_provider: LlmProvider,
    pub embedding_model: String,
    pub max_input_tokens: usize,
    pub timeout_seconds: u64,
    pub cache: bool,
    /// Override the provider for a specific purpose, e.g. keep resume writing local while
    /// using a cloud model for extraction.
    pub purpose_overrides: BTreeMap<String, LlmProvider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MatchingConfig {
    pub algorithm_version: String,
    pub weights: Weights,
    /// A job with an unmet hard blocker cannot score above this, however good the skill fit.
    pub blocker_cap: f32,
    pub recency_decay_after_years: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Weights {
    pub required_coverage: f32,
    pub preferred_coverage: f32,
    pub semantic: f32,
    pub seniority: f32,
    pub comp: f32,
    pub location: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ResumeConfig {
    pub renderer: String,
    pub default_template: String,
    pub templates_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LogConfig {
    pub level: String,
    /// `pretty` for a terminal, `json` for journald + `jq`.
    pub format: String,
}

// ---------------------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------------------

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8787".into(),
            public_url: None,
            extra_allowed_origins: Vec::new(),
            request_timeout_seconds: 60,
        }
    }
}

impl Default for DataConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("./data"),
            db_path: None,
            run_migrations_on_start: true,
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: AuthMode::None,
            session_ttl_days: 30,
            login_rate_limit_per_15min: 10,
        }
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            concurrency: 0,
            lease_seconds: 120,
            max_attempts: 5,
            poll_interval_ms: 1000,
        }
    }
}

impl Default for AcquireConfig {
    fn default() -> Self {
        Self {
            user_agent: format!(
                "jobseeker/{} (+self-hosted personal use)",
                env!("CARGO_PKG_VERSION")
            ),
            respect_robots: true,
            per_host_delay_ms: 2000,
            timeout_seconds: 20,
            allow_private_networks: false,
            max_body_bytes: 8 * 1024 * 1024,
            max_screenshot_bytes: 12 * 1024 * 1024,
            browser_enabled: false,
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: LlmProvider::None,
            base_url: None,
            model: "qwen2.5:14b-instruct".into(),
            embedding_provider: LlmProvider::None,
            embedding_model: "nomic-embed-text".into(),
            max_input_tokens: 16_000,
            timeout_seconds: 120,
            cache: true,
            purpose_overrides: BTreeMap::new(),
        }
    }
}

impl Default for MatchingConfig {
    fn default() -> Self {
        Self {
            algorithm_version: "1.1.0".into(),
            weights: Weights::default(),
            blocker_cap: 0.45,
            recency_decay_after_years: 3.0,
        }
    }
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            required_coverage: 0.45,
            preferred_coverage: 0.15,
            semantic: 0.15,
            seniority: 0.10,
            comp: 0.10,
            location: 0.05,
        }
    }
}

impl Default for ResumeConfig {
    fn default() -> Self {
        Self {
            renderer: "typst".into(),
            default_template: "ats".into(),
            templates_dir: None,
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            format: "pretty".into(),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Loading and validation
// ---------------------------------------------------------------------------------------

impl Config {
    /// Layered load: defaults → optional file → `JOBSEEKER__*` env vars.
    ///
    /// Env keys use a double underscore as the section separator, e.g.
    /// `JOBSEEKER__SERVER__BIND=0.0.0.0:8787`.
    pub fn load(path: Option<&Path>) -> crate::Result<Self> {
        let mut figment = Figment::from(Serialized::defaults(Config::default()));
        if let Some(path) = path {
            if !path.exists() {
                return Err(crate::Error::Config(format!(
                    "config file not found: {}",
                    path.display()
                )));
            }
            figment = figment.merge(Toml::file(path));
        }
        let config: Config = figment
            .merge(Env::prefixed("JOBSEEKER__").split("__"))
            .extract()
            .map_err(|e| crate::Error::Config(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> crate::Result<()> {
        if self.server.bind.parse::<std::net::SocketAddr>().is_err() {
            return Err(crate::Error::Config(format!(
                "server.bind must be an address like 127.0.0.1:8787, got {:?}",
                self.server.bind
            )));
        }
        if self.worker.lease_seconds < 10 {
            return Err(crate::Error::Config(
                "worker.lease_seconds must be at least 10".into(),
            ));
        }
        if self.worker.max_attempts == 0 {
            return Err(crate::Error::Config(
                "worker.max_attempts must be at least 1".into(),
            ));
        }
        // A rate-limit floor is a project invariant, not a preference: see
        // docs/13-security-privacy-legal.md.
        if self.acquire.per_host_delay_ms < 500 {
            return Err(crate::Error::Config(
                "acquire.per_host_delay_ms must be at least 500 to stay a polite client".into(),
            ));
        }
        let w = &self.matching.weights;
        let sum = w.required_coverage
            + w.preferred_coverage
            + w.semantic
            + w.seniority
            + w.comp
            + w.location;
        if sum <= 0.0 {
            return Err(crate::Error::Config(
                "matching.weights must not all be zero".into(),
            ));
        }
        if !(0.0..=1.0).contains(&self.matching.blocker_cap) {
            return Err(crate::Error::Config(
                "matching.blocker_cap must be within 0.0..=1.0".into(),
            ));
        }
        if self.llm.provider != LlmProvider::None && self.llm.model.is_empty() {
            return Err(crate::Error::Config(
                "llm.model is required when llm.provider is set".into(),
            ));
        }
        Ok(())
    }

    pub fn db_path(&self) -> PathBuf {
        self.data
            .db_path
            .clone()
            .unwrap_or_else(|| self.data.dir.join("jobseeker.db"))
    }

    pub fn jobs_dir(&self) -> PathBuf {
        self.data.dir.join("jobs")
    }

    pub fn captures_dir(&self) -> PathBuf {
        self.data.dir.join("captures")
    }

    pub fn media_dir(&self) -> PathBuf {
        self.data.dir.join("media")
    }

    pub fn documents_dir(&self) -> PathBuf {
        self.data.dir.join("documents")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.data.dir.join("backups")
    }

    pub fn worker_concurrency(&self) -> usize {
        if self.worker.concurrency > 0 {
            self.worker.concurrency
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2)
                .clamp(2, 8)
        }
    }

    /// True when the service is reachable beyond loopback with no authentication — worth
    /// shouting about at startup.
    pub fn is_dangerously_open(&self) -> bool {
        let non_loopback = self
            .server
            .bind
            .parse::<std::net::SocketAddr>()
            .map(|a| !a.ip().is_loopback())
            .unwrap_or(false);
        non_loopback && self.auth.mode == AuthMode::None
    }
}

/// Redacts nothing today because no secret lives in `Config` (keys come from the
/// environment), but the wrapper exists so that adding one cannot accidentally log it.
pub struct Redacted<'a>(pub &'a Config);

impl fmt::Debug for Redacted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("server", &self.0.server)
            .field("data", &self.0.data)
            .field("auth", &self.0.auth)
            .field("worker", &self.0.worker)
            .field("acquire", &self.0.acquire)
            .field("llm.provider", &self.0.llm.provider)
            .field("llm.model", &self.0.llm.model)
            .field("llm.api_key", &"<from environment>")
            .field("matching", &self.0.matching)
            .field("resume", &self.0.resume)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_safe() {
        let c = Config::default();
        c.validate().unwrap();
        assert_eq!(c.server.bind, "127.0.0.1:8787");
        assert_eq!(c.auth.mode, AuthMode::None);
        assert!(!c.is_dangerously_open(), "loopback + no auth is fine");
        assert!(!c.acquire.browser_enabled, "browser acquisition is opt-in");
        assert!(c.acquire.respect_robots);
    }

    #[test]
    fn open_bind_without_auth_is_flagged() {
        let mut c = Config::default();
        c.server.bind = "0.0.0.0:8787".into();
        assert!(c.is_dangerously_open());
        c.auth.mode = AuthMode::Password;
        assert!(!c.is_dangerously_open());
    }

    #[test]
    fn rejects_impolite_rate_limit() {
        let mut c = Config::default();
        c.acquire.per_host_delay_ms = 10;
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_bad_bind_address() {
        let mut c = Config::default();
        c.server.bind = "not-an-address".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn db_path_defaults_under_the_data_dir() {
        let c = Config::default();
        assert_eq!(c.db_path(), PathBuf::from("./data/jobseeker.db"));
    }

    #[test]
    fn worker_concurrency_is_clamped() {
        let c = Config::default();
        let n = c.worker_concurrency();
        assert!((2..=8).contains(&n), "got {n}");
    }
}
