//! Persist and read explainable match scores.

use jobseeker_core::domain::scoring::MatchScore;
use jobseeker_core::ids::{JobId, MatchScoreId, ProfileId};
use jobseeker_core::time::{now, to_rfc3339};
use jobseeker_core::Result;
use serde::Serialize;
use sqlx::Row;

use crate::{db_err, Db};

#[derive(Debug, Clone, Serialize)]
pub struct MatchSummary {
    pub id: String,
    pub profile_id: String,
    pub overall: f64,
    pub required_coverage: Option<f64>,
    pub preferred_coverage: Option<f64>,
    pub seniority_fit: Option<f64>,
    pub comp_fit: Option<f64>,
    pub location_fit: Option<f64>,
    pub blocker_count: i64,
    pub blockers: serde_json::Value,
    pub flags: serde_json::Value,
    pub is_stale: bool,
    pub computed_at: String,
    pub verdicts: Vec<RequirementVerdict>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequirementVerdict {
    pub requirement_id: String,
    pub status: String,
    pub score: f64,
    pub rationale: String,
}

/// Insert or replace the score for `(job, profile, algorithm)`.
pub async fn upsert(db: &Db, score: &MatchScore) -> Result<MatchScoreId> {
    let ts = to_rfc3339(&now());
    let blockers = serde_json::to_string(&score.blockers)?;
    let weights = serde_json::to_string(&score.weights_used)?;
    let flags = serde_json::to_string(&score.flags)?;
    let explanation = serde_json::to_string(&score.requirement_matches)?;

    sqlx::query(
        r#"INSERT INTO match_score (
            id, job_id, profile_id, algorithm_version, overall,
            required_coverage, preferred_coverage, seniority_fit, comp_fit,
            location_fit, semantic_similarity, blocker_count, blockers_json,
            weights_json, explanation_json, flags_json, inputs_hash, is_stale,
            computed_at, duration_ms
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, 0, ?18, ?19
        )
        ON CONFLICT (job_id, profile_id, algorithm_version) DO UPDATE SET
            overall = excluded.overall,
            required_coverage = excluded.required_coverage,
            preferred_coverage = excluded.preferred_coverage,
            seniority_fit = excluded.seniority_fit,
            comp_fit = excluded.comp_fit,
            location_fit = excluded.location_fit,
            semantic_similarity = excluded.semantic_similarity,
            blocker_count = excluded.blocker_count,
            blockers_json = excluded.blockers_json,
            weights_json = excluded.weights_json,
            explanation_json = excluded.explanation_json,
            flags_json = excluded.flags_json,
            inputs_hash = excluded.inputs_hash,
            is_stale = 0,
            computed_at = excluded.computed_at,
            duration_ms = excluded.duration_ms"#,
    )
    .bind(score.id.as_str())
    .bind(score.job_id.as_str())
    .bind(score.profile_id.as_str())
    .bind(&score.algorithm_version)
    .bind(score.overall as f64)
    .bind(score.subscores.required_coverage.map(f64::from))
    .bind(score.subscores.preferred_coverage.map(f64::from))
    .bind(score.subscores.seniority_fit.map(f64::from))
    .bind(score.subscores.comp_fit.map(f64::from))
    .bind(score.subscores.location_fit.map(f64::from))
    .bind(score.subscores.semantic_similarity.map(f64::from))
    .bind(score.blockers.len() as i64)
    .bind(&blockers)
    .bind(&weights)
    .bind(&explanation)
    .bind(&flags)
    .bind(&score.inputs_hash)
    .bind(&ts)
    .bind(score.duration_ms)
    .execute(db.writer())
    .await
    .map_err(db_err)?;

    let stored_id: String = sqlx::query_scalar(
        "SELECT id FROM match_score
          WHERE job_id = ?1 AND profile_id = ?2 AND algorithm_version = ?3",
    )
    .bind(score.job_id.as_str())
    .bind(score.profile_id.as_str())
    .bind(&score.algorithm_version)
    .fetch_one(db.reader())
    .await
    .map_err(db_err)?;
    let match_id: MatchScoreId = stored_id.parse()?;

    sqlx::query("DELETE FROM requirement_match WHERE match_score_id = ?1")
        .bind(match_id.as_str())
        .execute(db.writer())
        .await
        .map_err(db_err)?;

