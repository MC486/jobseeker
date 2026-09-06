//! Persist a parsed experience bank onto a profile.

use std::collections::BTreeMap;

use jobseeker_core::domain::profile::ExperienceKind;
use jobseeker_core::ids::{AccomplishmentId, ExperienceId, ProfileId};
use jobseeker_core::slug::slugify;
use jobseeker_core::time::{now, to_rfc3339};
use jobseeker_core::Result;
use serde::Serialize;
use sqlx::Row;

use crate::repo::skill;
use crate::{db_err, Db};

#[derive(Debug, Clone)]
pub struct BankWrite {
    pub full_name: Option<String>,
    pub headline: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub location: Option<String>,
    pub links_json: String,
    pub summary_md: Option<String>,
    pub target_titles_json: String,
    pub target_comp_min_cents: Option<i64>,
    pub target_locations_json: String,
    pub accepts_remote: bool,
    pub willing_to_relocate: bool,
    pub items: Vec<ItemWrite>,
    pub skills: Vec<SkillWrite>,
}

#[derive(Debug, Clone)]
pub struct ItemWrite {
    pub kind: ExperienceKind,
    pub org: String,
    pub title: Option<String>,
    pub location: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub is_current: bool,
    pub description_md: Option<String>,
    pub accomplishments: Vec<AccomplishmentWrite>,
}

