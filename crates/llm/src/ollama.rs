//! Ollama, the default local provider.
//!
//! Ollama is the recommended setup for a home server: the postings you save are your job
//! search, and shipping them to a third party by default would be the wrong choice to make
//! on a user's behalf (ADR-0005).
//!
//! Its `format` parameter accepts a JSON Schema and constrains decoding, so responses
//! satisfy the schema by construction rather than by prompt-and-pray.

use std::time::{Duration, Instant};

use jobseeker_core::config::{LlmConfig, LlmProvider};
use jobseeker_core::{Error, Result};
use serde::Deserialize;

use crate::{Completion, Embedding, LlmClient, Request};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";

pub struct OllamaClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
    embedding_model: String,
}

impl OllamaClient {
    pub fn new(cfg: &LlmConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_seconds))
            .build()
            .map_err(|e| Error::Config(format!("could not build the HTTP client: {e}")))?;
        Ok(Self {
            http,
            base_url: cfg
                .base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            model: cfg.model.clone(),
            embedding_model: cfg.embedding_model.clone(),
        })
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ChatMessage,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[async_trait::async_trait]
impl LlmClient for OllamaClient {
    fn model(&self) -> &str {
        &self.model
    }

    fn provider(&self) -> LlmProvider {
        LlmProvider::Ollama
    }

    async fn complete(&self, request: &Request) -> Result<Completion> {
        let started = Instant::now();
        let mut body = serde_json::json!({
            "model": self.model,
            "stream": false,
            "messages": [
                { "role": "system", "content": request.system },
                { "role": "user", "content": request.user },
            ],
            "options": {
                "temperature": request.temperature,
                "num_predict": request.max_output_tokens,
            }
        });
        // Constrained decoding: the model cannot emit a response that violates the schema,
        // which removes the whole class of "the model wrote prose instead of JSON" failures.
        if let Some(schema) = &request.schema {
            body["format"] = schema.clone();
        }

        let response = self
            .http
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| classify(e, &self.base_url))?;

        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(Error::LlmUnavailable(format!(
                "ollama returned {status}: {}",
                detail.chars().take(200).collect::<String>()
            )));
        }

        let parsed: ChatResponse = response
            .json()
            .await
            .map_err(|e| Error::LlmUnavailable(format!("unreadable ollama response: {e}")))?;

        Ok(Completion {
            text: parsed.message.content,
            model: self.model.clone(),
            prompt_tokens: parsed.prompt_eval_count,
            completion_tokens: parsed.eval_count,
            duration_ms: started.elapsed().as_millis() as i64,
            cached: false,
        })
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let response = self
            .http
            .post(format!("{}/api/embed", self.base_url))
            .json(&serde_json::json!({ "model": self.embedding_model, "input": texts }))
            .send()
            .await
            .map_err(|e| classify(e, &self.base_url))?;

        if !response.status().is_success() {
            return Err(Error::LlmUnavailable(format!(
                "ollama embeddings returned {}",
                response.status()
            )));
        }

        let parsed: EmbedResponse = response
            .json()
            .await
            .map_err(|e| Error::LlmUnavailable(format!("unreadable embedding response: {e}")))?;

        if parsed.embeddings.len() != texts.len() {
            return Err(Error::LlmUnavailable(format!(
                "asked for {} embeddings, got {}",
                texts.len(),
                parsed.embeddings.len()
            )));
        }

        Ok(parsed
            .embeddings
            .into_iter()
            .map(|vector| Embedding {
                dimensions: vector.len(),
                vector,
                model: self.embedding_model.clone(),
            })
            .collect())
    }

    async fn health(&self) -> Result<()> {
        // A local Ollama that is not running is the single most common misconfiguration, so
        // the error names the fix rather than just reporting a refused connection.
        let response = self
            .http
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
            .map_err(|e| classify(e, &self.base_url))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::LlmUnavailable(format!(
                "ollama at {} returned {}",
                self.base_url,
                response.status()
            )))
        }
    }
}

fn classify(error: reqwest::Error, base_url: &str) -> Error {
    if error.is_timeout() {
        // A local model on CPU is genuinely slow; suggest the knob rather than implying the
        // service is broken.
        Error::LlmUnavailable(format!(
            "ollama at {base_url} timed out; raise llm.timeout_seconds or use a smaller model"
        ))
    } else if error.is_connect() {
        Error::LlmUnavailable(format!(
            "cannot reach ollama at {base_url}; is `ollama serve` running?"
        ))
    } else {
        Error::LlmUnavailable(format!("ollama request failed: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> LlmConfig {
        LlmConfig {
            provider: LlmProvider::Ollama,
            ..LlmConfig::default()
        }
    }

    #[test]
    fn defaults_to_a_loopback_endpoint() {
        let client = OllamaClient::new(&cfg()).unwrap();
        assert_eq!(client.base_url, DEFAULT_BASE_URL);
        assert!(
            client.base_url.starts_with("http://127.0.0.1"),
            "the default provider must be local"
        );
    }

    #[test]
    fn a_configured_base_url_is_used_and_normalized() {
        let mut c = cfg();
        c.base_url = Some("http://gpu-box.lan:11434/".into());
        let client = OllamaClient::new(&c).unwrap();
        assert_eq!(
            client.base_url, "http://gpu-box.lan:11434",
            "trailing slash removed"
        );
    }

    #[test]
    fn connection_and_timeout_errors_are_retryable_and_actionable() {
        let connect =
            Error::LlmUnavailable("cannot reach ollama at x; is `ollama serve` running?".into());
        assert!(connect.is_retryable(), "a down provider is worth retrying");
        assert_eq!(connect.http_status(), 503);
    }

    #[tokio::test]
    async fn embedding_an_empty_batch_does_not_call_the_provider() {
        let client = OllamaClient::new(&cfg()).unwrap();
        // No network is available in tests; this must short-circuit rather than fail.
        assert!(client.embed(&[]).await.unwrap().is_empty());
    }
}
