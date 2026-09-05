//! Content-addressed completion caching.
//!
//! Re-extracting a capture after fixing an adapter is a routine operation
//! (`jobseeker reextract`), and without a cache it would re-bill every posting in the
//! archive. Keying on the hash of (model, purpose, prompt, schema, temperature) means an
//! unchanged prompt costs nothing.
//!
//! This layer is in-memory and bounded. The durable cache lives in the `llm_call` table,
//! written by the pipeline, which also gives the user a real cost ledger.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use jobseeker_core::config::LlmProvider;
use jobseeker_core::Result;

use crate::{Completion, Embedding, LlmClient, Request};

/// Entries beyond this are evicted oldest-first. Job descriptions are a few kilobytes, so
/// this bounds the cache at a handful of megabytes — trivial next to the token cost it
/// avoids.
const MAX_ENTRIES: usize = 512;

pub struct Cached {
    inner: Arc<dyn LlmClient>,
    entries: Mutex<HashMap<String, Completion>>,
    /// Insertion order, for eviction.
    order: Mutex<Vec<String>>,
    hits: Mutex<u64>,
    misses: Mutex<u64>,
}

impl Cached {
    pub fn new(inner: Arc<dyn LlmClient>) -> Self {
        Self {
            inner,
            entries: Mutex::new(HashMap::new()),
            order: Mutex::new(Vec::new()),
            hits: Mutex::new(0),
            misses: Mutex::new(0),
        }
    }

    /// `(hits, misses)`, surfaced on the admin screen so the user can see what the cache is
    /// saving them.
    pub fn stats(&self) -> (u64, u64) {
        (
            *self.hits.lock().expect("cache lock"),
            *self.misses.lock().expect("cache lock"),
        )
    }

    fn store(&self, key: String, completion: &Completion) {
        let mut entries = self.entries.lock().expect("cache lock");
        let mut order = self.order.lock().expect("cache lock");
        if entries.len() >= MAX_ENTRIES {
            if let Some(oldest) = order.first().cloned() {
                entries.remove(&oldest);
                order.remove(0);
            }
        }
        // A cached completion is marked as such, so a caller can tell a free answer from a
        // billed one and the `llm_call` ledger stays honest.
        entries.insert(
            key.clone(),
            Completion {
                cached: true,
                ..completion.clone()
            },
        );
        order.push(key);
    }
}

#[async_trait::async_trait]
impl LlmClient for Cached {
    fn model(&self) -> &str {
        self.inner.model()
    }

    fn provider(&self) -> LlmProvider {
        self.inner.provider()
    }

    async fn complete(&self, request: &Request) -> Result<Completion> {
        let key = request.cache_key(self.inner.model());

        if let Some(hit) = self.entries.lock().expect("cache lock").get(&key).cloned() {
            *self.hits.lock().expect("cache lock") += 1;
            return Ok(hit);
        }

        let completion = self.inner.complete(request).await?;
        *self.misses.lock().expect("cache lock") += 1;
        self.store(key, &completion);
        Ok(completion)
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>> {
        // Embeddings are cached in the `embedding` table keyed by content hash, which
        // outlives the process; duplicating that here would only add eviction bugs.
        self.inner.embed(texts).await
    }

    fn supports_embeddings(&self) -> bool {
        self.inner.supports_embeddings()
    }

    async fn health(&self) -> Result<()> {
        self.inner.health().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockClient;
    use crate::{Purpose, Request};

    fn cached() -> (Cached, Arc<MockClient>) {
        let mock = Arc::new(MockClient::new("mock-1"));
        (Cached::new(mock.clone()), mock)
    }

    #[tokio::test]
    async fn an_identical_prompt_is_served_without_calling_the_model() {
        let (cache, mock) = cached();
        let r = Request::new(Purpose::ExtractFields, "sys", "a posting");

        let first = cache.complete(&r).await.unwrap();
        assert!(!first.cached);
        assert_eq!(mock.call_count(), 1);

        let second = cache.complete(&r).await.unwrap();
        assert!(second.cached, "the second answer is free");
        assert_eq!(second.text, first.text);
        assert_eq!(
            mock.call_count(),
            1,
            "the model must not be consulted twice"
        );
        assert_eq!(cache.stats(), (1, 1));
    }

    #[tokio::test]
    async fn a_different_prompt_is_a_miss() {
        let (cache, mock) = cached();
        cache
            .complete(&Request::new(Purpose::ExtractFields, "sys", "posting A"))
            .await
            .unwrap();
        cache
            .complete(&Request::new(Purpose::ExtractFields, "sys", "posting B"))
            .await
            .unwrap();
        assert_eq!(mock.call_count(), 2);
        assert_eq!(cache.stats(), (0, 2));
    }

    #[tokio::test]
    async fn the_same_prompt_for_a_different_purpose_is_not_a_hit() {
        let (cache, mock) = cached();
        cache
            .complete(&Request::new(Purpose::ExtractFields, "sys", "text"))
            .await
            .unwrap();
        cache
            .complete(&Request::new(Purpose::Narrate, "sys", "text"))
            .await
            .unwrap();
        assert_eq!(
            mock.call_count(),
            2,
            "purpose is part of the identity of a call"
        );
    }

    #[tokio::test]
    async fn a_failed_call_is_not_cached() {
        let (cache, mock) = cached();
        let r = Request::new(Purpose::ExtractFields, "sys", "a posting");
        mock.fail_once();
        assert!(cache.complete(&r).await.is_err());
        // A transient provider outage must not become a permanently cached failure.
        assert!(cache.complete(&r).await.is_ok());
        assert_eq!(mock.call_count(), 2);
    }

    #[tokio::test]
    async fn the_cache_is_bounded_and_evicts_the_oldest_entry() {
        let (cache, mock) = cached();
        for i in 0..MAX_ENTRIES + 8 {
            cache
                .complete(&Request::new(
                    Purpose::ExtractFields,
                    "sys",
                    format!("posting {i}"),
                ))
                .await
                .unwrap();
        }
        assert!(
            cache.entries.lock().unwrap().len() <= MAX_ENTRIES,
            "the cache must stay bounded"
        );

        // The first prompt was evicted, so it costs a call again.
        let before = mock.call_count();
        cache
            .complete(&Request::new(Purpose::ExtractFields, "sys", "posting 0"))
            .await
            .unwrap();
        assert_eq!(mock.call_count(), before + 1);
    }

    #[tokio::test]
    async fn caching_is_transparent_to_the_model_and_provider_it_wraps() {
        let (cache, _) = cached();
        assert_eq!(cache.model(), "mock-1");
        assert_eq!(cache.provider(), LlmProvider::Mock);
        assert!(cache.supports_embeddings());
    }
}
