//! Tokenization and similarity.
//!
//! Used for in-job requirement deduplication, cross-post detection, and skill alias
//! lookup. The tokenizer is intentionally crude — it exists to make two spellings of the
//! same phrase collide, not to do linguistics.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::BTreeSet;

/// Words carrying no discriminating signal in a job posting. Kept small on purpose: an
/// aggressive stoplist merges requirements that are genuinely different ("experience with
/// Kubernetes" vs "experience without Kubernetes" is not a risk, but "5 years" vs "no
/// years" is).
pub const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "have", "in", "including",
    "into", "is", "it", "its", "of", "on", "or", "our", "that", "the", "their", "to", "with",
    "you", "your", "we", "us", "will", "must", "should", "able", "ability", "strong", "solid",
    "excellent", "good", "great", "proven", "demonstrated", "experience", "experienced",
    "knowledge", "understanding", "familiarity", "familiar", "working", "work", "plus", "using",
    "use", "skills", "skill",
];

static WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"[a-z0-9][a-z0-9+#._-]*").unwrap());

/// Lowercase, strip accents, collapse whitespace. The entry point for every comparison key.
pub fn normalize(input: &str) -> String {
    deunicode_lower(input).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn deunicode_lower(input: &str) -> String {
    // Smart quotes and dashes are common in postings pasted out of Word; fold them before
    // tokenizing so "don't" and "don’t" agree.
    let folded: String = input
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2013}' | '\u{2014}' | '\u{2212}' | '\u{2012}' => '-',
            '\u{00A0}' | '\u{2007}' | '\u{202F}' => ' ',
            '\u{2022}' | '\u{00B7}' | '\u{25CF}' | '\u{25AA}' | '\u{2043}' => ' ',
            other => other,
        })
        .collect();
    deunicode::deunicode(&folded).to_lowercase()
}

/// Content words, in order, with stopwords removed.
pub fn tokenize(input: &str) -> Vec<String> {
    let lowered = deunicode_lower(input);
    WORD.find_iter(&lowered)
        .map(|m| m.as_str().trim_matches(|c: char| c == '.' || c == '-' || c == '_'))
        .filter(|t| !t.is_empty() && !STOPWORDS.contains(t))
        .map(str::to_string)
        .collect()
}

/// The comparison key stored in `requirement.normalized_text`: content words, deduplicated
/// and sorted, so word order does not defeat dedup.
pub fn comparison_key(input: &str) -> String {
    let set: BTreeSet<String> = tokenize(input).into_iter().collect();
    set.into_iter().collect::<Vec<_>>().join(" ")
}