    for m in &score.requirement_matches {
        let rid = uuid::Uuid::now_v7().to_string();
        let evidence = serde_json::to_string(&m.evidence)?;
        sqlx::query(
            r#"INSERT INTO requirement_match (
                id, match_score_id, requirement_id, status, score, weight,
                evidence_json, years_have, years_needed, rationale, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
        )
        .bind(&rid)
        .bind(match_id.as_str())
        .bind(m.requirement_id.as_str())
        .bind(m.status.as_str())
        .bind(m.score as f64)
        .bind(m.weight as f64)
        .bind(&evidence)
        .bind(m.years_have.map(f64::from))
        .bind(m.years_needed.map(f64::from))
        .bind(&m.rationale)
        .bind(&ts)
        .execute(db.writer())
        .await
        .map_err(db_err)?;
    }
    Ok(match_id)
}

pub async fn latest_for_job(
    db: &Db,
    job_id: &JobId,
    profile_id: Option<&ProfileId>,
) -> Result<Option<MatchSummary>> {
    let row = if let Some(pid) = profile_id {
        sqlx::query(
            "SELECT id, profile_id, overall, required_coverage, preferred_coverage,
                    seniority_fit, comp_fit, location_fit, blocker_count, blockers_json,
                    flags_json, is_stale, computed_at
               FROM match_score
              WHERE job_id = ?1 AND profile_id = ?2
              ORDER BY computed_at DESC LIMIT 1",
        )
        .bind(job_id.as_str())
        .bind(pid.as_str())
        .fetch_optional(db.reader())
        .await
        .map_err(db_err)?
    } else {
        sqlx::query(
            "SELECT id, profile_id, overall, required_coverage, preferred_coverage,
                    seniority_fit, comp_fit, location_fit, blocker_count, blockers_json,
                    flags_json, is_stale, computed_at
               FROM match_score
              WHERE job_id = ?1
              ORDER BY computed_at DESC LIMIT 1",
        )
        .bind(job_id.as_str())
        .fetch_optional(db.reader())
        .await
        .map_err(db_err)?
    };
    let Some(row) = row else { return Ok(None) };
    let id: String = row.try_get("id").map_err(db_err)?;
    let verdicts = load_verdicts(db, &id).await?;
    let blockers: String = row.try_get("blockers_json").map_err(db_err)?;
    let flags: Option<String> = row.try_get("flags_json").map_err(db_err)?;
    Ok(Some(MatchSummary {
        id,
        profile_id: row.try_get("profile_id").map_err(db_err)?,
        overall: row.try_get("overall").map_err(db_err)?,
        required_coverage: row.try_get("required_coverage").map_err(db_err)?,
        preferred_coverage: row.try_get("preferred_coverage").map_err(db_err)?,
        seniority_fit: row.try_get("seniority_fit").map_err(db_err)?,
        comp_fit: row.try_get("comp_fit").map_err(db_err)?,
        location_fit: row.try_get("location_fit").map_err(db_err)?,
        blocker_count: row.try_get("blocker_count").map_err(db_err)?,
        blockers: serde_json::from_str(&blockers).unwrap_or(serde_json::Value::Array(vec![])),
        flags: flags
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Array(vec![])),
        is_stale: row.try_get::<i64, _>("is_stale").map_err(db_err)? != 0,
        computed_at: row.try_get("computed_at").map_err(db_err)?,
        verdicts,
    }))
}

pub async fn mark_stale_for_profile(db: &Db, profile_id: &ProfileId) -> Result<u64> {
    let res = sqlx::query("UPDATE match_score SET is_stale = 1 WHERE profile_id = ?1")
        .bind(profile_id.as_str())
        .execute(db.writer())
        .await
        .map_err(db_err)?;
    Ok(res.rows_affected())
}

async fn load_verdicts(db: &Db, match_id: &str) -> Result<Vec<RequirementVerdict>> {
    let rows = sqlx::query(
        "SELECT requirement_id, status, score, rationale
           FROM requirement_match WHERE match_score_id = ?1",
    )
    .bind(match_id)
    .fetch_all(db.reader())
    .await
    .map_err(db_err)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(RequirementVerdict {
            requirement_id: r.try_get("requirement_id").map_err(db_err)?,
            status: r.try_get("status").map_err(db_err)?,
            score: r.try_get("score").map_err(db_err)?,
            rationale: r.try_get("rationale").map_err(db_err)?,
        });
    }
    Ok(out)
}
