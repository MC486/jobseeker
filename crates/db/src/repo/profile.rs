//! Default profile and the columns the matcher snapshot is built from.

use jobseeker_core::ids::{ProfileId, SkillId};
use jobseeker_core::time::{now, to_rfc3339};
use jobseeker_core::Result;
use sqlx::Row;

use crate::repo::skill;
use crate::{db_err, Db};

#[derive(Debug, Clone)]
pub struct ProfileSkillRow {
    pub skill_id: Option<SkillId>,
    pub slug: String,
    pub years: Option<f32>,
    pub last_used_year: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct ProfileRow {
    pub id: ProfileId,
    pub target_comp_min_cents: Option<i64>,
    pub target_locations: Vec<String>,
    pub accepts_remote: bool,
    pub willing_to_relocate: bool,
    pub revision: i64,
    pub skills: Vec<ProfileSkillRow>,
}

/// Ensure a default profile exists, with a small skill bank so scoring is not empty.
pub async fn ensure_default(db: &Db) -> Result<ProfileId> {
    skill::ensure_seed(db).await?;
    if let Some(id) = default_id(db).await? {
        return Ok(id);
    }

    let id = ProfileId::new();
    let ts = to_rfc3339(&now());
    sqlx::query(
        "INSERT INTO profile (
            id, name, headline, target_comp_min_cents, target_locations_json,
            willing_to_relocate, accepts_remote, is_default, revision,
            created_at, updated_at
         ) VALUES (?1, 'Default', 'Platform engineer', 15000000, ?2, 0, 1, 1, 1, ?3, ?3)",
    )
    .bind(id.as_str())
    .bind(r#"["US"]"#)
    .bind(&ts)
    .execute(db.writer())
    .await
    .map_err(db_err)?;

    for (slug, years) in [("rust", 8.0_f64), ("kubernetes", 5.0)] {
        if let Some(skill_id) = skill::id_by_slug(db, slug).await? {
            attach_skill(db, &id, &skill_id, years).await?;
        }
    }
    Ok(id)
}

pub async fn default_id(db: &Db) -> Result<Option<ProfileId>> {
    let id: Option<String> =
        sqlx::query_scalar("SELECT id FROM profile WHERE is_default = 1 AND deleted_at IS NULL")
            .fetch_optional(db.reader())
            .await
            .map_err(db_err)?;
    id.map(|s| s.parse()).transpose()
}

async fn attach_skill(db: &Db, profile: &ProfileId, skill_id: &SkillId, years: f64) -> Result<()> {
    let id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT OR IGNORE INTO profile_skill (id, profile_id, skill_id, years, level, is_primary)
         VALUES (?1, ?2, ?3, ?4, 'proficient', 1)",
    )
    .bind(&id)
    .bind(profile.as_str())
    .bind(skill_id.as_str())
    .bind(years)
    .execute(db.writer())
    .await
    .map(|_| ())
    .map_err(db_err)
}

/// Load the default profile, or a specific one.
pub async fn get(db: &Db, id: Option<&ProfileId>) -> Result<Option<ProfileRow>> {
    let id = match id {
        Some(id) => id.clone(),
        None => match default_id(db).await? {
            Some(id) => id,
            None => return Ok(None),
        },
    };

    let row = sqlx::query(
        "SELECT id, target_comp_min_cents, target_locations_json,
                willing_to_relocate, accepts_remote, revision
           FROM profile WHERE id = ?1 AND deleted_at IS NULL",
    )
    .bind(id.as_str())
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    let Some(row) = row else { return Ok(None) };

    let locations_raw: Option<String> = row.try_get("target_locations_json").map_err(db_err)?;
    let target_locations: Vec<String> = locations_raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let skill_rows = sqlx::query(
        "SELECT ps.skill_id, s.slug, ps.years, ps.last_used_year
           FROM profile_skill ps
           JOIN skill s ON s.id = ps.skill_id
          WHERE ps.profile_id = ?1",
    )
    .bind(id.as_str())
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;

    let mut skills = Vec::with_capacity(skill_rows.len());
    for r in skill_rows {
        let skill_id: String = r.try_get("skill_id").map_err(db_err)?;
        skills.push(ProfileSkillRow {
            skill_id: skill_id.parse().ok(),
            slug: r.try_get("slug").map_err(db_err)?,
            years: r
                .try_get::<Option<f64>, _>("years")
                .map_err(db_err)?
                .map(|y| y as f32),
            last_used_year: r
                .try_get::<Option<i64>, _>("last_used_year")
                .map_err(db_err)?
                .map(|y| y as i32),
        });
    }

    Ok(Some(ProfileRow {
        id,
        target_comp_min_cents: row.try_get("target_comp_min_cents").map_err(db_err)?,
        target_locations,
        accepts_remote: row.try_get::<i64, _>("accepts_remote").map_err(db_err)? != 0,
        willing_to_relocate: row
            .try_get::<i64, _>("willing_to_relocate")
            .map_err(db_err)?
            != 0,
        revision: row.try_get("revision").map_err(db_err)?,
        skills,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_profile_has_rust_and_kubernetes() {
        let db = Db::open_in_memory().await.unwrap();
        let id = ensure_default(&db).await.unwrap();
        let again = ensure_default(&db).await.unwrap();
        assert_eq!(id, again);
        let row = get(&db, None).await.unwrap().unwrap();
        assert!(row.skills.iter().any(|s| s.slug == "rust"));
        assert!(row.skills.iter().any(|s| s.slug == "kubernetes"));
        assert!(row.accepts_remote);
    }
}