#[derive(Debug, Clone)]
pub struct AccomplishmentWrite {
    pub text: String,
    pub variants: BTreeMap<String, String>,
    pub strength: i32,
    pub verified: bool,
    pub skill_slugs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SkillWrite {
    pub slug: String,
    pub years: Option<f32>,
    pub last_used_year: Option<i32>,
    pub is_primary: bool,
    pub evidence_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    pub id: String,
    pub name: String,
    pub full_name: Option<String>,
    pub headline: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub location: Option<String>,
    pub links: serde_json::Value,
    pub summary_md: Option<String>,
    pub target_titles: Vec<String>,
    pub target_comp_min_cents: Option<i64>,
    pub target_locations: Vec<String>,
    pub accepts_remote: bool,
    pub willing_to_relocate: bool,
    pub revision: i64,
    pub years_experience: Option<f32>,
    pub skills: Vec<ProfileSkillView>,
    pub experience: Vec<ExperienceView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileSkillView {
    pub slug: String,
    pub years: Option<f32>,
    pub last_used_year: Option<i32>,
    pub is_primary: bool,
    pub evidence_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExperienceView {
    pub id: String,
    pub kind: String,
    pub org: String,
    pub title: Option<String>,
    pub location: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub is_current: bool,
    pub description_md: Option<String>,
    pub accomplishments: Vec<AccomplishmentView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccomplishmentView {
    pub id: String,
    pub text: String,
    pub variants: serde_json::Value,
    pub strength: i32,
    pub verified: bool,
    pub skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub profile_id: String,
    pub items: usize,
    pub accomplishments: usize,
    pub skills: usize,
}

/// Replace the profile's identity, experience, and skills with a parsed bank.
pub async fn replace_bank(
    db: &Db,
    profile_id: &ProfileId,
    bank: &BankWrite,
) -> Result<ImportReport> {
    skill::ensure_seed(db).await?;
    let ts = to_rfc3339(&now());
    let name = bank.full_name.clone().unwrap_or_else(|| "Default".into());

    sqlx::query(
        "UPDATE profile SET
            name = ?2,
            full_name = ?3,
            headline = ?4,
            email = ?5,
            phone = ?6,
            location = ?7,
            links_json = ?8,
            summary_md = ?9,
            target_titles_json = ?10,
            target_comp_min_cents = ?11,
            target_locations_json = ?12,
            willing_to_relocate = ?13,
            accepts_remote = ?14,
            revision = revision + 1,
            updated_at = ?15
         WHERE id = ?1",
    )
    .bind(profile_id.as_str())
    .bind(&name)
    .bind(bank.full_name.as_deref())
    .bind(bank.headline.as_deref())
    .bind(bank.email.as_deref())
    .bind(bank.phone.as_deref())
    .bind(bank.location.as_deref())
    .bind(&bank.links_json)
    .bind(bank.summary_md.as_deref())
    .bind(&bank.target_titles_json)
    .bind(bank.target_comp_min_cents)
    .bind(&bank.target_locations_json)
    .bind(i64::from(bank.willing_to_relocate))
    .bind(i64::from(bank.accepts_remote))
    .bind(&ts)
    .execute(db.writer())
    .await
    .map_err(db_err)?;

    sqlx::query("DELETE FROM experience_item WHERE profile_id = ?1")
        .bind(profile_id.as_str())
        .execute(db.writer())
        .await
        .map_err(db_err)?;
    sqlx::query("DELETE FROM profile_skill WHERE profile_id = ?1")
        .bind(profile_id.as_str())
        .execute(db.writer())
        .await
        .map_err(db_err)?;

    let mut accomplishments = 0usize;
    for (ordinal, item) in bank.items.iter().enumerate() {
        accomplishments += insert_item(db, profile_id, item, ordinal as i64, &ts).await?;
    }
    for row in &bank.skills {
        insert_profile_skill(db, profile_id, row).await?;
    }

    Ok(ImportReport {
        profile_id: profile_id.to_string(),
        items: bank.items.len(),
        accomplishments,
        skills: bank.skills.len(),
    })
}

async fn insert_item(
    db: &Db,
    profile_id: &ProfileId,
    item: &ItemWrite,
    ordinal: i64,
    ts: &str,
) -> Result<usize> {
    let id = ExperienceId::new();
    let org_normalized = slugify(&item.org);
    sqlx::query(
        "INSERT INTO experience_item (
            id, profile_id, kind, org, org_normalized, title, location,
            start_date, end_date, is_current, description_md, ordinal,
            created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
    )
    .bind(id.as_str())
    .bind(profile_id.as_str())
    .bind(item.kind.as_str())
    .bind(&item.org)
    .bind(&org_normalized)
    .bind(item.title.as_deref())
    .bind(item.location.as_deref())
    .bind(item.start_date.as_deref())
    .bind(item.end_date.as_deref())
    .bind(i64::from(item.is_current))
    .bind(item.description_md.as_deref())
    .bind(ordinal)
    .bind(ts)
    .execute(db.writer())
    .await
    .map_err(db_err)?;

    for (i, acc) in item.accomplishments.iter().enumerate() {
        insert_accomplishment(db, &id, acc, i as i64, ts).await?;
    }
    Ok(item.accomplishments.len())
}

async fn insert_accomplishment(
    db: &Db,
    experience_id: &ExperienceId,
    acc: &AccomplishmentWrite,
    ordinal: i64,
    ts: &str,
) -> Result<()> {
    let id = AccomplishmentId::new();
    let variants = serde_json::to_string(&acc.variants)?;
    sqlx::query(
        "INSERT INTO accomplishment (
            id, experience_item_id, text, variants_json, strength, verified,
            ordinal, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
    )
    .bind(id.as_str())
    .bind(experience_id.as_str())
    .bind(&acc.text)
    .bind(&variants)
    .bind(acc.strength)
    .bind(i64::from(acc.verified))
    .bind(ordinal)
    .bind(ts)
    .execute(db.writer())
    .await
    .map_err(db_err)?;

    for slug in &acc.skill_slugs {
        if let Some(skill_id) = skill::id_by_slug(db, slug).await? {
            sqlx::query(
                "INSERT OR IGNORE INTO accomplishment_skill
                    (accomplishment_id, skill_id, weight, is_primary)
                 VALUES (?1, ?2, 1.0, 0)",
            )
            .bind(id.as_str())
            .bind(skill_id.as_str())
            .execute(db.writer())
            .await
            .map_err(db_err)?;
        }
    }
    Ok(())
}

async fn insert_profile_skill(db: &Db, profile_id: &ProfileId, row: &SkillWrite) -> Result<()> {
    let Some(skill_id) = skill::id_by_slug(db, &row.slug).await? else {
        return Ok(());
    };
    let id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT OR REPLACE INTO profile_skill (
            id, profile_id, skill_id, years, last_used_year, is_primary, evidence_count
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&id)
    .bind(profile_id.as_str())
    .bind(skill_id.as_str())
    .bind(row.years.map(f64::from))
    .bind(row.last_used_year.map(i64::from))
    .bind(i64::from(row.is_primary))
    .bind(row.evidence_count)
    .execute(db.writer())
    .await
    .map(|_| ())
    .map_err(db_err)
}

pub async fn get_view(db: &Db, profile_id: &ProfileId) -> Result<Option<ProfileView>> {
    let row = sqlx::query(
        "SELECT id, name, full_name, headline, email, phone, location, links_json,
                summary_md, target_titles_json, target_comp_min_cents,
                target_locations_json, willing_to_relocate, accepts_remote, revision
           FROM profile WHERE id = ?1 AND deleted_at IS NULL",
    )
    .bind(profile_id.as_str())
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    let Some(row) = row else { return Ok(None) };

    let titles = json_vec(row.try_get("target_titles_json").map_err(db_err)?);
    let locations = json_vec(row.try_get("target_locations_json").map_err(db_err)?);
    let links_raw: Option<String> = row.try_get("links_json").map_err(db_err)?;
    let links = links_raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Array(vec![]));

    let skill_rows = sqlx::query(
        "SELECT s.slug, ps.years, ps.last_used_year, ps.is_primary, ps.evidence_count
           FROM profile_skill ps
           JOIN skill s ON s.id = ps.skill_id
          WHERE ps.profile_id = ?1
          ORDER BY ps.evidence_count DESC, s.slug",
    )
    .bind(profile_id.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    let mut skills = Vec::new();
    for r in skill_rows {
        skills.push(ProfileSkillView {
            slug: r.try_get("slug").map_err(db_err)?,
            years: r
                .try_get::<Option<f64>, _>("years")
                .map_err(db_err)?
                .map(|y| y as f32),
            last_used_year: r
                .try_get::<Option<i64>, _>("last_used_year")
                .map_err(db_err)?
                .map(|y| y as i32),
            is_primary: r.try_get::<i64, _>("is_primary").map_err(db_err)? != 0,
            evidence_count: r.try_get("evidence_count").map_err(db_err)?,
        });
    }

    let exp_rows = sqlx::query(
        "SELECT id, kind, org, title, location, start_date, end_date, is_current, description_md
           FROM experience_item
          WHERE profile_id = ?1
          ORDER BY ordinal, start_date DESC",
    )
    .bind(profile_id.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;

    let mut experience = Vec::new();
    let mut years_experience = 0.0_f32;
    for r in exp_rows {
        let id: String = r.try_get("id").map_err(db_err)?;
        let kind: String = r.try_get("kind").map_err(db_err)?;
        let start_date: Option<String> = r.try_get("start_date").map_err(db_err)?;
        let end_date: Option<String> = r.try_get("end_date").map_err(db_err)?;
        let is_current = r.try_get::<i64, _>("is_current").map_err(db_err)? != 0;
        if kind == ExperienceKind::Role.as_str() {
            if let Some(y) = tenure_years(start_date.as_deref(), end_date.as_deref(), is_current) {
                years_experience += y;
            }
        }
        let acc_rows = sqlx::query(
            "SELECT a.id, a.text, a.variants_json, a.strength, a.verified
               FROM accomplishment a
              WHERE a.experience_item_id = ?1
              ORDER BY a.ordinal",
        )
        .bind(&id)
        .fetch_all(db.reader())
        .await
        .map_err(db_err)?;
        let mut accomplishments = Vec::new();
        for a in acc_rows {
            let acc_id: String = a.try_get("id").map_err(db_err)?;
            let variants_raw: Option<String> = a.try_get("variants_json").map_err(db_err)?;
            let skill_slugs: Vec<String> = sqlx::query_scalar(
                "SELECT s.slug FROM accomplishment_skill aks
                   JOIN skill s ON s.id = aks.skill_id
                  WHERE aks.accomplishment_id = ?1
                  ORDER BY s.slug",
            )
            .bind(&acc_id)
            .fetch_all(db.reader())
            .await
            .map_err(db_err)?;
            accomplishments.push(AccomplishmentView {
                id: acc_id,
                text: a.try_get("text").map_err(db_err)?,
                variants: variants_raw
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or(serde_json::json!({})),
                strength: a.try_get::<i64, _>("strength").map_err(db_err)? as i32,
                verified: a.try_get::<i64, _>("verified").map_err(db_err)? != 0,
                skills: skill_slugs,
            });
        }
        experience.push(ExperienceView {
            id,
            kind,
            org: r.try_get("org").map_err(db_err)?,
            title: r.try_get("title").map_err(db_err)?,
            location: r.try_get("location").map_err(db_err)?,
            start_date,
            end_date,
            is_current,
            description_md: r.try_get("description_md").map_err(db_err)?,
            accomplishments,
        });
    }

    Ok(Some(ProfileView {
        id: row.try_get("id").map_err(db_err)?,
        name: row.try_get("name").map_err(db_err)?,
        full_name: row.try_get("full_name").map_err(db_err)?,
        headline: row.try_get("headline").map_err(db_err)?,
        email: row.try_get("email").map_err(db_err)?,
        phone: row.try_get("phone").map_err(db_err)?,
        location: row.try_get("location").map_err(db_err)?,
        links,
        summary_md: row.try_get("summary_md").map_err(db_err)?,
        target_titles: titles,
        target_comp_min_cents: row.try_get("target_comp_min_cents").map_err(db_err)?,
        target_locations: locations,
        accepts_remote: row.try_get::<i64, _>("accepts_remote").map_err(db_err)? != 0,
        willing_to_relocate: row
            .try_get::<i64, _>("willing_to_relocate")
            .map_err(db_err)?
            != 0,
        revision: row.try_get("revision").map_err(db_err)?,
        years_experience: (years_experience > 0.0).then_some(years_experience),
        skills,
        experience,
    }))
}

fn json_vec(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn tenure_years(start: Option<&str>, end: Option<&str>, current: bool) -> Option<f32> {
    let start = parse_ym(start?)?;
    let end = if current {
        parse_ym("2026-09")?
    } else {
        parse_ym(end?)?
    };
    let months = (end.0 - start.0) * 12 + (end.1 as i32 - start.1 as i32);
    (months >= 0).then_some(months as f32 / 12.0)
}

fn parse_ym(s: &str) -> Option<(i32, u32)> {
    let mut parts = s.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next().and_then(|m| m.parse().ok()).unwrap_or(1);
    (1..=12).contains(&month).then_some((year, month))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::profile;

    #[tokio::test]
    async fn replace_bank_overwrites_the_placeholder_skills() {
        let db = Db::open_in_memory().await.unwrap();
        let id = profile::ensure_default(&db).await.unwrap();
        let write = BankWrite {
            full_name: Some("Alex Rivera".into()),
            headline: Some("Applied Data Scientist".into()),
            email: None,
            phone: None,
            location: Some("Orlando, FL".into()),
            links_json: "[]".into(),
            summary_md: Some("Applied ML plus platform work.".into()),
            target_titles_json: r#"["Applied Data Scientist"]"#.into(),
            target_comp_min_cents: Some(14_500_000),
            target_locations_json: r#"["Orlando, FL","US"]"#.into(),
            accepts_remote: true,
            willing_to_relocate: false,
            items: vec![ItemWrite {
                kind: ExperienceKind::Role,
                org: "Harbor Analytics".into(),
                title: Some("Data Scientist".into()),
                location: Some("Orlando, FL".into()),
                start_date: Some("2026-01".into()),
                end_date: None,
                is_current: true,
                description_md: None,
                accomplishments: vec![AccomplishmentWrite {
                    text: "Built an API-driven Dataiku migration onto Snowflake.".into(),
                    variants: BTreeMap::new(),
                    strength: 5,
                    verified: true,
                    skill_slugs: vec!["dataiku".into(), "snowflake".into()],
                }],
            }],
            skills: vec![SkillWrite {
                slug: "python".into(),
                years: Some(2.0),
                last_used_year: Some(2026),
                is_primary: true,
                evidence_count: 3,
            }],
        };
        let report = replace_bank(&db, &id, &write).await.unwrap();
        assert_eq!(report.accomplishments, 1);
        let view = get_view(&db, &id).await.unwrap().unwrap();
        assert_eq!(view.full_name.as_deref(), Some("Alex Rivera"));
        assert!(view.skills.iter().any(|s| s.slug == "python"));
        assert!(!view.skills.iter().any(|s| s.slug == "kubernetes"));
        assert!(view
            .experience
            .iter()
            .any(|e| e.title.as_deref() == Some("Data Scientist")));
    }
}
