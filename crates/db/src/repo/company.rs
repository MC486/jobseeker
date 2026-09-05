//! Company resolution.
//!
//! The interesting operation is *resolution*, not insertion: "Acme, Inc." arriving from
//! LinkedIn and "Acme Robotics" arriving from Greenhouse should not become two employers.

use jobseeker_core::domain::Company;
use jobseeker_core::ids::CompanyId;
use jobseeker_core::slug::{normalize_company_name, slugify};
use jobseeker_core::time::{now, parse_rfc3339, to_rfc3339};
use jobseeker_core::Result;
use sqlx::Row;

use crate::{db_err, Db};

/// Find an existing company by normalized name, or create one.
///
/// Only exact normalized-name matching happens here. Fuzzy resolution (trigram similarity,
/// apply-URL host matching) belongs in `jobseeker-pipeline`, where it can ask the user to
/// confirm — a wrong company merge is user-visible and annoying, so the threshold for doing
/// it silently is deliberately "exact".
pub async fn resolve_or_create(db: &Db, name: &str, website: Option<&str>) -> Result<CompanyId> {
    let normalized = normalize_company_name(name);
    if let Some(existing) = find_by_normalized_name(db, &normalized).await? {
        if let Some(site) = website {
            // Backfill a website we did not have before, but never overwrite one.
            sqlx::query(
                "UPDATE company SET website = COALESCE(website, ?1), updated_at = ?2
                  WHERE id = ?3",
            )
            .bind(site)
            .bind(to_rfc3339(&now()))
            .bind(existing.as_str())
            .execute(db.writer())
            .await
            .map_err(db_err)?;
        }
        return Ok(existing);
    }

    let id = CompanyId::new();
    let ts = to_rfc3339(&now());
    let slug = unique_slug(db, &slugify(name)).await?;
    sqlx::query(
        "INSERT INTO company (id, name, slug, name_normalized, website, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
    )
    .bind(id.as_str())
    .bind(name)
    .bind(&slug)
    .bind(&normalized)
    .bind(website)
    .bind(&ts)
    .execute(db.writer())
    .await
    .map_err(db_err)?;
    Ok(id)
}

pub async fn find_by_normalized_name(db: &Db, normalized: &str) -> Result<Option<CompanyId>> {
    let id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM company WHERE name_normalized = ?1 AND deleted_at IS NULL LIMIT 1",
    )
    .bind(normalized)
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    id.map(|s| s.parse()).transpose()
}

/// Slugs appear in file paths, which are a public contract, so collisions get a numeric
/// suffix rather than being allowed to clash.
async fn unique_slug(db: &Db, base: &str) -> Result<String> {
    for suffix in 0..100 {
        let candidate = if suffix == 0 {
            base.to_string()
        } else {
            format!("{base}-{suffix}")
        };
        let taken: Option<String> = sqlx::query_scalar("SELECT id FROM company WHERE slug = ?1")
            .bind(&candidate)
            .fetch_optional(db.reader())
            .await
            .map_err(db_err)?;
        if taken.is_none() {
            return Ok(candidate);
        }
    }
    Ok(format!("{base}-{}", CompanyId::new().short()))
}

pub async fn get(db: &Db, id: &CompanyId) -> Result<Option<Company>> {
    let row = sqlx::query(
        "SELECT id, name, slug, name_normalized, website, careers_url, linkedin_url, industry,
                size_bucket, hq_location, funding_stage, is_public, notes_md, rating,
                created_at, updated_at
           FROM company WHERE id = ?1 AND deleted_at IS NULL",
    )
    .bind(id.as_str())
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;

    let Some(row) = row else { return Ok(None) };
    Ok(Some(Company {
        id: row.try_get::<String, _>("id").map_err(db_err)?.parse()?,
        name: row.try_get("name").map_err(db_err)?,
        slug: row.try_get("slug").map_err(db_err)?,
        name_normalized: row.try_get("name_normalized").map_err(db_err)?,
        website: row.try_get("website").map_err(db_err)?,
        careers_url: row.try_get("careers_url").map_err(db_err)?,
        linkedin_url: row.try_get("linkedin_url").map_err(db_err)?,
        industry: row.try_get("industry").map_err(db_err)?,
        size_bucket: row.try_get("size_bucket").map_err(db_err)?,
        hq_location: row.try_get("hq_location").map_err(db_err)?,
        funding_stage: row.try_get("funding_stage").map_err(db_err)?,
        is_public: row
            .try_get::<Option<i64>, _>("is_public")
            .map_err(db_err)?
            .map(|v| v != 0),
        notes_md: row.try_get("notes_md").map_err(db_err)?,
        rating: row
            .try_get::<Option<i64>, _>("rating")
            .map_err(db_err)?
            .map(|v| v as i32),
        created_at: parse_rfc3339(&row.try_get::<String, _>("created_at").map_err(db_err)?)?,
        updated_at: parse_rfc3339(&row.try_get::<String, _>("updated_at").map_err(db_err)?)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spelling_variants_resolve_to_one_company() {
        let db = Db::open_in_memory().await.unwrap();

        let a = resolve_or_create(&db, "Acme Robotics", None).await.unwrap();
        let b = resolve_or_create(&db, "Acme Robotics, Inc.", None)
            .await
            .unwrap();
        let c = resolve_or_create(&db, "ACME ROBOTICS LLC", None)
            .await
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM company")
            .fetch_one(db.reader())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn genuinely_different_companies_stay_separate() {
        let db = Db::open_in_memory().await.unwrap();
        let a = resolve_or_create(&db, "Acme Robotics", None).await.unwrap();
        let b = resolve_or_create(&db, "Acme Financial", None)
            .await
            .unwrap();
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn slug_collisions_get_a_suffix() {
        let db = Db::open_in_memory().await.unwrap();
        // Different normalized names that slugify identically would collide; force the case
        // by inserting a company holding the slug first.
        resolve_or_create(&db, "Acme Robotics", None).await.unwrap();
        sqlx::query(
            "UPDATE company SET name_normalized = 'something else' WHERE slug = 'acme-robotics'",
        )
        .execute(db.writer())
        .await
        .unwrap();
        resolve_or_create(&db, "Acme Robotics", None).await.unwrap();

        let slugs: Vec<String> = sqlx::query_scalar("SELECT slug FROM company ORDER BY slug")
            .fetch_all(db.reader())
            .await
            .unwrap();
        assert_eq!(slugs, vec!["acme-robotics", "acme-robotics-1"]);
    }

    #[tokio::test]
    async fn website_is_backfilled_but_never_overwritten() {
        let db = Db::open_in_memory().await.unwrap();
        let id = resolve_or_create(&db, "Acme", None).await.unwrap();
        resolve_or_create(&db, "Acme", Some("https://acme.example"))
            .await
            .unwrap();
        assert_eq!(
            get(&db, &id).await.unwrap().unwrap().website.as_deref(),
            Some("https://acme.example")
        );

        resolve_or_create(&db, "Acme", Some("https://impostor.example"))
            .await
            .unwrap();
        assert_eq!(
            get(&db, &id).await.unwrap().unwrap().website.as_deref(),
            Some("https://acme.example"),
            "an existing website must win over a later guess"
        );
    }

    #[tokio::test]
    async fn get_returns_none_for_a_missing_company() {
        let db = Db::open_in_memory().await.unwrap();
        assert!(get(&db, &CompanyId::new()).await.unwrap().is_none());
    }
}
