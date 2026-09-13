//! Persist and read explainable match scores.

use std::collections::HashMap;

use jobseeker_core::domain::scoring::{weighted_mean, MatchScore, VerdictStatus};
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
    pub skills_coverage: Option<f64>,
    pub years_fit: Option<f64>,
    pub seniority_fit: Option<f64>,
    pub comp_fit: Option<f64>,
    pub location_fit: Option<f64>,
    pub preference_fit: Option<f64>,
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
    pub years_have: Option<f64>,
    pub years_needed: Option<f64>,
    pub required: bool,
    #[serde(skip_serializing)]
    pub weight: f64,
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
                    flags_json, is_stale, computed_at, explanation_json
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
                    flags_json, is_stale, computed_at, explanation_json
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
    let mut verdicts = load_verdicts(db, &id).await?;
    if verdicts.is_empty() {
        let blob: Option<String> = row.try_get("explanation_json").map_err(db_err)?;
        verdicts = verdicts_from_explanation(blob.as_deref());
    }
    let (skills_coverage, years_fit) = breakdown_from_verdicts(&verdicts);
    let blockers: String = row.try_get("blockers_json").map_err(db_err)?;
    let flags: Option<String> = row.try_get("flags_json").map_err(db_err)?;
    let comp_fit: Option<f64> = row.try_get("comp_fit").map_err(db_err)?;
    let location_fit: Option<f64> = row.try_get("location_fit").map_err(db_err)?;
    Ok(Some(MatchSummary {
        id,
        profile_id: row.try_get("profile_id").map_err(db_err)?,
        overall: row.try_get("overall").map_err(db_err)?,
        required_coverage: row.try_get("required_coverage").map_err(db_err)?,
        preferred_coverage: row.try_get("preferred_coverage").map_err(db_err)?,
        skills_coverage,
        years_fit,
        seniority_fit: row.try_get("seniority_fit").map_err(db_err)?,
        comp_fit,
        location_fit,
        preference_fit: preference_from(comp_fit, location_fit),
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

/// Latest skills/years split for each listed job, in one pair of queries.
///
/// List rows already carry `match_overall` from a subquery. These two numbers are
/// derived from verdicts (or `explanation_json` when verdict rows are missing), so
/// they cannot live in that same subquery without a migration. Overall is never
/// recomputed here.
pub async fn breakdowns_for_jobs(
    db: &Db,
    job_ids: &[String],
) -> Result<HashMap<String, (Option<f64>, Option<f64>)>> {
    if job_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = job_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT m.job_id, m.id, m.explanation_json, m.computed_at
           FROM match_score m
          WHERE m.job_id IN ({placeholders})
          ORDER BY m.computed_at DESC, m.id DESC"
    );
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
    for id in job_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(db.reader()).await.map_err(db_err)?;

    let mut latest: HashMap<String, (String, Option<String>)> = HashMap::new();
    let mut match_ids = Vec::new();
    for row in rows {
        let job_id: String = row.try_get("job_id").map_err(db_err)?;
        if latest.contains_key(&job_id) {
            continue;
        }
        let match_id: String = row.try_get("id").map_err(db_err)?;
        let explanation: Option<String> = row.try_get("explanation_json").map_err(db_err)?;
        match_ids.push(match_id.clone());
        latest.insert(job_id, (match_id, explanation));
    }

    let mut verdicts_by_match = load_verdicts_for_matches(db, &match_ids).await?;
    let mut out = HashMap::with_capacity(latest.len());
    for (job_id, (match_id, explanation)) in latest {
        let mut verdicts = verdicts_by_match.remove(&match_id).unwrap_or_default();
        if verdicts.is_empty() {
            verdicts = verdicts_from_explanation(explanation.as_deref());
        }
        out.insert(job_id, breakdown_from_verdicts(&verdicts));
    }
    Ok(out)
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
    let mut map = load_verdicts_for_matches(db, &[match_id.to_string()]).await?;
    Ok(map.remove(match_id).unwrap_or_default())
}

async fn load_verdicts_for_matches(
    db: &Db,
    match_ids: &[String],
) -> Result<HashMap<String, Vec<RequirementVerdict>>> {
    let mut out: HashMap<String, Vec<RequirementVerdict>> = HashMap::new();
    if match_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = match_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT rm.match_score_id, rm.requirement_id, rm.status, rm.score, rm.rationale,
                rm.years_have, rm.years_needed, rm.weight, r.necessity
           FROM requirement_match rm
           LEFT JOIN requirement r ON r.id = rm.requirement_id
          WHERE rm.match_score_id IN ({placeholders})"
    );
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
    for id in match_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(db.reader()).await.map_err(db_err)?;
    for r in rows {
        let match_id: String = r.try_get("match_score_id").map_err(db_err)?;
        let necessity: Option<String> = r.try_get("necessity").map_err(db_err)?;
        let required = matches!(
            necessity.as_deref(),
            Some("required") | Some("implied") | None
        );
        let weight: f64 = r.try_get("weight").unwrap_or(1.0);
        out.entry(match_id).or_default().push(RequirementVerdict {
            requirement_id: r.try_get("requirement_id").map_err(db_err)?,
            status: r.try_get("status").map_err(db_err)?,
            score: r.try_get("score").map_err(db_err)?,
            rationale: r.try_get("rationale").map_err(db_err)?,
            years_have: r.try_get("years_have").map_err(db_err)?,
            years_needed: r.try_get("years_needed").map_err(db_err)?,
            required,
            weight: if weight > 0.0 { weight } else { 1.0 },
        });
    }
    Ok(out)
}

fn verdicts_from_explanation(blob: Option<&str>) -> Vec<RequirementVerdict> {
    let Some(blob) = blob else {
        return Vec::new();
    };
    let Ok(matches) =
        serde_json::from_str::<Vec<jobseeker_core::domain::scoring::RequirementMatch>>(blob)
    else {
        return Vec::new();
    };
    matches
        .into_iter()
        .map(|m| RequirementVerdict {
            requirement_id: m.requirement_id.as_str().to_string(),
            status: m.status.as_str().to_string(),
            score: f64::from(m.score),
            rationale: m.rationale,
            years_have: m.years_have.map(f64::from),
            years_needed: m.years_needed.map(f64::from),
            required: true,
            weight: f64::from(if m.weight > 0.0 { m.weight } else { 1.0 }),
        })
        .collect()
}

fn preference_from(comp_fit: Option<f64>, location_fit: Option<f64>) -> Option<f64> {
    let weights = jobseeker_core::config::Weights::default();
    weighted_mean(
        [
            comp_fit.map(|v| (v as f32, weights.comp)),
            location_fit.map(|v| (v as f32, weights.location)),
        ]
        .into_iter()
        .flatten(),
    )
    .map(f64::from)
}

/// Rebuild the diagnostic split from stored verdicts so older scores (no extra)
/// columns) still show skills vs years. Overall is never recomputed here.
fn breakdown_from_verdicts(verdicts: &[RequirementVerdict]) -> (Option<f64>, Option<f64>) {
    let skills = weighted_mean(verdicts.iter().filter_map(|v| {
        if !v.required || v.status == VerdictStatus::Unknown.as_str() {
            return None;
        }
        let skill = if v.years_needed.is_some() {
            if v.years_have.is_some() {
                1.0
            } else {
                v.score as f32
            }
        } else {
            v.score as f32
        };
        Some((skill, v.weight as f32))
    }))
    .map(f64::from);
    let years = weighted_mean(verdicts.iter().filter_map(|v| {
        if !v.required || v.status == VerdictStatus::Unknown.as_str() {
            return None;
        }
        let needed = v.years_needed.filter(|n| *n > 0.0)?;
        let have = v.years_have.unwrap_or(0.0);
        Some(((have / needed) as f32, v.weight as f32))
    }))
    .map(f64::from);
    (skills, years)
}
