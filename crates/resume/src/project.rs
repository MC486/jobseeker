//! Project the experience bank onto one job.
//!
//! Selection is deterministic. The markdown copies stored bullets and quotes the
//! posting. It does not call a model and it does not add facts (`docs/08-resume.md`).

use jobseeker_normalize::skill::extract_all;
use jobseeker_normalize::text::jaccard;
use serde::Serialize;
use std::collections::HashSet;

/// One stored accomplishment the projection may quote.
#[derive(Debug, Clone)]
pub struct BankBullet {
    pub id: String,
    pub role: String,
    pub text: String,
    pub skill_slugs: Vec<String>,
    pub strength: i32,
}

/// One demand on the posting. A linked `skill_slug` is what coverage compares.
#[derive(Debug, Clone)]
pub struct Demand {
    pub id: String,
    pub text: String,
    pub skill_slug: Option<String>,
    pub required: bool,
    pub scored: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChosenBullet {
    pub accomplishment_id: String,
    pub role: String,
    pub text: String,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UncoveredDemand {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CoverageReport {
    pub required_total: i64,
    pub required_covered: i64,
    pub uncovered: Vec<UncoveredDemand>,
}

#[derive(Debug, Clone)]
pub struct Projection {
    pub markdown: String,
    pub bullets: Vec<ChosenBullet>,
    pub coverage: CoverageReport,
}

const RESUME_BUDGET: usize = 8;
const LETTER_BUDGET: usize = 3;

pub fn project_resume(
    name: &str,
    headline: Option<&str>,
    bullets: &[BankBullet],
    demands: &[Demand],
) -> Projection {
    let chosen = choose(bullets, demands, RESUME_BUDGET);
    let coverage = coverage_of(demands, &chosen);
    Projection {
        markdown: resume_markdown(name, headline, &chosen),
        bullets: chosen_rows(&chosen),
        coverage,
    }
}

pub fn project_cover_letter(
    company: &str,
    title: &str,
    bullets: &[BankBullet],
    demands: &[Demand],
) -> Projection {
    let chosen = choose(bullets, demands, LETTER_BUDGET);
    let coverage = coverage_of(demands, &chosen);
    Projection {
        markdown: cover_letter_markdown(company, title, &chosen, &coverage),
        bullets: chosen_rows(&chosen),
        coverage,
    }
}

fn chosen_rows(chosen: &[(BankBullet, Vec<String>)]) -> Vec<ChosenBullet> {
    chosen
        .iter()
        .map(|(b, targets)| ChosenBullet {
            accomplishment_id: b.id.clone(),
            role: b.role.clone(),
            text: b.text.clone(),
            targets: targets.clone(),
        })
        .collect()
}

fn required_scored(demands: &[Demand]) -> Vec<&Demand> {
    demands.iter().filter(|d| d.required && d.scored).collect()
}

fn choose(
    bullets: &[BankBullet],
    demands: &[Demand],
    budget: usize,
) -> Vec<(BankBullet, Vec<String>)> {
    if budget == 0 || bullets.is_empty() {
        return Vec::new();
    }
    let targets = required_scored(demands);
    let mut pool = bullets.to_vec();
    pool.sort_by(|a, b| b.strength.cmp(&a.strength).then(a.id.cmp(&b.id)));
    let mut chosen: Vec<(BankBullet, Vec<String>)> = Vec::new();
    let mut covered: HashSet<String> = HashSet::new();

    if targets.is_empty() {
        for bullet in pool.into_iter().take(budget) {
            chosen.push((bullet, Vec::new()));
        }
        return chosen;
    }

    while chosen.len() < budget && !pool.is_empty() {
        let mut best: Option<(usize, usize, i32)> = None;
        for (i, bullet) in pool.iter().enumerate() {
            let newly = targets
                .iter()
                .filter(|d| !covered.contains(&d.id) && covers(bullet, d))
                .count();
            if newly == 0 {
                continue;
            }
            let rank = (newly, bullet.strength);
            let better = best.map(|(_, n, s)| rank > (n, s)).unwrap_or(true);
            if better {
                best = Some((i, newly, bullet.strength));
            }
        }
        let Some((i, _, _)) = best else {
            break;
        };
        let bullet = pool.remove(i);
        let ids: Vec<String> = targets
            .iter()
            .filter(|d| covers(&bullet, d))
            .map(|d| d.id.clone())
            .collect();
        for id in &ids {
            covered.insert(id.clone());
        }
        chosen.push((bullet, ids));
    }

    if chosen.is_empty() {
        if let Some(bullet) = pool.into_iter().next() {
            chosen.push((bullet, Vec::new()));
        }
    }
    chosen
}

fn covers(bullet: &BankBullet, demand: &Demand) -> bool {
    if !demand.scored {
        return false;
    }
    let want = demand
        .skill_slug
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| jobseeker_normalize::skill::resolve(&demand.text).map(str::to_string));
    let Some(want) = want else {
        return jaccard(&bullet.text, &demand.text) >= 0.34;
    };
    bullet.skill_slugs.iter().any(|s| s == &want)
        || extract_all(&bullet.text).iter().any(|s| *s == want)
}

fn coverage_of(demands: &[Demand], chosen: &[(BankBullet, Vec<String>)]) -> CoverageReport {
    let targets = required_scored(demands);
    let mut hit = HashSet::new();
    for (_, ids) in chosen {
        for id in ids {
            hit.insert(id.clone());
        }
    }
    let uncovered = targets
        .iter()
        .filter(|d| !hit.contains(&d.id))
        .map(|d| UncoveredDemand {
            id: d.id.clone(),
            text: d.text.clone(),
        })
        .collect::<Vec<_>>();
    let total = targets.len() as i64;
    let covered = total - uncovered.len() as i64;
    CoverageReport {
        required_total: total,
        required_covered: covered,
        uncovered,
    }
}

fn resume_markdown(
    name: &str,
    headline: Option<&str>,
    chosen: &[(BankBullet, Vec<String>)],
) -> String {
    let mut out = format!("# {}\n", name.trim());
    if let Some(headline) = headline.map(str::trim).filter(|s| !s.is_empty()) {
        out.push('\n');
        out.push_str(headline);
        out.push('\n');
    }
    out.push_str("\nDraft. Each bullet is copied from the experience bank.\n");
    let mut role = String::new();
    for (bullet, _) in chosen {
        if bullet.role != role {
            role.clone_from(&bullet.role);
            out.push_str("\n## ");
            out.push_str(bullet.role.trim());
            out.push('\n');
        }
        out.push_str("\n- ");
        out.push_str(bullet.text.trim());
        out.push('\n');
    }
    if chosen.is_empty() {
        out.push_str("\nNo accomplishments were selected.\n");
    }
    out
}

fn cover_letter_markdown(
    company: &str,
    title: &str,
    chosen: &[(BankBullet, Vec<String>)],
    coverage: &CoverageReport,
) -> String {
    let mut out = String::from("# Cover letter — draft\n\n");
    out.push_str("This draft quotes the posting and copies bullets from the experience bank.\n\n");
    out.push_str(company.trim());
    out.push_str(" is hiring a ");
    out.push_str(title.trim());
    out.push_str(".\n");
    for (bullet, _) in chosen {
        out.push_str("\n- ");
        out.push_str(bullet.text.trim());
        out.push_str(" (");
        out.push_str(bullet.role.trim());
        out.push_str(")\n");
    }
    out.push('\n');
    if coverage.uncovered.is_empty() && coverage.required_total > 0 {
        out.push_str("The selected bullets speak to every required skill on this posting.\n");
    } else if !coverage.uncovered.is_empty() {
        out.push_str("Open on this posting: ");
        for (i, gap) in coverage.uncovered.iter().enumerate() {
            if i > 0 {
                out.push_str("; ");
            }
            out.push_str(gap.text.trim());
        }
        out.push_str(".\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bullet(text: &str, skills: &[&str], strength: i32) -> BankBullet {
        BankBullet {
            id: format!("b-{text}"),
            role: "Engineer — Northwind".into(),
            text: text.into(),
            skill_slugs: skills.iter().map(|s| (*s).to_string()).collect(),
            strength,
        }
    }

    fn demand(text: &str, skill: Option<&str>) -> Demand {
        Demand {
            id: format!("d-{text}"),
            text: text.into(),
            skill_slug: skill.map(str::to_string),
            required: true,
            scored: true,
        }
    }

    #[test]
    fn a_linked_skill_is_covered_by_that_skills_bullet() {
        let rust = bullet(
            "Rebuilt the ingestion pipeline in Rust, cutting p95 latency 85%",
            &["rust"],
            4,
        );
        let python = demand("Production Python", Some("rust"));
        let sql = demand("SQL", None);
        let draft = project_resume("Ada", Some("Platform engineer"), &[rust], &[python, sql]);
        assert!(draft.markdown.contains("cutting p95 latency 85%"));
        assert!(!draft.markdown.to_lowercase().contains("passionate"));
        assert_eq!(draft.coverage.required_total, 2);
        assert_eq!(draft.coverage.required_covered, 1);
        assert_eq!(draft.coverage.uncovered[0].text, "SQL");
        assert_eq!(
            draft.bullets[0].targets,
            vec!["d-Production Python".to_string()]
        );
    }

    #[test]
    fn an_unlinked_python_demand_is_not_covered_by_rust() {
        let rust = bullet("Rebuilt the ingestion pipeline in Rust", &["rust"], 4);
        let python = demand("Production Python", None);
        let draft = project_resume("Ada", None, &[rust], &[python]);
        assert_eq!(draft.coverage.required_covered, 0);
        assert_eq!(draft.bullets.len(), 1, "the bank still appears");
        assert!(draft.bullets[0].targets.is_empty());
    }

    #[test]
    fn a_cover_letter_quotes_the_company_and_names_the_gap() {
        let rust = bullet("Shipped a Rust service", &["rust"], 5);
        let letter = project_cover_letter(
            "Bench Co",
            "Table Row 32",
            &[rust],
            &[
                demand("Production Python", Some("rust")),
                demand("SQL", None),
            ],
        );
        assert!(letter.markdown.starts_with("# Cover letter — draft"));
        assert!(letter
            .markdown
            .contains("Bench Co is hiring a Table Row 32."));
        assert!(letter.markdown.contains("Shipped a Rust service"));
        assert!(letter.markdown.contains("Open on this posting: SQL."));
        assert!(!letter.markdown.to_lowercase().contains("passionate"));
    }

    #[test]
    fn an_empty_bank_selects_nothing() {
        let draft = project_resume("Ada", None, &[], &[demand("Rust", None)]);
        assert!(draft.bullets.is_empty());
        assert!(draft.markdown.contains("No accomplishments were selected."));
    }
}
