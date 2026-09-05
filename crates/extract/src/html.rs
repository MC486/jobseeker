//! HTML helpers.
//!
//! Captured pages are noisy: navigation, cookie banners, "similar jobs" rails. Extraction
//! reads the document once here so every stage sees the same parse.

use scraper::{Html, Selector};

pub fn parse(html: &str) -> Html {
    Html::parse_document(html)
}

/// Visible text of the page, preferring `<main>` / `[role=main]` / a job container when
/// one exists so a 4,000-word sidebar does not drown the posting.
pub fn main_text(document: &Html) -> String {
    const CANDIDATES: &[&str] = &[
        "main",
        "[role=main]",
        "[data-job]",
        ".job-description",
        "#job-description",
        ".job__description",
        "article",
        "body",
    ];
    for sel in CANDIDATES {
        if let Ok(selector) = Selector::parse(sel) {
            if let Some(node) = document.select(&selector).next() {
                let text = collect_text(node);
                if text.len() >= 80 || *sel == "body" {
                    return text;
                }
            }
        }
    }
    String::new()
}

fn collect_text(node: scraper::ElementRef<'_>) -> String {
    node.text()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A conservative HTML → Markdown conversion. The goal is a readable, grepable artifact
/// and a stable input for atomization, not a perfect rendering of the original page.
pub fn to_markdown(html: &str) -> String {
    let document = Html::parse_fragment(html);
    let mut out = String::new();
    walk(document.root_element(), &mut out, 0);
    jobseeker_normalize::text::tidy_markdown(&out)
}

fn walk(node: scraper::ElementRef<'_>, out: &mut String, list_depth: usize) {
    for child in node.children() {
        if let Some(el) = scraper::ElementRef::wrap(child) {
            let name = el.value().name();
            match name {
                "script" | "style" | "nav" | "footer" | "noscript" | "svg" => continue,
                "h1" | "h2" | "h3" | "h4" => {
                    let level = name.as_bytes()[1] - b'0';
                    let hashes = "#".repeat(level as usize);
                    let text = inner_text(el);
                    if !text.is_empty() {
                        out.push_str(&format!("\n\n{hashes} {text}\n\n"));
                    }
                }
                "p" | "div" | "section" => {
                    walk(el, out, list_depth);
                    out.push_str("\n\n");
                }
                "br" => out.push('\n'),
                "li" => {
                    let indent = "  ".repeat(list_depth);
                    out.push_str(&format!("\n{indent}- "));
                    walk(el, out, list_depth);
                }
                "ul" | "ol" => walk(el, out, list_depth + 1),
                "strong" | "b" => {
                    let text = inner_text(el);
                    if !text.is_empty() {
                        out.push_str(&format!("**{text}**"));
                    }
                }
                "em" | "i" => {
                    let text = inner_text(el);
                    if !text.is_empty() {
                        out.push_str(&format!("*{text}*"));
                    }
                }
                "a" => {
                    let text = inner_text(el);
                    let href = el.value().attr("href").unwrap_or("");
                    if text.is_empty() {
                        continue;
                    }
                    if href.is_empty() {
                        out.push_str(&text);
                    } else {
                        out.push_str(&format!("[{text}]({href})"));
                    }
                }
                _ => walk(el, out, list_depth),
            }
        } else if let Some(text) = child.value().as_text() {
            let t = text.text.trim();
            if !t.is_empty() {
                if !out.is_empty() && !out.ends_with(['\n', ' ', '-']) {
                    out.push(' ');
                }
                out.push_str(t);
            }
        }
    }
}

fn inner_text(node: scraper::ElementRef<'_>) -> String {
    node.text()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// First `<h1>` on the page, which is the title on most ATS templates.
pub fn first_heading(document: &Html) -> Option<String> {
    let selector = Selector::parse("h1").ok()?;
    document
        .select(&selector)
        .next()
        .map(inner_text)
        .filter(|s| !s.is_empty() && s.len() < 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_headings_survive_as_markdown() {
        let md = to_markdown(
            "<h2>Requirements</h2><ul><li>5+ years of Rust</li><li>Kubernetes</li></ul>",
        );
        assert!(md.contains("## Requirements"), "got {md}");
        assert!(md.contains("- 5+ years of Rust"), "got {md}");
        assert!(md.contains("- Kubernetes"), "got {md}");
    }

    #[test]
    fn scripts_and_nav_are_dropped() {
        let md = to_markdown("<nav>Home</nav><script>alert(1)</script><p>Hello</p>");
        assert!(md.contains("Hello"));
        assert!(!md.contains("Home"));
        assert!(!md.contains("alert"));
    }

    #[test]
    fn the_first_heading_is_the_title_candidate() {
        let doc =
            parse("<html><body><h1>Senior Platform Engineer</h1><h1>Ignore</h1></body></html>");
        assert_eq!(
            first_heading(&doc).as_deref(),
            Some("Senior Platform Engineer")
        );
    }
}
