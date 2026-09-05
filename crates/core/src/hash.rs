//! Content hashing.
//!
//! BLAKE3 everywhere, prefixed `b3:` so a stored hash is self-describing if we ever change
//! algorithms. Hashes are how the system achieves idempotency: identical captures, identical
//! LLM prompts and unchanged job records all collapse to the same key.

/// Hash arbitrary bytes: `b3:<64 hex chars>`.
pub fn content_hash(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}

/// Hash a list of fields with an unambiguous separator, so `["ab","c"]` and `["a","bc"]`
/// do not collide.
pub fn hash_parts(parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("b3:{}", hasher.finalize().to_hex())
}

/// Short prefix for display and for the two-character sharding directories used by the
/// content-addressed blob store.
pub fn short_hash(hash: &str) -> &str {
    let hex = hash.strip_prefix("b3:").unwrap_or(hash);
    &hex[..hex.len().min(12)]
}

/// Two-character shard directory for a content-addressed path.
pub fn shard(hash: &str) -> &str {
    let hex = hash.strip_prefix("b3:").unwrap_or(hash);
    &hex[..hex.len().min(2)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_prefixed_and_stable() {
        let h = content_hash(b"hello");
        assert!(h.starts_with("b3:"));
        assert_eq!(h, content_hash(b"hello"));
        assert_ne!(h, content_hash(b"hellp"));
    }

    #[test]
    fn part_boundaries_are_unambiguous() {
        assert_ne!(hash_parts(&["ab", "c"]), hash_parts(&["a", "bc"]));
    }

    #[test]
    fn shard_and_short_strip_the_prefix() {
        let h = content_hash(b"x");
        assert_eq!(shard(&h).len(), 2);
        assert_eq!(short_hash(&h).len(), 12);
        assert!(!shard(&h).contains(':'));
    }
}
