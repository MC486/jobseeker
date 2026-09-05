//! Cloud providers, behind an explicit opt-in.
//!
//! Using one of these means the text of every posting you save — and, for resume writing,
//! your employment history — is sent to a third party. That is a legitimate choice, but it
//! must be a *choice*: the API key comes from the environment, never from a config file
//! that might be committed, and the pipeline logs the egress
//! (`docs/13-security-privacy-legal.md`).
//!
//! Anthropic's Messages API and the OpenAI Chat Completions API differ in enough details
//! (auth header, system prompt placement, response shape) that both are handled here rather
//! than pretending one is a drop-in for the other.

use std::time::{Duration, Instant};

use jobseeker_core::config::{LlmConfig, LlmProvider};
use jobseeker_core::{Error, Result};
use serde::Deserialize;

use crate::{Completion, Embedding, LlmClient, Request};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    OpenAi,
    Anthropic,
}

pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
    embedding_model: String,
    api_key: String,
    dialect: Dialect,
}

impl std::fmt::Debug for OpenAiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiClient")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("dialect", &self.dialect)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl OpenAiClient {
    pub fn openai(cfg: &LlmConfig) -> Result<Self> {
        Self::build(
            cfg,
            Dialect::OpenAi,
            "https://api.openai.com/v1",
            "OPENAI_API_KEY",
        )
    }

    pub fn anthropic(cfg: &LlmConfig) -> Result<Self> {
        Self::build(
            cfg,
            Dialect::Anthropic,
            "https://api.anthropic.com/v1",
            "ANTHROPIC_API_KEY",
        )
    }

    fn build(cfg: &LlmConfig, dialect: Dialect, default_url: &str, key_var: &str) -> Result<Self> {
        // Read from the environment only. A key in a TOML file ends up in a backup, a git
        // history, or a support paste sooner or later.
        let api_key = std::env::var(key_var).map_err(|_| {
            Error::Config(format!(
                "llm.provider requires {key_var} in the environment; \
                 set it in the systemd unit or use llm.provider = \"ollama\" to stay local"
            ))
        })?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_seconds))
            .build()
            .map_err(|e| Error::Config(format!("could not build the HTTP client: {e}")))?;
        Ok(Self {
            http,
            base_url: cfg
                .base_url
                .clone()
                .unwrap_or_else(|| default_url.to_string())
                .trim_end_matches('/')
                .to_string(),
            model: cfg.model.clone(),
            embedding_model: cfg.embedding_model.clone(),
            api_key,
            dialect,
        })
    }

    fn authorize(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.dialect {
            Dialect::OpenAi => builder.bearer_auth(&self.api_key),
            Dialect::Anthropic => builder
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01"),
        }
    }

    fn chat_body(&self, request: &Request) -> serde_json::Value {
        match self.dialect {
            Dialect::OpenAi => {
                let mut body = serde_json::json!({
                    "model": self.model,
                    "temperature": request.temperature,
                    "max_completion_tokens": request.max_output_tokens,
                    "messages": [
                        { "role": "system", "content": request.system },
                        { "role": "user", "content": request.user },
                    ],
                });
                if let Some(schema) = &request.schema {
                    body["response_format"] = serde_json::json!({
                        "type": "json_schema",
                        "json_schema": {
                            "name": request.purpose.as_str(),
                            "strict": false,
                            "schema": schema,
                        }
                    });
                }
                body
            }
            // Anthropic takes the system prompt as a top-level field, and has no structured
            // output mode, so the schema is stated in the prompt and validated on return.
            Dialect::Anthropic => {
                let user = match &request.schema {
                    Some(schema) => format!(
                        "{}\n\nRespond with JSON only, matching this schema:\n{}",
                        request.user, schema
                    ),
                    None => request.user.clone(),
                };
                serde_json::json!({
                    "model": self.model,
                    "system": request.system,
                    "temperature": request.temperature,
                    "max_tokens": request.max_output_tokens,
                    "messages": [{ "role": "user", "content": user }],
                })
            }
        }
    }

    fn chat_path(&self) -> &'static str {
        match self.dialect {
            Dialect::OpenAi => "/chat/completions",
            Dialect::Anthropic => "/messages",
        }
    }
}

