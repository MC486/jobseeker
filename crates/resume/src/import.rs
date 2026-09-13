//! Deterministic Markdown → experience-bank parse.
//!
//! No LLM. Headings, dates, and bullets become draft profile fields, experience
//! items, and accomplishments. Review still happens in the UI; this is the
//! bootstrap from `docs/08-resume.md` §2.

use std::collections::{BTreeMap, BTreeSet};

use jobseeker_core::domain::enums::EducationLevel;
use jobseeker_core::domain::profile::{ExperienceKind, Link};
use jobseeker_normalize::skill::extract_all;
use serde::{Deserialize, Serialize};

/// Structured result of parsing a career evidence bank or a conventional resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedBank {
    pub identity: ParsedIdentity,
    pub items: Vec<ParsedItem>,
    pub skills: Vec<ParsedSkill>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedIdentity {
    pub full_name: Option<String>,
    pub headline: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub location: Option<String>,
    pub links: Vec<Link>,
    pub summary_md: Option<String>,
    pub target_titles: Vec<String>,
    pub target_comp_min_cents: Option<i64>,
    pub target_locations: Vec<String>,
    pub accepts_remote: bool,
    pub willing_to_relocate: bool,
    pub work_auth: Option<String>,
    pub citizenship: Option<String>,
    pub clearance_held: Option<String>,
    pub can_obtain_clearance: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedItem {
    pub kind: ExperienceKind,
    pub org: String,
    pub title: Option<String>,
    pub location: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub is_current: bool,
    pub description_md: Option<String>,
    pub accomplishments: Vec<ParsedAccomplishment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedAccomplishment {
    pub text: String,
    pub variants: BTreeMap<String, String>,
    pub strength: i32,
    pub verified: bool,
    pub skill_slugs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedSkill {
    pub slug: String,
    pub years: Option<f32>,
    pub last_used_year: Option<i32>,
    pub is_primary: bool,
    pub evidence_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Unknown,
    Targets,
    Identity,
    Timeline,
    CurrentWork,
    Internship,
    Operations,
    Education,
    Skills,
    Personal,
}

/// Parse Markdown (evidence bank or a conventional resume) into a bank draft.
pub fn parse_markdown(input: &str) -> ParsedBank {
    let lines: Vec<&str> = input.lines().map(str::trim).collect();
    let mut identity = ParsedIdentity {
        accepts_remote: looks_remote(input),
        willing_to_relocate: input.to_ascii_lowercase().contains("willing to relocate"),
        ..ParsedIdentity::default()
    };
    identity.full_name = h1_name(input);
    identity.summary_md = narrative_paragraph(&lines);
    identity.target_comp_min_cents = first_attractive_salary(input);
    identity.location =
        labeled_value(&lines, "Location").or_else(|| table_value(input, "Location"));
    if let Some(loc) = identity.location.clone() {
        identity.target_locations.push(loc);
    }
    if identity.accepts_remote && !identity.target_locations.iter().any(|l| l == "US") {
        identity.target_locations.push("US".into());
    }
    if let Some(name) = table_value(input, "Name") {
        identity.full_name.get_or_insert(name);
    }
    if let Some(phone) = labeled_value(&lines, "Phone").or_else(|| table_value(input, "Phone")) {
        if !is_meta_value(&phone) {
            identity.phone = Some(phone);
        }
    }
    if let Some(email) = first_email(input) {
        if !email.to_ascii_lowercase().contains("disney.com") {
            identity.email = Some(email);
        }
    }
    if let Some(gh) = github_url(input) {
        identity.links.push(Link {
            label: "GitHub".into(),
            url: gh,
        });
    }
    apply_eligibility(&mut identity, &lines, input);

    let mut items: Vec<ParsedItem> = Vec::new();
    let mut current: Option<usize> = None;
    let mut bucket = Bucket::Unknown;
    let mut allow_bullets = false;
    let mut verified_zone = false;
    let mut in_progress_zone = false;
    let mut variant_label: Option<String> = None;

    for raw in lines.iter().copied() {
        if raw.is_empty() {
            continue;
        }
        if let Some((level, title)) = heading(raw) {
            bucket = classify_heading(title, bucket, level);
            allow_bullets = heading_allows_bullets(title, bucket);
            verified_zone = heading_is_verified(title);
            in_progress_zone = heading_is_in_progress(title);
            variant_label = None;
            if let Some(item) = item_from_heading(title, bucket, level) {
                items.push(item);
                current = Some(items.len() - 1);
            } else if let Some(idx) = infer_item(&items, bucket) {
                current = Some(idx);
                if looks_variant_label(title) {
                    variant_label = Some(clean_variant_label(title));
                }
            } else if looks_variant_label(title) {
                variant_label = Some(clean_variant_label(title));
            }
            continue;
        }

        if let Some((start, end, current_flag)) = parse_dates_line(raw) {
            if let Some(idx) = current {
                items[idx].start_date = items[idx].start_date.clone().or(start);
                items[idx].end_date = items[idx].end_date.clone().or(end);
                items[idx].is_current |= current_flag;
            }
            continue;
        }
        if raw.to_ascii_lowercase().starts_with("degree:")
            || raw.to_ascii_lowercase().starts_with("program:")
        {
            if let Some(idx) = current {
                let value = raw.split_once(':').map(|(_, v)| v.trim()).unwrap_or(raw);
                if items[idx].title.is_none() {
                    items[idx].title = Some(value.to_string());
                }
                items[idx].description_md = Some(value.to_string());
            }
            continue;
        }
        if raw.to_ascii_lowercase().starts_with("graduation:") {
            if let Some(idx) = current {
                if let Some((_, _, _)) = parse_dates_line(raw) {
                    /* handled below via month-year */
                }
                if let Some(end) = parse_month_year(
                    raw.split_once(':')
                        .map(|(_, v)| v.trim())
                        .unwrap_or_default(),
                ) {
                    items[idx].end_date = Some(end);
                }
            }
            continue;
        }
        if raw.to_ascii_lowercase().starts_with("status:")
            && raw.to_ascii_lowercase().contains("in progress")
        {
            in_progress_zone = true;
            if let Some(idx) = current {
                items[idx].is_current = true;
            }
            continue;
        }

        if looks_variant_label(raw) {
            variant_label = Some(clean_variant_label(raw));
            continue;
        }

        if bucket == Bucket::Targets {
            if let Some(title) = bullet_text(raw) {
                if is_job_title_candidate(&title) {
                    identity.target_titles.push(title);
                }
            }
            continue;
        }
        if bucket == Bucket::Skills {
            continue;
        }

        let Some(text) = bullet_text(raw) else {
            if bucket == Bucket::Identity {
                continue;
            }
            // A short unlabeled line after a role heading can be a location.
            if let Some(idx) = current {
                if items[idx].location.is_none() && raw.to_ascii_lowercase().contains("orlando") {
                    items[idx].location = Some(raw.to_string());
                }
            }
            continue;
        };

        if !allow_bullets
            || !is_accomplishment(&text)
            || in_progress_zone && is_roadmap_claim(&text)
        {
            continue;
        }

        let idx = match current.or_else(|| infer_item(&items, bucket)) {
            Some(idx) => idx,
            None => continue,
        };
        if in_progress_zone && items[idx].kind != ExperienceKind::Education {
            // Keep education-in-progress, drop "do not claim" outcome bullets.
            if is_roadmap_claim(&text) {
                continue;
            }
        }

        let slugs = extract_all(&text)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let verified = verified_zone || heading_is_verified(&text);
        let strength = if verified {
            5
        } else if looks_quantified(&text) {
            4
        } else {
            3
        };
        if let Some(label) = variant_label.take() {
            if let Some(existing) = items[idx]
                .accomplishments
                .iter_mut()
                .find(|a| similar_bullet(&a.text, &text))
            {
                existing.variants.insert(label, text);
                continue;
            }
            let mut variants = BTreeMap::new();
            variants.insert(label, text.clone());
            items[idx].accomplishments.push(ParsedAccomplishment {
                text,
                variants,
                strength,
                verified,
                skill_slugs: slugs,
            });
            continue;
        }
        if items[idx]
            .accomplishments
            .iter()
            .any(|a| similar_bullet(&a.text, &text))
        {
            continue;
        }
        items[idx].accomplishments.push(ParsedAccomplishment {
            text,
            variants: BTreeMap::new(),
            strength,
            verified,
            skill_slugs: slugs,
        });
    }

    if identity.headline.is_none() {
        identity.headline = identity
            .target_titles
            .first()
            .cloned()
            .or_else(|| items.iter().find_map(|it| it.title.clone()));
    }

    let skills = synthesize_skills(input, &items);
    ParsedBank {
        identity,
        items,
        skills,
    }
}

fn synthesize_skills(input: &str, items: &[ParsedItem]) -> Vec<ParsedSkill> {
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut years: BTreeMap<String, f32> = BTreeMap::new();
    let mut last_year: BTreeMap<String, i32> = BTreeMap::new();
    let today = "2026-09";

    for item in items {
        let tenure = item_tenure_years(item, today).unwrap_or(1.0);
        let end_year = item
            .end_date
            .as_deref()
            .and_then(|d| d.get(..4)?.parse().ok())
            .unwrap_or(2026);
        let used = if item.is_current { 2026 } else { end_year };
        let mut mentioned = BTreeSet::new();
        for acc in &item.accomplishments {
            for slug in &acc.skill_slugs {
                *counts.entry(slug.clone()).or_insert(0) += 1;
                mentioned.insert(slug.clone());
            }
        }
        if item.kind.counts_as_work() {
            for slug in mentioned {
                years
                    .entry(slug.clone())
                    .and_modify(|y| *y = (*y).max(tenure))
                    .or_insert(tenure);
                last_year
                    .entry(slug)
                    .and_modify(|y| *y = (*y).max(used))
                    .or_insert(used);
            }
        }
    }

    for slug in extract_all(input) {
        counts.entry(slug.to_string()).or_insert(0);
        years.entry(slug.to_string()).or_insert(1.0);
        last_year.entry(slug.to_string()).or_insert(2026);
    }

    let mut out: Vec<ParsedSkill> = counts
        .into_iter()
        .map(|(slug, evidence_count)| {
            let is_primary = evidence_count >= 2
                || matches!(
                    slug.as_str(),
                    "python"
                        | "sql"
                        | "snowflake"
                        | "dataiku"
                        | "lightgbm"
                        | "tableau"
                        | "pandas"
                        | "data-science"
                );
            ParsedSkill {
                years: years.get(&slug).copied(),
                last_used_year: last_year.get(&slug).copied(),
                is_primary,
                evidence_count,
                slug,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.evidence_count
            .cmp(&a.evidence_count)
            .then_with(|| a.slug.cmp(&b.slug))
    });
    out
}

pub fn item_tenure_years(item: &ParsedItem, today: &str) -> Option<f32> {
    let start = parse_year_month(item.start_date.as_deref()?)?;
    let end = if item.is_current {
        parse_year_month(today)?
    } else {
        parse_year_month(item.end_date.as_deref()?)?
    };
    let months = (end.0 - start.0) * 12 + (end.1 as i32 - start.1 as i32);
    (months >= 0).then_some(months as f32 / 12.0)
}

pub fn years_experience(items: &[ParsedItem], today: &str) -> f32 {
    items
        .iter()
        .filter(|i| i.kind == ExperienceKind::Role)
        .filter_map(|i| item_tenure_years(i, today))
        .sum()
}

pub fn inferred_education(items: &[ParsedItem]) -> Option<EducationLevel> {
    let mut best = None;
    for item in items.iter().filter(|i| i.kind == ExperienceKind::Education) {
        if item.is_current {
            continue;
        }
        let blob = format!(
            "{} {}",
            item.title.as_deref().unwrap_or(""),
            item.description_md.as_deref().unwrap_or("")
        )
        .to_ascii_lowercase();
        if blob.contains("in progress") {
            continue;
        }
        let level = if blob.contains("phd") || blob.contains("doctor") {
            EducationLevel::Doctorate
        } else if blob.contains("master") {
            EducationLevel::Master
        } else if blob.contains("bachelor") || blob.contains("b.s") || blob.contains("bs ") {
            EducationLevel::Bachelor
        } else {
            continue;
        };
        if best
            .map(|b: EducationLevel| b.rank().unwrap_or(0))
            .unwrap_or(0)
            < level.rank().unwrap_or(0)
        {
            best = Some(level);
        }
    }
    best
}

fn parse_year_month(s: &str) -> Option<(i32, u32)> {
    let mut parts = s.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next().and_then(|m| m.parse().ok()).unwrap_or(1);
    (1..=12).contains(&month).then_some((year, month))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let level = trimmed.bytes().take_while(|b| *b == b'#').count() as u8;
    let title = trimmed[level as usize..].trim();
    (!title.is_empty()).then_some((level, title))
}

fn classify_heading(title: &str, current: Bucket, level: u8) -> Bucket {
    let t = title.to_ascii_lowercase();
    if t.contains("primary target") || t.contains("target family") {
        return Bucket::Targets;
    }
    if t.contains("identity") || t.contains("contact") {
        return Bucket::Identity;
    }
    if t.contains("professional timeline") || t.contains("employment") {
        return Bucket::Timeline;
    }
    if t.contains("current") && (t.contains("data scientist") || t.contains("project inventory"))
        || t.starts_with("4.")
        || t.contains("résumé bullet")
        || t.contains("resume bullet")
        || t.contains("completed proof")
    {
        return Bucket::CurrentWork;
    }
    if t.contains("internship") || t.starts_with("5.") {
        return Bucket::Internship;
    }
    if t.contains("operations") || t.contains("leadership inventory") || t.starts_with("6.") {
        return Bucket::Operations;
    }
    if t.contains("education") || t.starts_with("7.") {
        return Bucket::Education;
    }
    if t.contains("technical capability") || t.contains("skills") || t.starts_with("8.") {
        return Bucket::Skills;
    }
    if t.contains("personal") || t.contains("project inventory") || t.starts_with("10.") {
        return Bucket::Personal;
    }
    if level <= 2 {
        return current;
    }
    current
}

fn heading_allows_bullets(title: &str, bucket: Bucket) -> bool {
    let t = title.to_ascii_lowercase();
    if t.contains("needed for")
        || t.contains("do not")
        || t.contains("roadmap")
        || t.contains("evidence to recover")
        || t.contains("evidence gaps")
        || t.contains("change log")
    {
        return false;
    }
    matches!(
        bucket,
        Bucket::CurrentWork | Bucket::Internship | Bucket::Operations | Bucket::Personal
    ) && (t.contains("proof")
        || t.contains("bullet")
        || t.contains("verified")
        || t.contains("strong resume")
        || t.contains("framing")
        || t.starts_with("5.")
        || t.starts_with("4.")
        || t.starts_with("10.")
        || t.contains("segmentation")
        || t.contains("analysis")
        || t.contains("migration")
        || t.contains("workshop")
        || t.contains("homelab")
        || t.contains("inventory"))
}

fn heading_is_verified(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    t.contains("verified") || t.contains("completed") || t.contains("bullet bank")
}

fn heading_is_in_progress(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    t.contains("in progress") || t.contains("do not claim") || t.contains("roadmap")
}

fn item_from_heading(title: &str, bucket: Bucket, level: u8) -> Option<ParsedItem> {
    if level <= 1 {
        return None;
    }
    let cleaned = strip_section_number(title);
    if matches!(bucket, Bucket::Education)
        && !cleaned.to_ascii_lowercase().contains("education")
        && looks_school(&cleaned)
    {
        return Some(ParsedItem {
            kind: ExperienceKind::Education,
            org: cleaned,
            title: None,
            location: None,
            start_date: None,
            end_date: None,
            is_current: false,
            description_md: None,
            accomplishments: Vec::new(),
        });
    }
    if let Some((role, org)) = split_role_org(&cleaned) {
        if is_status_org(&org) {
            return None;
        }
        let kind = if bucket == Bucket::Personal || cleaned.to_ascii_lowercase().contains("homelab")
        {
            ExperienceKind::Project
        } else {
            ExperienceKind::Role
        };
        // Inventory / bullet-bank headings are not extra jobs.
        if kind == ExperienceKind::Role && bucket != Bucket::Timeline {
            return None;
        }
        return Some(ParsedItem {
            kind,
            org,
            title: Some(role),
            location: None,
            start_date: None,
            end_date: None,
            is_current: false,
            description_md: None,
            accomplishments: Vec::new(),
        });
    }
    if bucket == Bucket::Personal && looks_project_name(&cleaned) {
        return Some(ParsedItem {
            kind: ExperienceKind::Project,
            org: "Personal".into(),
            title: Some(cleaned),
            location: None,
            start_date: None,
            end_date: None,
            is_current: true,
            description_md: None,
            accomplishments: Vec::new(),
        });
    }
    if bucket == Bucket::Education && looks_school(&cleaned) {
        return Some(ParsedItem {
            kind: ExperienceKind::Education,
            org: cleaned,
            title: None,
            location: None,
            start_date: None,
            end_date: None,
            is_current: false,
            description_md: None,
            accomplishments: Vec::new(),
        });
    }
    None
}

fn infer_item(items: &[ParsedItem], bucket: Bucket) -> Option<usize> {
    let want = match bucket {
        Bucket::CurrentWork => "scientist",
        Bucket::Internship => "intern",
        Bucket::Operations => "coordinator",
        Bucket::Personal => {
            return items
                .iter()
                .rposition(|i| i.kind == ExperienceKind::Project)
        }
        _ => return items.iter().rposition(|i| i.kind == ExperienceKind::Role),
    };
    items.iter().position(|i| {
        i.kind == ExperienceKind::Role
            && i.title
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase()
                .contains(want)
    })
}

fn split_role_org(title: &str) -> Option<(String, String)> {
    for sep in [" — ", " – ", " - "] {
        if let Some((left, right)) = title.split_once(sep) {
            let left = left.trim();
            let right = right.trim();
            if left.is_empty() || right.is_empty() {
                continue;
            }
            if looks_school(left) {
                return None;
            }
            return Some((left.to_string(), right.to_string()));
        }
    }
    None
}

fn is_status_org(org: &str) -> bool {
    matches!(
        org.to_ascii_lowercase().as_str(),
        "completed"
            | "in progress"
            | "source-supported"
            | "verified"
            | "candidate"
            | "career evidence bank"
    )
}

fn looks_school(s: &str) -> bool {
    let t = s.to_ascii_lowercase();
    t.contains("university")
        || t.contains("college")
        || t.contains("institute")
        || t.ends_with(" ut ")
}

fn looks_project_name(s: &str) -> bool {
    let t = s.to_ascii_lowercase();
    t.contains("workshop")
        || t.contains("homelab")
        || t.contains("sandbox")
        || t.contains("tessitura")
        || t.contains("sfz")
        || t.contains("platform")
}

fn looks_variant_label(s: &str) -> bool {
    let t = s.to_ascii_lowercase();
    t.contains("data scientist /")
        || t.contains("version")
        || t.ends_with("bullet")
        || t.contains("business-forward")
        || t.contains("ml platform")
}

fn clean_variant_label(s: &str) -> String {
    s.trim_end_matches(':').trim().to_ascii_lowercase()
}

fn bullet_text(line: &str) -> Option<String> {
    let t = line.trim();
    let body = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("• "))?;
    let body = body.trim();
    (!body.is_empty()).then(|| body.to_string())
}

fn is_accomplishment(text: &str) -> bool {
    if text.len() < 40 {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    if META_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return false;
    }
    if lower.starts_with("status") || lower.starts_with("dates:") {
        return false;
    }
    text.chars().next().is_some_and(|c| c.is_uppercase())
}

fn is_roadmap_claim(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("do not claim")
        || t.contains("not yet")
        || t.starts_with("full deployment")
        || t.starts_with("complete asset")
}

fn looks_quantified(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_digit())
}

fn similar_bullet(a: &str, b: &str) -> bool {
    let na = normalize_cmp(a);
    let nb = normalize_cmp(b);
    na == nb || na.contains(&nb) || nb.contains(&na)
}

fn normalize_cmp(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .take(12)
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_job_title_candidate(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    (t.contains("scientist") || t.contains("engineer") || t.contains("analyst"))
        && text.len() < 80
        && !t.starts_with("do not")
}

fn looks_remote(input: &str) -> bool {
    let t = input.to_ascii_lowercase();
    t.contains("preferred: remote") || t.contains("accepts remote") || t.contains("remote")
}

fn h1_name(input: &str) -> Option<String> {
    for line in input.lines() {
        if let Some((1, title)) = heading(line.trim()) {
            let name = title
                .split(" — ")
                .next()
                .or_else(|| title.split(" – ").next())
                .unwrap_or(title)
                .trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn narrative_paragraph(lines: &[&str]) -> Option<String> {
    let mut take = false;
    let mut buf = String::new();
    for line in lines {
        if heading(line).is_some() {
            if take && !buf.is_empty() {
                break;
            }
            take = line.to_ascii_lowercase().contains("career narrative")
                || line.to_ascii_lowercase().contains("summary");
            continue;
        }
        if take && !line.is_empty() && heading(line).is_none() && bullet_text(line).is_none() {
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(line);
        }
    }
    (!buf.is_empty()).then_some(buf)
}

fn labeled_value(lines: &[&str], label: &str) -> Option<String> {
    let needle = label.to_ascii_lowercase();
    for (i, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower
            .strip_prefix(&format!("{needle}:"))
            .or_else(|| lower.strip_prefix(&format!("- {needle}:")))
        {
            let value = line[line.len() - rest.len()..].trim();
            if !value.is_empty() && !is_meta_value(value) {
                return Some(value.to_string());
            }
        }
        if lower == needle || lower == format!("| {needle} |") {
            for next in lines.iter().skip(i + 1) {
                if next.is_empty() || heading(next).is_some() {
                    continue;
                }
                if is_meta_value(next) {
                    continue;
                }
                if next.eq_ignore_ascii_case("field")
                    || next.eq_ignore_ascii_case("current information")
                {
                    continue;
                }
                return Some((*next).to_string());
            }
        }
    }
    None
}

fn table_value(input: &str, field: &str) -> Option<String> {
    let field_l = field.to_ascii_lowercase();
    for line in input.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            continue;
        }
        let cells: Vec<String> = t
            .split('|')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        if cells.len() >= 2 && cells[0].eq_ignore_ascii_case(&field_l) {
            let value = cells[1].clone();
            if !is_meta_value(&value) && !value.contains("---") {
                return Some(value);
            }
        }
    }
    None
}

fn apply_eligibility(identity: &mut ParsedIdentity, lines: &[&str], input: &str) {
    let raw = labeled_value(lines, "Citizenship")
        .or_else(|| table_value(input, "Citizenship"))
        .or_else(|| labeled_value(lines, "Work authorization"));
    if let Some(value) = raw.as_deref().and_then(parse_citizenship) {
        identity.citizenship = Some(value);
    }
    if identity.citizenship.as_deref() == Some("us") {
        identity.work_auth = Some("us_citizen".into());
    }
    if let Some(value) =
        labeled_value(lines, "Clearance").or_else(|| table_value(input, "Clearance"))
    {
        let (held, obtain) = parse_clearance_eligibility(&value);
        identity.clearance_held = held;
        identity.can_obtain_clearance = obtain;
    }
}

fn parse_citizenship(value: &str) -> Option<String> {
    let t = value.to_ascii_lowercase();
    if t.contains("dual")
        && !t.contains("united states")
        && !t.contains("u.s.")
        && !t.contains("us")
    {
        return Some("other".into());
    }
    if t.contains("united states")
        || t.contains("u.s. citizen")
        || t.contains("us citizen")
        || t.contains("usa")
        || t == "us"
        || t.starts_with("us,")
        || t.starts_with("us;")
        || t.contains("born and bred")
    {
        return Some("us".into());
    }
    if t.contains("citizen") || t.contains("green card") || t.contains("permanent resident") {
        return Some("other".into());
    }
    None
}

fn parse_clearance_eligibility(value: &str) -> (Option<String>, Option<bool>) {
    let t = value.to_ascii_lowercase();
    let obtain = if t.contains("eligible")
        || t.contains("can obtain")
        || t.contains("able to obtain")
        || t.contains("no background")
    {
        Some(true)
    } else if t.contains("ineligible") || t.contains("cannot obtain") {
        Some(false)
    } else {
        None
    };
    let held = if let Some(d) = jobseeker_normalize::seniority::detect_clearance_demand(value) {
        if t.contains("none") || t.contains("not held") || t.contains("no clearance") {
            None
        } else {
            Some(d.kind)
        }
    } else {
        None
    };
    (held, obtain)
}

fn is_meta_value(s: &str) -> bool {
    let t = s.to_ascii_lowercase();
    t.starts_with("confirm")
        || t.starts_with("required")
        || t.starts_with("candidate")
        || t.starts_with("do not")
        || t == "verified"
        || t == "status / action"
        || t.contains("---")
}

fn first_email(input: &str) -> Option<String> {
    for token in input.split_whitespace() {
        let t = token.trim_matches(|c: char| {
            !c.is_ascii_alphanumeric() && c != '@' && c != '.' && c != '_' && c != '+' && c != '-'
        });
        if t.contains('@') && t.contains('.') {
            return Some(t.to_string());
        }
    }
    None
}

fn github_url(input: &str) -> Option<String> {
    for token in input.split_whitespace() {
        let t = token.trim_matches(|c: char| c == '|' || c == '(' || c == ')');
        let lower = t.to_ascii_lowercase();
        if lower.contains("github.com/") {
            let path = t
                .trim_start_matches("https://")
                .trim_start_matches("http://");
            return Some(format!("https://{path}"));
        }
    }
    None
}

fn first_attractive_salary(input: &str) -> Option<i64> {
    let mut best = None;
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let mut j = i + 1;
            let mut n: i64 = 0;
            let mut digits = 0;
            while j < bytes.len() {
                let c = bytes[j];
                if c.is_ascii_digit() {
                    n = n.saturating_mul(10).saturating_add((c - b'0') as i64);
                    digits += 1;
                    j += 1;
                } else if c == b',' {
                    j += 1;
                } else {
                    break;
                }
            }
            if digits >= 5 && n >= 80_000 && n <= 400_000 {
                best = Some(best.map_or(n, |b: i64| b.max(n)));
            }
            i = j;
        } else {
            i += 1;
        }
    }
    best.map(|n| n * 100)
}

fn parse_dates_line(line: &str) -> Option<(Option<String>, Option<String>, bool)> {
    let lower = line.to_ascii_lowercase();
    if !(lower.contains("date")
        || lower.contains("–")
        || lower.contains("—")
        || lower.contains('-')
        || lower.contains("present"))
    {
        return None;
    }
    if heading(line).is_some() {
        return None;
    }
    let body = line
        .split_once(':')
        .map(|(_, v)| v.trim())
        .unwrap_or(line)
        .trim();
    let sep = if body.contains('–') {
        '–'
    } else if body.contains('—') {
        '—'
    } else if body.contains('-') {
        '-'
    } else {
        return month_only(body);
    };
    // "January 2026–Present" may use a unicode dash without spaces.
    let parts: Vec<&str> = if body.contains("–") {
        body.split('–').collect()
    } else if body.contains("—") {
        body.split('—').collect()
    } else {
        // Avoid splitting "2026-01". Prefer "2026–" prose form; fall back to last hyphen between words.
        split_prose_range(body, sep)?
    };
    if parts.len() < 2 {
        return month_only(body);
    }
    let start = parse_month_year(parts[0].trim());
    let end_raw = parts[1].trim();
    let current =
        end_raw.eq_ignore_ascii_case("present") || end_raw.eq_ignore_ascii_case("current");
    let end = if current {
        None
    } else {
        parse_month_year(end_raw)
    };
    (start.is_some() || end.is_some()).then_some((start, end, current))
}

fn month_only(body: &str) -> Option<(Option<String>, Option<String>, bool)> {
    parse_month_year(body).map(|d| (Some(d), None, false))
}

fn split_prose_range(body: &str, _sep: char) -> Option<Vec<&str>> {
    // "January 2025-January 2026"
    let idx = body.find(|c: char| c == '-')?;
    if idx == 0 {
        return None;
    }
    Some(vec![&body[..idx], &body[idx + 1..]])
}

fn parse_month_year(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches('.');
    if let Some((y, m)) = parse_year_month(s) {
        return Some(format!("{y:04}-{m:02}"));
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("spring") {
        let year = year_in(&lower)?;
        return Some(format!("{year}-01"));
    }
    if lower.starts_with("fall") || lower.starts_with("autumn") {
        let year = year_in(&lower)?;
        return Some(format!("{year}-09"));
    }
    const MONTHS: [(&str, u32); 12] = [
        ("january", 1),
        ("february", 2),
        ("march", 3),
        ("april", 4),
        ("may", 5),
        ("june", 6),
        ("july", 7),
        ("august", 8),
        ("september", 9),
        ("october", 10),
        ("november", 11),
        ("december", 12),
    ];
    let month = MONTHS
        .iter()
        .find(|(n, _)| lower.starts_with(*n) || lower.contains(&format!(" {n}")))
        .map(|(_, m)| *m)?;
    let year = year_in(&lower)?;
    Some(format!("{year:04}-{month:02}"))
}

fn year_in(s: &str) -> Option<i32> {
    for word in s.split(|c: char| !c.is_ascii_digit()) {
        if word.len() == 4 {
            if let Ok(y) = word.parse::<i32>() {
                if (1990..=2100).contains(&y) {
                    return Some(y);
                }
            }
        }
    }
    None
}

fn strip_section_number(title: &str) -> String {
    let t = title.trim();
    let mut chars = t.chars().peekable();
    let mut saw_digit = false;
    while let Some(c) = chars.peek().copied() {
        if c.is_ascii_digit() || c == '.' {
            saw_digit = true;
            chars.next();
            continue;
        }
        if saw_digit && c.is_whitespace() {
            chars.next();
            continue;
        }
        break;
    }
    chars.collect::<String>().trim().to_string()
}

const META_PREFIXES: &[&str] = &[
    "confirm ",
    "needed for",
    "do not ",
    "capture ",
    "inventory ",
    "mark each",
    "include projects",
    "treat this as",
    "source material",
    "status:",
    "dates:",
    "approximately two years of bona fide",
];

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../fixtures/evidence-bank.md");

    #[test]
    fn evidence_bank_fixture_fills_a_usable_profile() {
        let bank = parse_markdown(FIXTURE);
        assert_eq!(bank.identity.full_name.as_deref(), Some("Alex Rivera"));
        assert!(bank.identity.accepts_remote);
        assert_eq!(bank.identity.target_comp_min_cents, Some(14_500_000));
        assert!(bank
            .identity
            .target_titles
            .iter()
            .any(|t| t.contains("Data Scientist")));
        assert!(
            bank.items.iter().any(|i| {
                i.title.as_deref() == Some("Data Scientist")
                    && i.org.contains("Harbor")
                    && i.is_current
                    && i.start_date.as_deref() == Some("2026-01")
            }),
            "current role: {:?}",
            bank.items
        );
        assert!(bank.items.iter().any(|i| {
            i.title.as_deref() == Some("Decision Science Intern")
                && i.start_date.as_deref() == Some("2025-01")
                && i.end_date.as_deref() == Some("2026-01")
        }));
        assert!(bank.items.iter().any(|i| {
            i.kind == ExperienceKind::Education && i.org.contains("Southern New Hampshire")
        }));
        let bullets: Vec<_> = bank
            .items
            .iter()
            .flat_map(|i| i.accomplishments.iter())
            .map(|a| a.text.as_str())
            .collect();
        assert!(
            bullets.iter().any(|b| b.contains("5,000 datasets")),
            "{bullets:?}"
        );
        assert!(
            bullets.iter().any(|b| b.contains("55 million")),
            "{bullets:?}"
        );
        assert!(
            bullets.iter().any(|b| b.contains("Monte Carlo")),
            "{bullets:?}"
        );
        assert!(
            bank.skills.iter().any(|s| s.slug == "python"),
            "{:?}",
            bank.skills
        );
        assert!(bank.skills.iter().any(|s| s.slug == "snowflake"));
        assert!(bank.skills.iter().any(|s| s.slug == "dataiku"));
        assert!(bank.skills.iter().any(|s| s.slug == "lightgbm"));
        assert!(
            !bank.skills.iter().any(|s| s.slug == "kubernetes"),
            "imported bank must not inherit the placeholder k8s skill"
        );
        let years = years_experience(&bank.items, "2026-09");
        assert!(
            years > 2.0 && years < 6.0,
            "honest tenure, not an 8-year posture: {years}"
        );
        assert_eq!(
            inferred_education(&bank.items),
            Some(EducationLevel::Bachelor)
        );
        assert_eq!(bank.identity.citizenship.as_deref(), Some("us"));
        assert_eq!(bank.identity.can_obtain_clearance, Some(true));
        assert!(bank.identity.clearance_held.is_none());
    }

    #[test]
    fn inventory_headings_do_not_become_extra_jobs() {
        let md = "\
# Michael Example — Career Evidence Bank\n\n\
## 3. Professional Timeline\n\n\
### Data Scientist — Example Co\n\n\
Dates: January 2026–Present\n\n\
### Coordinator and Trainer — Parks\n\n\
Dates: February 2022–January 2025\n\n\
## 4. Current Data Scientist Project Inventory\n\n\
### Completed Proof Points\n\n\
- Built and executed an API-driven migration that repointed approximately 5,000 datasets across 50 Dataiku projects to Snowflake.\n\n\
#### Bulk Dataiku Connection Migration — Completed\n\n\
- Built and executed an API-driven migration that repointed approximately 5,000 datasets across 50 Dataiku projects to a higher-capacity Snowflake warehouse, replacing manual reconfiguration.\n";
        let bank = parse_markdown(md);
        assert!(
            !bank
                .items
                .iter()
                .any(|i| i.org.eq_ignore_ascii_case("Completed")
                    || i.org.eq_ignore_ascii_case("Career Evidence Bank")),
            "{:?}",
            bank.items
        );
        let ds = bank
            .items
            .iter()
            .find(|i| i.title.as_deref() == Some("Data Scientist"))
            .unwrap();
        assert!(
            ds.accomplishments.iter().any(|a| a.text.contains("5,000")),
            "{:?}",
            ds.accomplishments
        );
        let ops = bank
            .items
            .iter()
            .find(|i| i.title.as_deref() == Some("Coordinator and Trainer"))
            .unwrap();
        assert!(ops.accomplishments.is_empty(), "{:?}", ops.accomplishments);
    }

    #[test]
    fn messy_definition_lists_still_yield_a_name_and_role() {
        let md = "\
# Michael Example — Career Evidence Bank\n\n\
## 3. Professional Timeline\n\n\
### Data Scientist — Example Co\n\n\
Dates: January 2026–Present\n\n\
## 4. Current Data Scientist Project Inventory\n\n\
### Completed Proof Points\n\n\
- Built and executed an API-driven migration that repointed approximately 5,000 datasets across 50 Dataiku projects to Snowflake.\n";
        let bank = parse_markdown(md);
        assert_eq!(bank.identity.full_name.as_deref(), Some("Michael Example"));
        assert!(bank.items.iter().any(|i| i.is_current));
        assert_eq!(
            bank.items
                .iter()
                .map(|i| i.accomplishments.len())
                .sum::<usize>(),
            1
        );
    }

    #[test]
    fn resolve_helper_still_sees_new_taxonomy_nodes() {
        assert_eq!(
            jobseeker_normalize::skill::resolve("Dataiku DSS"),
            Some("dataiku")
        );
        assert_eq!(
            jobseeker_normalize::skill::resolve("LightGBM"),
            Some("lightgbm")
        );
        assert_eq!(
            jobseeker_normalize::skill::resolve("Tableau Server"),
            Some("tableau")
        );
    }
}
