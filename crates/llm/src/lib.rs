//! Language model access, behind a trait.
//!
//! Three commitments shape this crate (ADR-0005):
//!
//! 1. **Local-first.** The default provider is `None`; nothing leaves the machine unless the
//!    user configures a provider. A cloud provider is a deliberate, logged choice.
//! 2. **Schema-constrained output.** Every call declares the JSON shape it expects and
//!    validates the response. A model that returns prose gets one repair attempt, then
//!    fails the field rather than corrupting the record.
//! 3. **Caching by content.** Identical (model, prompt, schema) triples resolve to the same
//!    hash, so re-extracting a capture after an adapter fix costs no tokens.
//!
//! The `Mock` provider is not a testing afterthought: it is what makes the extraction
//! pipeline testable end to end without a GPU, and it is used by the integration suite.

pub mod cache;
pub mod mock;
pub mod ollama;
pub mod openai;
pub mod prompt;

use std::sync::Arc;

use jobseeker_core::config::{LlmConfig, LlmProvider};
use jobseeker_core::hash::hash_parts;
use jobseeker_core::{Error, Result};
use serde::{Deserialize, Serialize};

/// What a completion is *for*. Recorded on every call so cost can be attributed, and so a
/// user can route resume writing to a local model while extraction uses a cloud one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// Fill fields the deterministic stages could not.
    ExtractFields,
    /// Break prose into atomized requirements.
    AtomizeRequirements,
    /// Parse an uploaded resume into experience items.
    ParseResume,
    /// Rephrase an existing accomplishment for a target job. Never invents facts.
    PhraseBullet,
    /// Prose summary of a score that was computed arithmetically.
    Narrate,
    CoverLetter,
    InterviewPrep,
    Embedding,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::ExtractFields => "extract_fields",
            Purpose::AtomizeRequirements => "atomize_requirements",
            Purpose::ParseResume => "parse_resume",
            Purpose::PhraseBullet => "phrase_bullet",
            Purpose::Narrate => "narrate",
            Purpose::CoverLetter => "cover_letter",
            Purpose::InterviewPrep => "interview_prep",
            Purpose::Embedding => "embedding",
        }
    }

    /// Purposes whose output is used verbatim in a document the user sends to an employer.
    /// These get the strictest no-new-facts validation (`docs/08-resume.md`).
    pub fn is_user_facing_prose(self) -> bool {
        matches!(
            self,
            Purpose::PhraseBullet | Purpose::CoverLetter | Purpose::Narrate
        )
    }
}

/// One completion request.
#[derive(Debug, Clone)]
pub struct Request {
    pub purpose: Purpose,
    pub system: String,
    pub user: String,
    /// JSON Schema the response must satisfy. Providers that support constrained decoding
    /// pass it through; the rest get it appended to the prompt and validated after.
    pub schema: Option<serde_json::Value>,
    pub temperature: f32,
    pub max_output_tokens: u32,
}

impl Request {
    pub fn new(purpose: Purpose, system: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            purpose,
            system: system.into(),
            user: user.into(),
            schema: None,
            // Extraction is a reading task, not a creative one: near-zero temperature makes
            // re-runs reproducible, which the eval harness depends on.
            temperature: 0.0,
            max_output_tokens: 2048,
        }
    }

    pub fn with_schema(mut self, schema: serde_json::Value) -> Self {
        self.schema = Some(schema);
        self
    }

    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = t.clamp(0.0, 2.0);
        self
    }

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_output_tokens = n;
        self
    }

    /// Cache key: everything that could change the answer, and nothing that could not.
    /// The purpose is included so a prompt reused for two purposes does not collide.
    pub fn cache_key(&self, model: &str) -> String {
        hash_parts(&[
            model,
            self.purpose.as_str(),
            &self.system,
            &self.user,
            &self
                .schema
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            &format!("{:.2}", self.temperature),
        ])
    }
}

/// A completion, with the accounting a self-hosted user needs to see.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completion {
    pub text: String,
    pub model: String,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub duration_ms: i64,
    /// True when this came from the cache and cost nothing.
    pub cached: bool,
}

