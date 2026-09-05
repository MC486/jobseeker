//! Requirement atomization.
//!
//! Turning a wall of prose into one row per demand is the transform the whole product rests
//! on: matching, gap analysis and resume targeting are all set operations over these rows
//! (`docs/06-extraction.md` §4).
//!
//! Two rules keep this honest:
//!
//! 1. **A compound bullet is split.** "5+ years of Python and Kubernetes" is two demands
//!    with one shared year count, and reporting it as one makes the gap analysis useless.
//! 2. **Necessity comes from the heading, not from the sentence.** Postings put the same
//!    phrasing under "Requirements" and "Nice to have", and the heading is the only
//!    reliable signal of which it is.

use jobseeker_core::domain::enums::{Necessity, RequirementKind};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::seniority::{detect_clearance, detect_education, extract_years};
use crate::text::comparison_key;

/// One atomized demand, before it is given an id and a job.
#[derive(Debug, Clone, PartialEq)]
pub struct AtomizedRequirement {
    /// Verbatim source text, so the UI can quote what the posting actually said.
    pub text: String,
    pub normalized_text: String,
    pub kind: RequirementKind,
    pub necessity: Necessity,
    pub min_years: Option<f32>,
    pub max_years: Option<f32>,
    pub education_level: Option<jobseeker_core::domain::enums::EducationLevel>,
    /// A categorical disqualifier rather than a graded gap.
    pub is_blocker: bool,
    pub quantity_raw: Option<String>,
    /// Character offsets into the source Markdown, so the UI can highlight the origin.
    pub source_span: Option<(usize, usize)>,
}

/// Which section of a posting a bullet came from, which is what determines necessity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Required,
    Preferred,
    NiceToHave,
    Responsibilities,
    Benefits,
    /// Prose outside any recognized heading. Bullets here are recorded but not treated as
    /// hard requirements, because we cannot tell.
    Unknown,
}

impl Section {
    pub fn necessity(self) -> Option<Necessity> {
        Some(match self {
            Section::Required => Necessity::Required,
            Section::Preferred => Necessity::Preferred,
            Section::NiceToHave => Necessity::NiceToHave,
            Section::Unknown => Necessity::Preferred,
            // Responsibilities and benefits are not demands on the candidate.
            Section::Responsibilities | Section::Benefits => return None,
        })
    }
}

/// Classify a Markdown heading into a section.
pub fn classify_heading(heading: &str) -> Section {
    static NICE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(nice[- ]to[- ]have|bonus|plus(?:es)?|icing|extra credit|would be (?:a )?plus)\b").unwrap()
    });
    static PREFERRED: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(preferred|desired|desirable|ideal|we'?d love|additional|not required but)\b").unwrap()
    });
    static REQUIRED: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(requirements?|qualifications?|must[- ]haves?|what (?:you|we)'?ll need|who you are|what we'?re looking for|minimum|basic qualifications|skills? (?:and|&) experience|about you)\b").unwrap()
    });
    static RESPONSIBILITIES: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(responsibilit\w+|what you'?ll do|the role|day[- ]to[- ]day|your impact|duties|about the (?:role|job)|in this role)\b").unwrap()
    });
    static BENEFITS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(benefits?|perks?|what we offer|compensation|why (?:join|work)|our offer)\b").unwrap()
    });

    // Order matters: "Preferred Qualifications" matches both PREFERRED and REQUIRED, and it
    // is preferred. "Nice to have" is checked first for the same reason.
    if NICE.is_match(heading) {
        Section::NiceToHave
    } else if PREFERRED.is_match(heading) {
        Section::Preferred
    } else if BENEFITS.is_match(heading) {
        Section::Benefits
    } else if RESPONSIBILITIES.is_match(heading) {
        Section::Responsibilities
    } else if REQUIRED.is_match(heading) {
        Section::Required
    } else {
        Section::Unknown
    }
}