#[derive(Deserialize)]
struct OpenAiChat {
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct AnthropicMessage {
    content: Vec<AnthropicBlock>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Deserialize)]
struct AnthropicBlock {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    embedding: Vec<f32>,
    #[serde(default)]
    index: usize,
}

#[async_trait::async_trait]
impl LlmClient for OpenAiClient {
    fn model(&self) -> &str {
        &self.model
    }

    fn provider(&self) -> LlmProvider {
        match self.dialect {
            Dialect::OpenAi => LlmProvider::OpenAi,
            Dialect::Anthropic => LlmProvider::Anthropic,
        }
    }

    async fn complete(&self, request: &Request) -> Result<Completion> {
        let started = Instant::now();
        let response = self
            .authorize(
                self.http
                    .post(format!("{}{}", self.base_url, self.chat_path()))
                    .json(&self.chat_body(request)),
            )
            .send()
            .await
            .map_err(classify)?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let snippet: String = detail.chars().take(200).collect();
            // 429 and 5xx are worth retrying with backoff; a 401 never is, and telling the
            // difference is what keeps a bad key from burning five attempts.
            return Err(match status.as_u16() {
                401 | 403 => Error::Config(format!(
                    "the language model provider rejected the API key: {snippet}"
                )),
                429 => Error::RateLimited(20),
                _ => Error::LlmUnavailable(format!("provider returned {status}: {snippet}")),
            });
        }

        let (text, prompt_tokens, completion_tokens) = match self.dialect {
            Dialect::OpenAi => {
                let parsed: OpenAiChat = response
                    .json()
                    .await
                    .map_err(|e| Error::LlmUnavailable(format!("unreadable response: {e}")))?;
                let text = parsed
                    .choices
                    .first()
                    .and_then(|c| c.message.content.clone())
                    .ok_or_else(|| Error::SchemaViolation("the provider returned no content".into()))?;
                let usage = parsed.usage;
                (
                    text,
                    usage.as_ref().and_then(|u| u.prompt_tokens),
                    usage.and_then(|u| u.completion_tokens),
                )
            }
            Dialect::Anthropic => {
                let parsed: AnthropicMessage = response
                    .json()
                    .await
                    .map_err(|e| Error::LlmUnavailable(format!("unreadable response: {e}")))?;
                let text = parsed
                    .content
                    .iter()
                    .filter_map(|b| b.text.clone())
                    .collect::<Vec<_>>()
                    .join("");
                if text.is_empty() {
                    return Err(Error::SchemaViolation("the provider returned no content".into()));
                }
                let usage = parsed.usage;
                (
                    text,
                    usage.as_ref().and_then(|u| u.input_tokens),
                    usage.and_then(|u| u.output_tokens),
                )
            }
        };

        Ok(Completion {
            text,
            model: self.model.clone(),
            prompt_tokens,
            completion_tokens,
            duration_ms: started.elapsed().as_millis() as i64,
            cached: false,
        })
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if self.dialect == Dialect::Anthropic {
            return Err(Error::LlmUnavailable(
                "this provider has no embeddings endpoint; set llm.embedding_provider to \
                 \"ollama\" to keep semantic search working"
                    .into(),
            ));
        }

        let response = self
            .authorize(
                self.http
                    .post(format!("{}/embeddings", self.base_url))
                    .json(&serde_json::json!({ "model": self.embedding_model, "input": texts })),
            )
            .send()
            .await
            .map_err(classify)?;

        if !response.status().is_success() {
            return Err(Error::LlmUnavailable(format!(
                "embeddings returned {}",
                response.status()
            )));
        }

        let mut parsed: EmbeddingResponse = response
            .json()
            .await
            .map_err(|e| Error::LlmUnavailable(format!("unreadable embedding response: {e}")))?;