impl Completion {
    /// Parse the response as JSON, tolerating the wrappers models add.
    pub fn json(&self) -> Result<serde_json::Value> {
        parse_lenient_json(&self.text)
    }

    /// Deserialize into a concrete type. A schema violation is reported as such rather than
    /// as a generic parse error, because the two are handled differently: a violation is
    /// worth one repair attempt, a transport error is worth a retry.
    pub fn parse<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        let value = self.json()?;
        serde_json::from_value(value)
            .map_err(|e| Error::SchemaViolation(format!("{e} in response: {}", truncate(&self.text))))
    }
}

/// An embedding vector plus the model that produced it. The model is part of the record
/// because vectors from different models are not comparable, and silently mixing them
/// produces similarity scores that look plausible and mean nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub model: String,
    pub dimensions: usize,
}

#[async_trait::async_trait]
pub trait LlmClient: Send + Sync {
    /// Model identifier, recorded on every row this call produces.
    fn model(&self) -> &str;

    fn provider(&self) -> LlmProvider;

    async fn complete(&self, request: &Request) -> Result<Completion>;

    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>>;

    /// Whether embeddings are available. Callers must degrade gracefully rather than
    /// failing: the semantic subscore is optional and renormalized away when absent
    /// (`docs/07-matching.md`).
    fn supports_embeddings(&self) -> bool {
        true
    }

    /// Cheap liveness check for `/readyz` and for the config screen. A local Ollama that is
    /// not running is the most common misconfiguration, and it should be visible.
    async fn health(&self) -> Result<()> {
        Ok(())
    }
}

/// Build a client from configuration.
///
/// Returns `None` for `LlmProvider::None`, which is the default. Every caller must handle
/// that case: the pipeline is required to work with deterministic stages alone, marking the
/// record `extraction_partial` rather than failing (FR-E-08).
pub fn from_config(cfg: &LlmConfig) -> Result<Option<Arc<dyn LlmClient>>> {
    let client: Arc<dyn LlmClient> = match cfg.provider {
        LlmProvider::None => return Ok(None),
        LlmProvider::Mock => Arc::new(mock::MockClient::new(&cfg.model)),
        LlmProvider::Ollama => Arc::new(ollama::OllamaClient::new(cfg)?),
        LlmProvider::OpenAi => Arc::new(openai::OpenAiClient::openai(cfg)?),
        LlmProvider::Anthropic => Arc::new(openai::OpenAiClient::anthropic(cfg)?),
    };
    if cfg.cache {
        return Ok(Some(Arc::new(cache::Cached::new(client))));
    }
    Ok(Some(client))
}

/// Models wrap JSON in prose, in fenced code blocks, or in both. This recovers the object
/// without being fooled by braces inside string literals.
pub fn parse_lenient_json(text: &str) -> Result<serde_json::Value> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str(trimmed) {
        return Ok(v);
    }

    // ```json … ``` fences.
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let after = after.strip_prefix("json").unwrap_or(after);
        if let Some(end) = after.find("```") {
            if let Ok(v) = serde_json::from_str(after[..end].trim()) {
                return Ok(v);
            }
        }
    }

    if let Some(slice) = first_balanced_json(trimmed) {
        if let Ok(v) = serde_json::from_str(slice) {
            return Ok(v);
        }
    }

    Err(Error::SchemaViolation(format!(
        "response was not JSON: {}",
        truncate(trimmed)
    )))
}