/// Walk a job description in Markdown and produce one requirement per demand.
///
/// Deduplicates on [`comparison_key`], because postings repeat the same demand under
/// "Requirements" and again in a summary paragraph.
pub fn atomize(description_md: &str) -> Vec<AtomizedRequirement> {
    static HEADING: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s{0,3}#{1,6}\s+(.*)$").unwrap());
    static BULLET: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^\s{0,8}(?:[-*+•]|\d{1,2}[.)])\s+(.*)$").unwrap());
    static BOLD_HEADING: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^\s*\*\*(.+?)\*\*:?\s*$").unwrap());

    let mut out: Vec<AtomizedRequirement> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut section = Section::Unknown;
    let mut offset = 0usize;

    for line in description_md.lines() {
        let line_start = offset;
        offset += line.len() + 1;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Many postings use bold text where a heading belongs; both must be recognized or
        // every bullet lands in `Unknown`.
        if let Some(h) = HEADING
            .captures(line)
            .or_else(|| BOLD_HEADING.captures(line))
        {
            section = classify_heading(&h[1]);
            continue;
        }

        let Some(b) = BULLET.captures(line) else { continue };
        let Some(necessity) = section.necessity() else { continue };
        let group = b.get(1).expect("the bullet pattern always captures its body");
        let bullet = group.as_str().trim();
        if bullet.len() < 8 {
            continue;
        }

        // Offset of the bullet *text*, past the list marker, so a highlight in the UI covers
        // the requirement rather than the punctuation in front of it.
        let bullet_offset = line_start + group.start();
        for atom in split_compound(bullet) {
            let key = comparison_key(&atom);
            if key.is_empty() || !seen.insert(key.clone()) {
                continue;
            }
            let years = extract_years(&atom);
            let clearance = detect_clearance(&atom);
            let kind = classify_kind(&atom, clearance.is_some());
            out.push(AtomizedRequirement {
                normalized_text: key,
                kind,
                necessity,
                min_years: years.map(|(min, _)| min),
                max_years: years.and_then(|(_, max)| max),
                education_level: (kind == RequirementKind::Education)
                    .then(|| detect_education(&atom))
                    .flatten(),
                // A clearance or work-authorization demand is categorical: no amount of
                // skill overlap substitutes for it (`docs/07-matching.md`).
                is_blocker: necessity == Necessity::Required
                    && (clearance.is_some() || is_authorization_demand(&atom)),
                quantity_raw: years.map(|_| atom.clone()).and_then(|_| quantity_phrase(&atom)),
                source_span: Some((bullet_offset, bullet_offset + bullet.len())),
                text: atom,
            });
        }
    }
    out
}

/// Split a compound bullet into its constituent demands.
///
/// The hard part is knowing when *not* to split: `"Python and Django"` is two skills, but
/// `"CI/CD and release engineering"` is one topic, and `"design and build systems"` is one
/// responsibility. The conservative rule below only splits when both sides look like
/// independent, nameable demands.
pub fn split_compound(bullet: &str) -> Vec<String> {
    // A slash only separates alternatives when it is spaced. Unspaced slashes are part of
    // the term itself — `TS/SCI`, `CI/CD`, `C/C++` — and splitting them produces fragments
    // that mean nothing and, worse, lose a clearance blocker.
    static SPLIT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\s*(?:;|,\s*(?:and|or)\s+|\s+and\s+|\s+or\s+|\s+/\s+)\s*").unwrap());

    let cleaned = bullet.trim().trim_end_matches(['.', ';', ',']).to_string();
    // A bullet that reads as a sentence is a responsibility, not a list of skills; splitting
    // it produces nonsense fragments.
    if cleaned.split_whitespace().count() > 24 || !SPLIT.is_match(&cleaned) {
        return vec![cleaned];
    }

    let prefix = shared_prefix(&cleaned);
    let body = prefix
        .as_ref()
        .map(|p| cleaned[p.len()..].trim().to_string())
        .unwrap_or_else(|| cleaned.clone());

    let parts: Vec<String> = SPLIT
        .split(&body)
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();

    // Only split when every part is short enough to be a name rather than a clause.
    let all_atomic = parts.len() > 1
        && parts.iter().all(|p| {
            let words = p.split_whitespace().count();
            words >= 1 && words <= 5
        });
    if !all_atomic {
        return vec![cleaned];
    }

    match prefix {
        // "5+ years of Python and Kubernetes" → the year count applies to each part.
        Some(p) => parts.iter().map(|part| format!("{p} {part}")).collect(),
        None => parts,
    }
}