        // The API does not guarantee ordering; sorting by index keeps vector *i* matched to
        // text *i*, which a silent mismatch would make impossible to notice.
        parsed.data.sort_by_key(|d| d.index);
        if parsed.data.len() != texts.len() {
            return Err(Error::LlmUnavailable(format!(
                "asked for {} embeddings, got {}",
                texts.len(),
                parsed.data.len()
            )));
        }

        Ok(parsed
            .data
            .into_iter()
            .map(|d| Embedding {
                dimensions: d.embedding.len(),
                vector: d.embedding,
                model: self.embedding_model.clone(),
            })
            .collect())
    }

    fn supports_embeddings(&self) -> bool {
        self.dialect == Dialect::OpenAi
    }
}

fn classify(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::LlmUnavailable("the language model provider timed out".into())
    } else {
        Error::Network(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(provider: LlmProvider) -> LlmConfig {
        LlmConfig {
            provider,
            model: "test-model".into(),
            ..LlmConfig::default()
        }
    }

    /// The environment is process-global, so key-dependent cases are exercised in one test
    /// to keep them from racing each other.
    #[test]
    fn a_missing_api_key_is_a_configuration_error_that_names_the_local_alternative() {
        let saved = std::env::var("OPENAI_API_KEY").ok();
        std::env::remove_var("OPENAI_API_KEY");

        let err = OpenAiClient::openai(&cfg(LlmProvider::OpenAi)).unwrap_err();
        assert_eq!(err.code(), "config_error", "not a runtime failure to retry");
        let message = err.to_string();
        assert!(message.contains("OPENAI_API_KEY"), "got {message}");
        assert!(
            message.contains("ollama"),
            "the error should point at the local option: {message}"
        );

        std::env::set_var("OPENAI_API_KEY", "sk-test");
        let client = OpenAiClient::openai(&cfg(LlmProvider::OpenAi)).unwrap();
        assert_eq!(client.base_url, "https://api.openai.com/v1");
        assert_eq!(client.provider(), LlmProvider::OpenAi);
        assert!(client.supports_embeddings());

        match saved {
            Some(v) => std::env::set_var("OPENAI_API_KEY", v),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
    }

    #[test]
    fn each_dialect_targets_its_own_endpoint_and_shapes_its_own_body() {
        let saved = std::env::var("ANTHROPIC_API_KEY").ok();
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test");

        let anthropic = OpenAiClient::anthropic(&cfg(LlmProvider::Anthropic)).unwrap();
        assert_eq!(anthropic.chat_path(), "/messages");
        assert_eq!(anthropic.provider(), LlmProvider::Anthropic);
        assert!(
            !anthropic.supports_embeddings(),
            "callers must fall back rather than assume embeddings exist"
        );

        let request = Request::new(crate::Purpose::ExtractFields, "be terse", "the posting")
            .with_schema(serde_json::json!({"type": "object"}));
        let body = anthropic.chat_body(&request);
        assert_eq!(body["system"], "be terse", "the system prompt is top-level here");
        assert!(
            body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("schema"),
            "with no structured output mode, the schema goes in the prompt"
        );

        match saved {
            Some(v) => std::env::set_var("ANTHROPIC_API_KEY", v),
            None => std::env::remove_var("ANTHROPIC_API_KEY"),
        }
    }

    #[test]
    fn openai_requests_use_structured_output_when_a_schema_is_given() {
        let saved = std::env::var("OPENAI_API_KEY").ok();
        std::env::set_var("OPENAI_API_KEY", "sk-test");

        let client = OpenAiClient::openai(&cfg(LlmProvider::OpenAi)).unwrap();
        let plain = client.chat_body(&Request::new(crate::Purpose::Narrate, "s", "u"));
        assert!(plain.get("response_format").is_none());

        let constrained = client.chat_body(
            &Request::new(crate::Purpose::ExtractFields, "s", "u")
                .with_schema(serde_json::json!({"type": "object"})),
        );
        assert_eq!(constrained["response_format"]["type"], "json_schema");
        assert_eq!(
            constrained["response_format"]["json_schema"]["name"],
            "extract_fields",
            "the purpose names the schema, which shows up in provider logs"
        );

        match saved {
            Some(v) => std::env::set_var("OPENAI_API_KEY", v),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
    }
}
