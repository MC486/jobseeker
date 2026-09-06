//! Seed the in-process taxonomy into SQLite so profile skills can be referenced.

use jobseeker_core::ids::SkillId;
use jobseeker_core::time::{now, to_rfc3339};
use jobseeker_core::Result;
use jobseeker_normalize::skill::SEED_SKILLS;

use crate::{db_err, Db};
use sqlx::Row;

/// Insert any seed skill whose slug is missing. Safe to call on existing databases
/// after the taxonomy grows (e.g. Dataiku / LightGBM).
pub async fn ensure_seed(db: &Db) -> Result<u64> {
    let ts = to_rfc3339(&now());
    let existing = sqlx::query("SELECT id, slug FROM skill")
        .fetch_all(db.reader())
        .await
        .map_err(db_err)?;
    let mut slug_to_id = std::collections::HashMap::new();
    for row in existing {
        let id: String = row.try_get("id").map_err(db_err)?;
        let slug: String = row.try_get("slug").map_err(db_err)?;
        slug_to_id.insert(slug, id);
    }

    let mut inserted = 0u64;
    for seed in SEED_SKILLS.iter() {
        if let Some(id) = slug_to_id.get(seed.slug) {
            for alias in seed.aliases {
                let alias_id = uuid::Uuid::now_v7().to_string();
                let normalized = alias.to_ascii_lowercase();
                sqlx::query(
                    "INSERT OR IGNORE INTO skill_alias
                        (id, skill_id, alias, alias_normalized, kind)
                     VALUES (?1, ?2, ?3, ?4, 'synonym')",
                )
                .bind(&alias_id)
                .bind(id)
                .bind(*alias)
                .bind(&normalized)
                .execute(db.writer())
                .await
                .map_err(db_err)?;
            }
            continue;
        }
        let id = SkillId::new();
        let parent_id = seed.parent.and_then(|p| slug_to_id.get(p).cloned());
        sqlx::query(
            "INSERT INTO skill (id, name, slug, kind, parent_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(id.as_str())
        .bind(seed.name)
        .bind(seed.slug)
        .bind(seed.kind.as_str())
        .bind(parent_id.as_deref())
        .bind(&ts)
        .execute(db.writer())
        .await
        .map_err(db_err)?;
        slug_to_id.insert(seed.slug.to_string(), id.as_str().to_string());
        inserted += 1;

        for alias in seed.aliases {
            let alias_id = uuid::Uuid::now_v7().to_string();
            let normalized = alias.to_ascii_lowercase();
            sqlx::query(
                "INSERT OR IGNORE INTO skill_alias
                    (id, skill_id, alias, alias_normalized, kind)
                 VALUES (?1, ?2, ?3, ?4, 'synonym')",
            )
            .bind(&alias_id)
            .bind(id.as_str())
            .bind(*alias)
            .bind(&normalized)
            .execute(db.writer())
            .await
            .map_err(db_err)?;
        }
    }
    Ok(inserted)
}

pub async fn id_by_slug(db: &Db, slug: &str) -> Result<Option<SkillId>> {
    let id: Option<String> = sqlx::query_scalar("SELECT id FROM skill WHERE slug = ?1")
        .bind(slug)
        .fetch_optional(db.reader())
        .await
        .map_err(db_err)?;
    id.map(|s| s.parse()).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn seed_is_idempotent_and_includes_rust() {
        let db = Db::open_in_memory().await.unwrap();
        let n = ensure_seed(&db).await.unwrap();
        assert!(n > 10);
        assert_eq!(ensure_seed(&db).await.unwrap(), 0);
        assert!(id_by_slug(&db, "rust").await.unwrap().is_some());
        assert!(id_by_slug(&db, "kubernetes").await.unwrap().is_some());
    }
}
