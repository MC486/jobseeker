//! A deterministic offline provider.
//!
//! This is load-bearing infrastructure, not a stub. The integration suite and the CI
//! extraction fixtures run against it, which means the whole pipeline — capture through
//! score — is exercised on every commit without a GPU, an API key, or a network
//! (`docs/15-testing.md`).
//!
//! Responses are keyed by [`Purpose`] and shaped to satisfy the same schemas a real model
//! must satisfy, so a prompt/schema mismatch fails here first.

use std::collections::HashMap;
use std::sync::Mutex;

use jobseeker_core::config::LlmProvider;
use jobseeker_core::hash::hash_parts;
use jobseeker_core::{Error, Result};

use crate::{Completion, Embedding, LlmClient, Purpose, Request};

pub struct MockClient {
    model: String,
    /// Exact-prompt overrides, so a test can pin one specific answer.
    scripted: Mutex<HashMap<String, String>>,
    /// Every request seen, so a test can assert *that* the model was consulted — and, more
    /// often, that it was not, because a deterministic stage already had the answer.
    calls: Mutex<Vec<(Purpose, String)>>,
    fail_next: Mutex<bool>,
}

impl MockClient {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            scripted: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            fail_next: Mutex::new(false),
        }
    }

    /// Pin a response for any request whose user prompt contains `needle`.
    pub fn script(&self, needle: &str, response: &str) {
        self.scripted
            .lock()
            .expect("mock lock")
            .insert(needle.to_string(), response.to_string());
    }

    /// Make the next call fail, to exercise the graceful-degradation path where extraction
    /// completes with `extraction_partial = true`.
    pub fn fail_once(&self) {
        *self.fail_next.lock().expect("mock lock") = true;
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().expect("mock lock").len()
    }

    pub fn calls_for(&self, purpose: Purpose) -> usize {
        self.calls
            .lock()
            .expect("mock lock")
            .iter()
            .filter(|(p, _)| *p == purpose)
            .count()
    }

    pub fn reset(&self) {
        self.calls.lock().expect("mock lock").clear();
    }

    /// A canned response per purpose, shaped like the real schema.
    fn canned(&self, request: &Request) -> String {
        match request.purpose {
            Purpose::ExtractFields => serde_json::json!({
                "title": "Senior Platform Engineer",
                "company_name": "Acme Robotics",
                "work_mode": "remote",
                "employment_type": "full_time",
                "seniority": "senior",
                "locations": ["Remote - US"],
                "salary_text": "$185,000 - $225,000 per year",
                "posted_at": "2026-09-02",
                "confidence": 0.82
            })
            .to_string(),
            Purpose::AtomizeRequirements => serde_json::json!({
                "requirements": [
                    {
                        "text": "5+ years building distributed systems",
                        "kind": "experience",
                        "necessity": "required",
                        "min_years": 5.0,
                        "skill": "distributed-systems",
                        "confidence": 0.9
                    },
                    {
                        "text": "Production Rust",
                        "kind": "skill",
                        "necessity": "required",
                        "skill": "rust",
                        "confidence": 0.88
                    }
                ]
            })
            .to_string(),
            Purpose::ParseResume => serde_json::json!({
                "profile": { "full_name": "Test Candidate", "headline": "Platform Engineer" },
                "experience": [{
                    "kind": "role",
                    "org": "Previous Corp",
                    "title": "Senior Engineer",
                    "start_date": "2021-03",
                    "end_date": "2026-08",
                    "accomplishments": [
                        { "text": "Cut p95 ingestion latency by 85%", "impact_metric": "p95 latency", "impact_value": 85.0, "impact_unit": "%" }
                    ]
                }]
            })
            .to_string(),
            // Phrasing must not introduce facts, so the mock echoes rather than embellishes.
            Purpose::PhraseBullet => serde_json::json!({
                "text": request.user.lines().last().unwrap_or_default().trim(),
                "changed": false
            })
            .to_string(),
            Purpose::Narrate => serde_json::json!({
                "narrative": "Strong overlap on the required platform skills; the stated Kubernetes depth is the main gap."
            })
            .to_string(),
            Purpose::CoverLetter => serde_json::json!({
                "body_md": "Dear Hiring Manager,\n\nI am writing about the platform engineering role.\n\nSincerely,\nTest Candidate"
            })
            .to_string(),
            Purpose::InterviewPrep => serde_json::json!({
                "questions": [
                    { "question": "Walk me through the ingestion rewrite.", "why": "Their core problem is throughput." }
                ]
            })
            .to_string(),
            Purpose::Embedding => "{}".to_string(),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for MockClient {
    fn model(&self) -> &str {
        &self.model
    }

    fn provider(&self) -> LlmProvider {
        LlmProvider::Mock
    }

    async fn complete(&self, request: &Request) -> Result<Completion> {
        self.calls
            .lock()
            .expect("mock lock")
            .push((request.purpose, request.user.clone()));

        {
            let mut fail = self.fail_next.lock().expect("mock lock");
            if *fail {
                *fail = false;
                return Err(Error::LlmUnavailable(
                    "mock failure requested by test".into(),
                ));
            }
        }

        let scripted = {
            let map = self.scripted.lock().expect("mock lock");
            map.iter()
                .find(|(needle, _)| request.user.contains(needle.as_str()))
                .map(|(_, response)| response.clone())
        };

        Ok(Completion {
            text: scripted.unwrap_or_else(|| self.canned(request)),
            model: self.model.clone(),
            prompt_tokens: Some((request.system.len() + request.user.len()) as u32 / 4),
            completion_tokens: Some(64),
            duration_ms: 0,
            cached: false,
        })
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>> {
        // Hash-derived pseudo-vectors: deterministic, unit-length, and similar for similar
        // strings only by accident. Good enough to exercise the plumbing, and deliberately
        // not good enough to be mistaken for real semantics in a test assertion.
        const DIMS: usize = 64;
        Ok(texts
            .iter()
            .map(|t| {
                let seed = hash_parts(&[t]);
                let bytes = seed.as_bytes();
                let mut v: Vec<f32> = (0..DIMS)
                    .map(|i| {
                        let b = bytes[(i * 3 + 3) % bytes.len()] as f32;
                        (b / 128.0) - 1.0
                    })
                    .collect();
                let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                if norm > 0.0 {
                    for x in &mut v {
                        *x /= norm;
                    }
                }
                Embedding {
                    vector: v,
                    model: self.model.clone(),
                    dimensions: DIMS,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> MockClient {
        MockClient::new("mock-1")
    }

    #[tokio::test]
    async fn canned_responses_satisfy_the_schema_the_pipeline_expects() {
        let c = client();
        let r = Request::new(Purpose::ExtractFields, "sys", "the posting");
        let json = c.complete(&r).await.unwrap().json().unwrap();
        assert_eq!(json["title"], "Senior Platform Engineer");
        assert!(json["confidence"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn requirement_atomization_returns_a_list() {
        let c = client();
        let r = Request::new(Purpose::AtomizeRequirements, "sys", "bullets");
        let json = c.complete(&r).await.unwrap().json().unwrap();
        assert_eq!(json["requirements"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn scripted_responses_override_the_canned_one() {
        let c = client();
        c.script("SPECIAL", r#"{"title":"Scripted"}"#);
        let r = Request::new(Purpose::ExtractFields, "sys", "a SPECIAL posting");
        assert_eq!(
            c.complete(&r).await.unwrap().json().unwrap()["title"],
            "Scripted"
        );

        let other = Request::new(Purpose::ExtractFields, "sys", "an ordinary posting");
        assert_eq!(
            c.complete(&other).await.unwrap().json().unwrap()["title"],
            "Senior Platform Engineer"
        );
    }

    #[tokio::test]
    async fn calls_are_counted_so_a_test_can_assert_the_model_was_not_needed() {
        let c = client();
        assert_eq!(c.call_count(), 0);
        c.complete(&Request::new(Purpose::ExtractFields, "s", "u"))
            .await
            .unwrap();
        c.complete(&Request::new(Purpose::Narrate, "s", "u"))
            .await
            .unwrap();
        assert_eq!(c.call_count(), 2);
        assert_eq!(c.calls_for(Purpose::ExtractFields), 1);
        assert_eq!(c.calls_for(Purpose::ParseResume), 0);
    }

    #[tokio::test]
    async fn a_scripted_failure_is_retryable_so_the_pipeline_degrades_rather_than_dies() {
        let c = client();
        c.fail_once();
        let err = c
            .complete(&Request::new(Purpose::ExtractFields, "s", "u"))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "llm_unavailable");
        assert!(err.is_retryable());
        // The failure is one-shot: the next call succeeds.
        assert!(c
            .complete(&Request::new(Purpose::ExtractFields, "s", "u"))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn embeddings_are_deterministic_unit_vectors() {
        let c = client();
        let a = c.embed(&["hello".to_string()]).await.unwrap();
        let b = c.embed(&["hello".to_string()]).await.unwrap();
        assert_eq!(
            a[0].vector, b[0].vector,
            "the same text must embed identically"
        );

        let norm: f32 = a[0].vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "expected unit length, got {norm}"
        );
        assert_eq!(a[0].dimensions, a[0].vector.len());

        let different = c.embed(&["goodbye".to_string()]).await.unwrap();
        assert_ne!(a[0].vector, different[0].vector);
    }

    #[tokio::test]
    async fn a_batch_embed_returns_one_vector_per_input() {
        let c = client();
        let out = c
            .embed(&["a".to_string(), "b".to_string(), "c".to_string()])
            .await
            .unwrap();
        assert_eq!(out.len(), 3);
    }

    #[tokio::test]
    async fn phrasing_echoes_rather_than_embellishing() {
        // The mock must not invent facts, because the no-new-facts validator is tested
        // against it and a fabricating mock would make that test vacuous.
        let c = client();
        let r = Request::new(
            Purpose::PhraseBullet,
            "sys",
            "Rebuilt the ingestion pipeline",
        );
        let json = c.complete(&r).await.unwrap().json().unwrap();
        assert_eq!(json["text"], "Rebuilt the ingestion pipeline");
    }
}