/// A quantity or experience clause that governs the whole bullet, e.g.
/// `"5+ years of experience with"`. Returned so it can be re-attached to each split part.
fn shared_prefix(bullet: &str) -> Option<String> {
    static PREFIX: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?i)^\s*(?:(?:at least|minimum(?: of)?|min\.?)\s+)?\d{1,2}\s*\+?\s*(?:-|–|to)?\s*\d{0,2}\s*years?(?:\s+of)?(?:\s+(?:hands[- ]on|professional|industry|relevant|proven|demonstrated))*(?:\s+experience)?(?:\s+(?:with|in|using|building|developing|operating|of))?\s*",
        )
        .unwrap()
    });
    PREFIX
        .find(bullet)
        .map(|m| m.as_str().trim_end().to_string())
        .filter(|p| !p.is_empty() && p.len() < bullet.len())
}

fn quantity_phrase(text: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(?:at least\s+|minimum(?: of)?\s+)?\d{1,2}\s*\+?\s*(?:-|–|to)?\s*\d{0,2}\s*years?\b").unwrap()
    });
    RE.find(text).map(|m| m.as_str().trim().to_string())
}

/// What kind of demand a bullet expresses. Drives which matcher runs and how heavily the
/// requirement is weighted.
pub fn classify_kind(text: &str, has_clearance: bool) -> RequirementKind {
    static EDUCATION: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(degree|bachelor|master|ph\.?d|doctorate|b\.?s\.?c?\.?|m\.?s\.?c?\.?|mba|graduate|university|college|high school|ged)\b").unwrap()
    });
    static CERTIFICATION: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(certified|certification|certificate|licen[cs]e[d]?|cissp|pmp|cpa|aws certified|ckad|cka|comptia|scrum master)\b").unwrap()
    });
    static LANGUAGE_HUMAN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(fluent|fluency|native speaker|bilingual|proficiency in (?:english|spanish|french|german|mandarin|japanese|portuguese))\b").unwrap()
    });
    static LOGISTICS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(travel|relocat\w+|on[- ]call|willing to work|shift|weekend|overtime|driver'?s licen[cs]e|must (?:be able to )?(?:lift|commute)|authorized to work|work authorization|visa|sponsorship|eligible to work)\b").unwrap()
    });
    static SOFT: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(communicat\w+|collaborat\w+|team player|self[- ]starter|attention to detail|interpersonal|passion\w*|motivated|organiz\w+ skills|problem[- ]solving|work independently|detail[- ]oriented|fast[- ]paced)\b").unwrap()
    });
    static TOOL: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(jira|confluence|git|github|gitlab|docker|kubernetes|terraform|ansible|jenkins|datadog|grafana|prometheus|figma|salesforce|excel|tableau|airflow|dbt|splunk)\b").unwrap()
    });
    static DOMAIN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(fintech|healthcare|robotics|e-?commerce|ad ?tech|biotech|gaming|insurance|banking|logistics|manufacturing|aerospace|defen[cs]e|hipaa|pci|sox|gdpr|fedramp)\b").unwrap()
    });
    static RESPONSIBILITY: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^(?:you (?:will|'ll)|we (?:will|'ll)|help |drive |own |lead |partner |collaborate |design and |build and |work with (?:cross|stake))").unwrap()
    });
    static EXPERIENCE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b\d{1,2}\s*\+?\s*years?\b").unwrap());

    if has_clearance {
        return RequirementKind::Clearance;
    }
    if LOGISTICS.is_match(text) {
        return RequirementKind::Logistics;
    }
    if EDUCATION.is_match(text) {
        return RequirementKind::Education;
    }
    if CERTIFICATION.is_match(text) {
        return RequirementKind::Certification;
    }
    if LANGUAGE_HUMAN.is_match(text) {
        return RequirementKind::Language;
    }
    if SOFT.is_match(text) {
        return RequirementKind::SoftSkill;
    }
    if DOMAIN.is_match(text) {
        return RequirementKind::Domain;
    }
    if TOOL.is_match(text) {
        return RequirementKind::Tool;
    }
    if RESPONSIBILITY.is_match(text.trim()) {
        return RequirementKind::Responsibility;
    }
    if EXPERIENCE.is_match(text) {
        return RequirementKind::Experience;
    }
    RequirementKind::Skill
}