/// Jaccard similarity over token sets, in `0.0..=1.0`.
pub fn jaccard(a: &str, b: &str) -> f32 {
    let (a, b): (BTreeSet<_>, BTreeSet<_>) =
        (tokenize(a).into_iter().collect(), tokenize(b).into_iter().collect());
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let union = a.union(&b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(&b).count() as f32 / union as f32
}

/// Cosine similarity over token *counts*, which is what `dedupe_job` stage 2 compares for
/// descriptions: repetition carries signal that a set membership test throws away.
pub fn token_cosine(a: &str, b: &str) -> f32 {
    use std::collections::BTreeMap;
    let count = |s: &str| -> BTreeMap<String, f32> {
        let mut m = BTreeMap::new();
        for t in tokenize(s) {
            *m.entry(t).or_insert(0.0) += 1.0;
        }
        m
    };
    let (a, b) = (count(a), count(b));
    let dot: f32 = a.iter().map(|(k, v)| b.get(k).copied().unwrap_or(0.0) * v).sum();
    let na: f32 = a.values().map(|v| v * v).sum::<f32>().sqrt();
    let nb: f32 = b.values().map(|v| v * v).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    (dot / (na * nb)).clamp(0.0, 1.0)
}

/// Cosine over embedding vectors. Mismatched lengths mean the vectors came from different
/// models and must not be compared.
pub fn vector_cosine(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    (na > 0.0 && nb > 0.0).then(|| (dot / (na * nb)).clamp(-1.0, 1.0))
}

/// Collapse runs of blank lines and trailing whitespace so generated Markdown is stable
/// byte-for-byte across re-exports (`docs/11-file-layout.md`).
pub fn tidy_markdown(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut blank_run = 0usize;
    for line in input.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(trimmed);
        out.push('\n');
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Plain-text projection of Markdown, for full-text search and embeddings. Strips the
/// syntax without trying to render: link text is kept, targets are dropped.
pub fn markdown_to_text(md: &str) -> String {
    static LINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
    static MARKS: Lazy<Regex> = Lazy::new(|| Regex::new(r"[*_`>#]+").unwrap());
    static BULLET: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\s*([-*+]|\d+\.)\s+").unwrap());
    let s = LINK.replace_all(md, "$1");
    let s = BULLET.replace_all(&s, "");
    let s = MARKS.replace_all(&s, "");
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Truncate on a word boundary, appending an ellipsis. Used for excerpts and for capping
/// LLM input.
pub fn truncate_words(input: &str, max_chars: usize) -> String {
    if input.len() <= max_chars {
        return input.to_string();
    }
    let cut = input
        .char_indices()
        .take_while(|(i, _)| *i < max_chars)
        .map(|(i, _)| i)
        .last()
        .unwrap_or(0);
    let slice = &input[..cut];
    let boundary = slice.rfind(char::is_whitespace).unwrap_or(cut);
    format!("{}…", slice[..boundary].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_folds_case_accents_and_smart_punctuation() {
        assert_eq!(normalize("  Sénior   Engineer "), "senior engineer");
        assert_eq!(normalize("don\u{2019}t"), "don't");
        assert_eq!(normalize("A\u{2013}B"), "a-b");
    }

    #[test]
    fn tokenizing_keeps_technology_names_intact() {
        assert_eq!(tokenize("C++ and C# with .NET"), vec!["c++", "c#", "net"]);
        assert_eq!(tokenize("Node.js / Next.js"), vec!["node.js", "next.js"]);
    }

    #[test]
    fn stopwords_are_dropped_so_phrasing_does_not_defeat_dedup() {
        assert_eq!(
            comparison_key("Experience with Kubernetes in production"),
            comparison_key("Production Kubernetes experience")
        );
    }

    #[test]
    fn comparison_keys_are_order_independent_but_content_sensitive() {
        assert_eq!(comparison_key("rust and python"), comparison_key("python and rust"));
        assert_ne!(comparison_key("rust"), comparison_key("python"));
    }

    #[test]
    fn jaccard_reports_partial_overlap() {
        assert_eq!(jaccard("senior platform engineer", "senior platform engineer"), 1.0);
        assert_eq!(jaccard("rust", "python"), 0.0);
        let partial = jaccard("senior platform engineer", "staff platform engineer");
        assert!(partial > 0.3 && partial < 0.7, "got {partial}");
    }

    #[test]
    fn cosine_weighs_repetition_that_jaccard_ignores() {
        let a = "rust rust rust kubernetes";
        let b = "rust kubernetes kubernetes kubernetes";
        assert_eq!(jaccard(a, b), 1.0, "identical token sets");
        assert!(token_cosine(a, b) < 1.0, "different emphasis must be detectable");
    }

    #[test]
    fn vectors_of_different_models_are_not_comparable() {
        assert_eq!(vector_cosine(&[1.0, 0.0], &[1.0, 0.0, 0.0]), None);
        assert_eq!(vector_cosine(&[], &[]), None);
        let same = vector_cosine(&[1.0, 0.0], &[1.0, 0.0]).unwrap();
        assert!((same - 1.0).abs() < 1e-6);
        let orth = vector_cosine(&[1.0, 0.0], &[0.0, 1.0]).unwrap();
        assert!(orth.abs() < 1e-6);
    }

    #[test]
    fn markdown_is_tidied_to_a_stable_form() {
        assert_eq!(tidy_markdown("a\n\n\n\nb   \n\n\n"), "a\n\nb\n");
    }

    #[test]
    fn markdown_becomes_searchable_text() {
        let md = "## Required\n\n- **5+ years** of [Rust](https://rust-lang.org)\n- `Kubernetes`\n";
        assert_eq!(markdown_to_text(md), "Required\n5+ years of Rust\nKubernetes");
    }

    #[test]
    fn truncation_respects_word_boundaries() {
        assert_eq!(truncate_words("short", 40), "short");
        let out = truncate_words("the quick brown fox jumps over the lazy dog", 20);
        assert!(out.ends_with('…'), "got {out}");
        assert!(!out.contains("jumps"), "must not cut mid-word: {out}");
    }
}