/// Scan for the first balanced `{…}` or `[…]`, tracking string state so a brace inside a
/// description does not end the object early.
fn first_balanced_json(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let open = bytes.iter().position(|b| *b == b'{' || *b == b'[')?;
    let (opener, closer) = if bytes[open] == b'{' {
        (b'{', b'}')
    } else {
        (b'[', b']')
    };

    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, b) in bytes.iter().enumerate().skip(open) {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match *b {
            b'"' => in_string = true,
            b if b == opener => depth += 1,
            b if b == closer => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn truncate(s: &str) -> String {
    if s.len() <= 300 {
        s.to_string()
    } else {
        format!("{}…", &s[..s.floor_char_boundary(300)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_json_parses() {
        let c = completion(r#"{"title":"Engineer"}"#);
        assert_eq!(c.json().unwrap()["title"], "Engineer");
    }

    #[test]
    fn fenced_json_parses() {
        let c = completion("Here you go:\n```json\n{\"title\":\"Engineer\"}\n```\nHope that helps!");
        assert_eq!(c.json().unwrap()["title"], "Engineer");
    }

    #[test]
    fn json_embedded_in_prose_is_recovered() {
        let c = completion("Sure. {\"title\":\"Engineer\"} Let me know if you need more.");
        assert_eq!(c.json().unwrap()["title"], "Engineer");
    }

    #[test]
    fn braces_inside_strings_do_not_truncate_the_object() {
        let c = completion(r#"{"note":"salary is {redacted}","title":"Engineer"}"#);
        let v = c.json().unwrap();
        assert_eq!(v["title"], "Engineer", "the object must not end at the inner brace");
        assert_eq!(v["note"], "salary is {redacted}");
    }

    #[test]
    fn escaped_quotes_inside_strings_are_handled() {
        let c = completion(r#"{"note":"they said \"yes\"","ok":true}"#);
        assert_eq!(c.json().unwrap()["ok"], true);
    }

    #[test]
    fn arrays_are_recovered_too() {
        let c = completion("```\n[{\"text\":\"5+ years of Rust\"}]\n```");
        assert_eq!(c.json().unwrap()[0]["text"], "5+ years of Rust");
    }

    #[test]
    fn prose_with_no_json_is_a_schema_violation_not_a_parse_error() {
        let err = completion("I'm sorry, I can't help with that.").json().unwrap_err();
        assert_eq!(err.code(), "schema_violation", "so the caller knows to repair, not retry");
    }

    #[test]
    fn unbalanced_json_is_rejected_rather_than_guessed_at() {
        assert!(completion(r#"{"title":"Engineer""#).json().is_err());
    }

    #[test]
    fn identical_requests_share_a_cache_key_and_different_ones_do_not() {
        let a = Request::new(Purpose::ExtractFields, "sys", "user");
        let b = Request::new(Purpose::ExtractFields, "sys", "user");
        assert_eq!(a.cache_key("m"), b.cache_key("m"));

        let other_model = a.cache_key("other");
        assert_ne!(a.cache_key("m"), other_model, "a different model is a different answer");

        let other_purpose = Request::new(Purpose::Narrate, "sys", "user");
        assert_ne!(
            a.cache_key("m"),
            other_purpose.cache_key("m"),
            "the same prompt for another purpose must not collide"
        );

        let hotter = Request::new(Purpose::ExtractFields, "sys", "user").with_temperature(0.9);
        assert_ne!(a.cache_key("m"), hotter.cache_key("m"));
    }

    #[test]
    fn extraction_defaults_to_deterministic_decoding() {
        assert_eq!(
            Request::new(Purpose::ExtractFields, "s", "u").temperature,
            0.0,
            "re-running the eval harness must reproduce the same output"
        );
    }

    #[test]
    fn prose_purposes_are_flagged_for_strict_validation() {
        assert!(Purpose::PhraseBullet.is_user_facing_prose());
        assert!(Purpose::CoverLetter.is_user_facing_prose());
        assert!(!Purpose::ExtractFields.is_user_facing_prose());
    }

    #[test]
    fn the_default_configuration_produces_no_client_at_all() {
        let cfg = LlmConfig::default();
        assert_eq!(cfg.provider, LlmProvider::None);
        assert!(
            from_config(&cfg).unwrap().is_none(),
            "nothing may leave the machine unless the user asked for it"
        );
    }

    #[test]
    fn truncation_does_not_panic_on_multibyte_text() {
        let long = "é".repeat(400);
        let _ = truncate(&long);
    }

    fn completion(text: &str) -> Completion {
        Completion {
            text: text.to_string(),
            model: "test".into(),
            prompt_tokens: None,
            completion_tokens: None,
            duration_ms: 0,
            cached: false,
        }
    }
}