/// Work-authorization demands are blockers in the same way a clearance is: categorical, and
/// not fixable by being a better engineer.
fn is_authorization_demand(text: &str) -> bool {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(authorized to work|work authorization|citizen(?:ship)?|permanent resident|green card|no (?:visa )?sponsorship|us person)\b").unwrap()
    });
    RE.is_match(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POSTING: &str = "\
## About the role

You will own the ingestion platform.

## Requirements

- 5+ years building distributed systems in a systems language
- Production Rust or C++
- 3+ years operating Kubernetes
- Bachelor's degree in Computer Science
- Must hold an active TS/SCI clearance
- Excellent communication skills

## Nice to have

- Experience with robotics or real-time systems

## Responsibilities

- Design and build the event ingestion pipeline
- Partner with the hardware team

## Benefits

- Unlimited PTO
- 401(k) with 4% match
";

    fn atoms() -> Vec<AtomizedRequirement> {
        atomize(POSTING)
    }

    #[test]
    fn headings_are_classified_including_the_confusing_ones() {
        assert_eq!(classify_heading("Requirements"), Section::Required);
        assert_eq!(classify_heading("Minimum Qualifications"), Section::Required);
        assert_eq!(classify_heading("What you'll need"), Section::Required);
        // This one matches both "preferred" and "qualifications"; preferred must win.
        assert_eq!(classify_heading("Preferred Qualifications"), Section::Preferred);
        assert_eq!(classify_heading("Nice to have"), Section::NiceToHave);
        assert_eq!(classify_heading("Bonus points"), Section::NiceToHave);
        assert_eq!(classify_heading("What you'll do"), Section::Responsibilities);
        assert_eq!(classify_heading("Benefits & Perks"), Section::Benefits);
    }

    #[test]
    fn necessity_comes_from_the_heading_not_the_sentence() {
        let a = atoms();
        let kube = a.iter().find(|r| r.text.contains("Kubernetes")).unwrap();
        assert_eq!(kube.necessity, Necessity::Required);

        let robotics = a.iter().find(|r| r.text.contains("robotics")).unwrap();
        assert_eq!(robotics.necessity, Necessity::NiceToHave);
    }

    #[test]
    fn benefits_and_responsibilities_are_not_requirements() {
        let a = atoms();
        assert!(
            !a.iter().any(|r| r.text.contains("PTO") || r.text.contains("401")),
            "benefits are not demands on the candidate: {a:#?}"
        );
        assert!(
            !a.iter().any(|r| r.text.contains("Partner with the hardware")),
            "responsibilities describe the job, not a bar to clear"
        );
    }

    #[test]
    fn year_counts_are_extracted_from_the_bullet() {
        let a = atoms();
        let kube = a.iter().find(|r| r.text.contains("Kubernetes")).unwrap();
        assert_eq!(kube.min_years, Some(3.0));
        assert_eq!(kube.quantity_raw.as_deref(), Some("3+ years"));
    }

    #[test]
    fn a_compound_bullet_splits_and_keeps_the_shared_year_count() {
        let split = split_compound("5+ years of experience with Python and Kubernetes");
        assert_eq!(split.len(), 2, "two demands, not one: {split:?}");
        assert!(split.iter().all(|s| s.contains("5+ years")), "got {split:?}");
        assert!(split.iter().any(|s| s.contains("Python")));
        assert!(split.iter().any(|s| s.contains("Kubernetes")));
    }

    #[test]
    fn a_prose_sentence_is_not_shredded_into_fragments() {
        // Splitting this on "and" would produce two meaningless half-sentences.
        let long = "Design and build the event ingestion pipeline that powers our fleet of \
                    warehouse robots, working closely with the hardware and firmware teams to \
                    define the interface contracts";
        assert_eq!(split_compound(long).len(), 1, "a clause must stay whole");
    }

    #[test]
    fn alternatives_split_but_paired_terms_do_not() {
        assert_eq!(split_compound("Production Rust or C++").len(), 2);
        let ci = split_compound(
            "Own the continuous integration and release engineering process for the platform team",
        );
        assert_eq!(ci.len(), 1, "one topic expressed as a phrase: {ci:?}");
    }

    #[test]
    fn an_unspaced_slash_is_part_of_the_term_not_a_separator() {
        // Splitting these was silently destroying a clearance blocker: "Must hold an active
        // TS/SCI" became "Must hold an active TS" plus a stray "SCI clearance".
        assert_eq!(split_compound("Must hold an active TS/SCI clearance").len(), 1);
        assert_eq!(split_compound("CI/CD pipeline ownership").len(), 1);
        assert_eq!(split_compound("Production C/C++").len(), 1);
        // A spaced slash really is a list of alternatives.
        assert_eq!(split_compound("Rust / Go / C++").len(), 3);
    }

    #[test]
    fn kinds_are_classified_so_weighting_can_differ() {
        let a = atoms();
        let find = |needle: &str| a.iter().find(|r| r.text.contains(needle)).unwrap().kind;
        assert_eq!(find("Kubernetes"), RequirementKind::Tool);
        assert_eq!(find("degree"), RequirementKind::Education);
        assert_eq!(find("TS/SCI"), RequirementKind::Clearance);
        assert_eq!(find("communication"), RequirementKind::SoftSkill);
        assert_eq!(find("robotics"), RequirementKind::Domain);
    }

    #[test]
    fn clearance_requirements_are_marked_as_hard_blockers() {
        let a = atoms();
        let clearance = a.iter().find(|r| r.text.contains("TS/SCI")).unwrap();
        assert!(clearance.is_blocker, "a clearance is categorical, not a graded gap");

        let kube = a.iter().find(|r| r.text.contains("Kubernetes")).unwrap();
        assert!(!kube.is_blocker, "a missing skill is a gap you can close");
    }

    #[test]
    fn work_authorization_is_also_a_blocker() {
        let reqs = atomize("## Requirements\n\n- Must be authorized to work in the US without sponsorship\n");
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].is_blocker);
        assert_eq!(reqs[0].kind, RequirementKind::Logistics);
    }

    #[test]
    fn repeated_demands_are_deduplicated() {
        let md = "## Requirements\n\n- Experience with Kubernetes in production\n\
                  - Production Kubernetes experience\n";
        let reqs = atomize(md);
        assert_eq!(reqs.len(), 1, "the same demand phrased twice is one row: {reqs:#?}");
    }

    #[test]
    fn bold_pseudo_headings_are_recognized() {
        // Plenty of postings never use a real Markdown heading.
        let md = "**Requirements**\n\n- 4+ years of Go\n\n**Nice to have**\n\n- Kafka experience\n";
        let reqs = atomize(md);
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].necessity, Necessity::Required);
        assert_eq!(reqs[1].necessity, Necessity::NiceToHave);
    }

    #[test]
    fn source_spans_point_back_into_the_description() {
        let md = "## Requirements\n\n- 4+ years of Go\n";
        let reqs = atomize(md);
        let (start, end) = reqs[0].source_span.unwrap();
        assert_eq!(&md[start..end], "4+ years of Go", "the span must cover the text, not the marker");
    }

    #[test]
    fn an_empty_or_headingless_description_yields_nothing_alarming() {
        assert!(atomize("").is_empty());
        // Bullets with no heading are recorded as preferred rather than required: we cannot
        // tell, and over-claiming "required" would distort every score.
        let loose = atomize("- Some familiarity with Rust\n");
        assert_eq!(loose.len(), 1);
        assert_eq!(loose[0].necessity, Necessity::Preferred);
    }
}
