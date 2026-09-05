//! Slugs for file paths and company/job identity.
//!
//! Paths are a public contract once M1 ships (CON-07), so slug generation must be stable and
//! deterministic: ASCII-folded, lowercased, non-alphanumerics collapsed, length-capped.

use deunicode::deunicode;

const MAX_SLUG_LEN: usize = 60;

/// `"Acme Robotics, Inc."` → `"acme-robotics-inc"`.
pub fn slugify(input: &str) -> String {
    slugify_max(input, MAX_SLUG_LEN)
}

pub fn slugify_max(input: &str, max_len: usize) -> String {
    let ascii = deunicode(input).to_lowercase();
    let mut out = String::with_capacity(ascii.len());
    let mut last_dash = true; // suppresses a leading dash
    for ch in ascii.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() > max_len {
        out.truncate(max_len);
        // Do not leave a dangling word fragment mid-token if we can avoid it.
        if let Some(pos) = out.rfind('-') {
            if pos > max_len / 2 {
                out.truncate(pos);
            }
        }
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

/// Company names normalized for identity resolution: legal suffixes stripped so
/// `"Acme, Inc."`, `"ACME Inc"` and `"Acme"` collapse to one key (see FR-E-14).
pub fn normalize_company_name(name: &str) -> String {
    const SUFFIXES: &[&str] = &[
        "inc", "inc.", "llc", "l.l.c.", "ltd", "ltd.", "limited", "corp", "corp.",
        "corporation", "co", "co.", "company", "gmbh", "ag", "sa", "s.a.", "bv", "b.v.",
        "nv", "n.v.", "plc", "pty", "ab", "as", "oy", "aps", "srl", "spa", "kk", "pbc",
        "holdings", "group",
    ];
    let ascii = deunicode(name).to_lowercase();
    let cleaned: String = ascii
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c.is_whitespace() || c == '.' { c } else { ' ' })
        .collect();
    let mut tokens: Vec<&str> = cleaned.split_whitespace().collect();
    while let Some(last) = tokens.last() {
        if SUFFIXES.contains(last) {
            tokens.pop();
        } else {
            break;
        }
    }
    // A name made entirely of legal suffixes ("Inc.") is not a suffix — it is the name.
    let kept: Vec<&str> = if tokens.is_empty() {
        cleaned.split_whitespace().collect()
    } else {
        tokens
    };
    kept.join(" ")
        .replace('.', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Job titles normalized for cross-post detection: seniority and level tokens are factored
/// out so `"Senior Platform Engineer II"` and `"Platform Engineer"` compare as related.
pub fn normalize_job_title(title: &str) -> String {
    const NOISE: &[&str] = &[
        "senior", "sr", "junior", "jr", "staff", "principal", "lead", "associate", "i",
        "ii", "iii", "iv", "v", "level", "remote", "hybrid", "onsite", "contract",
        "fulltime", "parttime", "the", "a", "an", "and", "of", "at",
    ];
    let ascii = deunicode(title).to_lowercase();
    let cleaned: String = ascii
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    let mut tokens: Vec<&str> = cleaned
        .split_whitespace()
        .filter(|t| !NOISE.contains(t))
        .collect();
    tokens.sort_unstable();
    tokens.dedup();
    tokens.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_ascii_and_stable() {
        assert_eq!(slugify("Acme Robotics, Inc."), "acme-robotics-inc");
        assert_eq!(slugify("Senior  Platform   Engineer"), "senior-platform-engineer");
        assert_eq!(slugify("Café & Bar"), "cafe-bar");
        assert_eq!(slugify("  ---  "), "untitled");
        assert_eq!(slugify("Sr. Développeur"), "sr-developpeur");
    }

    #[test]
    fn slugs_respect_the_length_cap_without_dangling_dashes() {
        let long = "a-very-long-job-title-that-goes-on-and-on-and-should-be-truncated-somewhere";
        let s = slugify(long);
        assert!(s.len() <= 60, "{} chars", s.len());
        assert!(!s.ends_with('-'));
    }

    #[test]
    fn company_names_collapse_legal_suffixes() {
        let expected = "acme robotics";
        for input in [
            "Acme Robotics",
            "Acme Robotics, Inc.",
            "ACME ROBOTICS INC",
            "Acme Robotics LLC",
            "Acme Robotics GmbH",
            "Acme Robotics Holdings Ltd.",
        ] {
            assert_eq!(normalize_company_name(input), expected, "input: {input}");
        }
    }

    #[test]
    fn company_normalization_survives_suffix_only_names() {
        assert_eq!(normalize_company_name("Inc."), "inc");
    }

    #[test]
    fn titles_normalize_for_cross_post_matching() {
        assert_eq!(
            normalize_job_title("Senior Platform Engineer II"),
            normalize_job_title("Platform Engineer")
        );
        assert_eq!(
            normalize_job_title("Staff Software Engineer, Backend (Remote)"),
            normalize_job_title("Software Engineer - Backend")
        );
        assert_ne!(
            normalize_job_title("Platform Engineer"),
            normalize_job_title("Data Engineer")
        );
    }
}
