//! Persist an extracted job into the relational schema.
//!
//! This is the write path the pipeline uses after extraction. Repositories stay
//! read-oriented; the full insert/replace lives here so the column list stays in one place.

use jobseeker_core::domain::enums::{
    ApplyKind, EducationLevel, EmploymentType, JobStatus, Seniority, Tristate, WorkMode,
};
use jobseeker_core::domain::job::ExtractedJob;
use jobseeker_core::domain::salary::SalaryPeriod;
use jobseeker_core::ids::{CompanyId, JobId, ListingId, RequirementId};
use jobseeker_core::slug::{normalize_job_title, slugify};
use jobseeker_core::time::{now, to_rfc3339};
use jobseeker_core::Result;
use jobseeker_normalize::location::parse_location;
use jobseeker_normalize::requirement::AtomizedRequirement;
use jobseeker_normalize::salary::parse_salary;

use crate::repo::{company, listing};
use crate::{db_err, Db};

/// Outcome of writing an extracted job into SQLite.
#[derive(Debug, Clone)]
pub struct PersistOutcome {
    pub job_id: JobId,
    pub company_id: CompanyId,
    pub created: bool,
}

/// Everything persist needs from the extraction pipeline.
pub struct PersistExtracted<'a> {
    pub listing_id: Option<&'a ListingId>,
    pub job: &'a ExtractedJob,
    pub requirements: &'a [AtomizedRequirement],
    pub description_md: &'a str,
    pub description_text: &'a str,
    pub content_hash: &'a str,
    pub partial: bool,
    pub model: Option<&'a str>,
}

/// Persist (or replace) the extracted job, attaching it to `listing_id` when present.
///
/// One listing maps to one current job row. Re-extracting updates that row in place so
/// requirement and location children stay attached to the same id.
pub async fn persist_extracted(db: &Db, input: PersistExtracted<'_>) -> Result<PersistOutcome> {
    let company_name = input
        .job
        .company_name
        .as_ref()
        .map(|s| s.value.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("Unknown company");
    let company_id = company::resolve_or_create(db, company_name, None).await?;

    let existing = if let Some(listing_id) = input.listing_id {
        listing_job_id(db, listing_id).await?
    } else {
        None
    };

    let mut tx = db.writer().begin().await.map_err(db_err)?;
    let (job_id, created) = if let Some(job_id) = existing {
        update_job(&mut tx, &job_id, &company_id, &input).await?;
        (job_id, false)
    } else {
        let job_id = JobId::new();
        insert_job(&mut tx, &job_id, &company_id, &input).await?;
        (job_id, true)
    };

    replace_locations(&mut tx, &job_id, input.job).await?;
    replace_requirements(&mut tx, &job_id, input.requirements).await?;
    tx.commit().await.map_err(db_err)?;

    if let Some(listing_id) = input.listing_id {
        listing::attach_to_job(db, listing_id, &job_id).await?;
    }

    crate::repo::job::reindex_fts(db, &job_id).await?;
    write_job_provenance(db, &job_id, input.job, input.model).await?;
    Ok(PersistOutcome {
        job_id,
        company_id,
        created,
    })
}

async fn listing_job_id(db: &Db, listing_id: &ListingId) -> Result<Option<JobId>> {
    let id: Option<Option<String>> =
        sqlx::query_scalar("SELECT job_id FROM job_source_listing WHERE id = ?1")
            .bind(listing_id.as_str())
            .fetch_optional(db.reader())
            .await
            .map_err(db_err)?;
    match id {
        Some(Some(s)) => Ok(Some(s.parse()?)),
        _ => Ok(None),
    }
}

struct JobRow {
    title: String,
    title_normalized: String,
    slug: String,
    seniority: Seniority,
    employment: EmploymentType,
    work_mode: WorkMode,
    work_mode_detail: Option<String>,
    department: Option<String>,
    description_md: String,
    description_text: String,
    summary: Option<String>,
    responsibilities_md: Option<String>,
    benefits_md: Option<String>,
    salary_min_cents: Option<i64>,
    salary_max_cents: Option<i64>,
    salary_currency: Option<String>,
    salary_period: SalaryPeriod,
    salary_is_estimate: bool,
    salary_raw: Option<String>,
    posted_at: Option<String>,
    posted_precision: Option<String>,
    closes_at: Option<String>,
    closes_precision: Option<String>,
    apply_url: Option<String>,
    apply_kind: ApplyKind,
    requires_clearance: Option<String>,
    visa: Tristate,
    travel_pct: Option<i32>,
    education_min: EducationLevel,
    years_min: Option<f32>,
    content_hash: String,
    extraction_model: Option<String>,
    extracted_at: String,
    extraction_confidence: Option<f32>,
    extraction_partial: bool,
    status: JobStatus,
}

impl JobRow {
    fn from_input(input: &PersistExtracted<'_>) -> Self {
        let title = input
            .job
            .title
            .as_ref()
            .map(|s| s.value.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "(untitled)".into());
        let salary_raw = input.job.salary.as_ref().map(|s| s.value.text.clone());
        let parsed = input.job.salary.as_ref().and_then(|s| {
            parse_salary(
                &s.value.text,
                s.value.country_hint.as_deref(),
                s.value.source_kind,
            )
        });
        let posted = input.job.posted_at.as_ref().map(|s| s.value);
        let closes = input.job.closes_at.as_ref().map(|s| s.value);
        Self {
            title_normalized: normalize_job_title(&title),
            slug: slugify(&title),
            title,
            seniority: input
                .job
                .seniority
                .as_ref()
                .map(|s| s.value)
                .unwrap_or(Seniority::Unknown),
            employment: input
                .job
                .employment_type
                .as_ref()
                .map(|s| s.value)
                .unwrap_or(EmploymentType::Unknown),
            work_mode: input
                .job
                .work_mode
                .as_ref()
                .map(|s| s.value)
                .unwrap_or(WorkMode::Unknown),
            work_mode_detail: input.job.work_mode_detail.as_ref().map(|s| s.value.clone()),
            department: input.job.department.as_ref().map(|s| s.value.clone()),
            description_md: input.description_md.to_string(),
            description_text: input.description_text.to_string(),
            summary: input.job.summary.as_ref().map(|s| s.value.clone()),
            responsibilities_md: input
                .job
                .responsibilities_md
                .as_ref()
                .map(|s| s.value.clone()),
            benefits_md: input.job.benefits_md.as_ref().map(|s| s.value.clone()),
            salary_min_cents: parsed.as_ref().and_then(|s| s.min_cents),
            salary_max_cents: parsed.as_ref().and_then(|s| s.max_cents),
            salary_currency: parsed.as_ref().map(|s| s.currency.clone()),
            salary_period: parsed
                .as_ref()
                .map(|s| s.period)
                .unwrap_or(SalaryPeriod::Unknown),
            salary_is_estimate: parsed.as_ref().is_some_and(|s| s.is_estimate),
            salary_raw,
            posted_at: posted.map(|p| to_rfc3339(&p.at)),
            posted_precision: posted.map(|p| p.precision.as_str().to_string()),
            closes_at: closes.map(|p| to_rfc3339(&p.at)),
            closes_precision: closes.map(|p| p.precision.as_str().to_string()),
            apply_url: input.job.apply_url.as_ref().map(|s| s.value.clone()),
            apply_kind: input
                .job
                .apply_kind
                .as_ref()
                .map(|s| s.value)
                .unwrap_or(ApplyKind::Unknown),
            requires_clearance: input
                .job
                .requires_clearance
                .as_ref()
                .map(|s| s.value.clone()),
            visa: input
                .job
                .visa_sponsorship
                .as_ref()
                .map(|s| s.value)
                .unwrap_or(Tristate::Unspecified),
            travel_pct: input.job.travel_pct.as_ref().map(|s| s.value),
            education_min: input
                .job
                .education_min
                .as_ref()
                .map(|s| s.value)
                .unwrap_or(EducationLevel::Unknown),
            years_min: input.job.years_experience_min.as_ref().map(|s| s.value),
            content_hash: input.content_hash.to_string(),
            extraction_model: input.model.map(str::to_string),
            extracted_at: to_rfc3339(&now()),
            extraction_confidence: input.job.mean_confidence(),
            extraction_partial: input.partial,
            status: JobStatus::Open,
        }
    }
}

async fn insert_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job_id: &JobId,
    company_id: &CompanyId,
    input: &PersistExtracted<'_>,
) -> Result<()> {
    let row = JobRow::from_input(input);
    let ts = to_rfc3339(&now());
    sqlx::query(
        r#"INSERT INTO job (
            id, company_id, slug, title, title_normalized,
            seniority, employment_type, work_mode, work_mode_detail, department,
            description_md, description_text, summary, responsibilities_md, benefits_md,
            salary_min_cents, salary_max_cents, salary_currency, salary_period,
            salary_is_estimate, salary_raw,
            posted_at, posted_at_precision, closes_at, closes_at_precision,
            first_seen_at, last_seen_at, status,
            apply_url, apply_kind, requires_clearance, visa_sponsorship, travel_pct,
            education_min, years_experience_min,
            content_hash, extraction_model, extracted_at, extraction_confidence,
            extraction_partial, created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?26, ?27,
            ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?26, ?26
        )"#,
    )
    .bind(job_id.as_str())
    .bind(company_id.as_str())
    .bind(&row.slug)
    .bind(&row.title)
    .bind(&row.title_normalized)
    .bind(row.seniority.as_str())
    .bind(row.employment.as_str())
    .bind(row.work_mode.as_str())
    .bind(row.work_mode_detail.as_deref())
    .bind(row.department.as_deref())
    .bind(&row.description_md)
    .bind(&row.description_text)
    .bind(row.summary.as_deref())
    .bind(row.responsibilities_md.as_deref())
    .bind(row.benefits_md.as_deref())
    .bind(row.salary_min_cents)
    .bind(row.salary_max_cents)
    .bind(row.salary_currency.as_deref())
    .bind(row.salary_period.as_str())
    .bind(i64::from(row.salary_is_estimate))
    .bind(row.salary_raw.as_deref())
    .bind(row.posted_at.as_deref())
    .bind(row.posted_precision.as_deref())
    .bind(row.closes_at.as_deref())
    .bind(row.closes_precision.as_deref())
    .bind(&ts)
    .bind(row.status.as_str())
    .bind(row.apply_url.as_deref())
    .bind(row.apply_kind.as_str())
    .bind(row.requires_clearance.as_deref())
    .bind(row.visa.as_str())
    .bind(row.travel_pct.map(i64::from))
    .bind(row.education_min.as_str())
    .bind(row.years_min.map(f64::from))
    .bind(&row.content_hash)
    .bind(row.extraction_model.as_deref())
    .bind(&row.extracted_at)
    .bind(row.extraction_confidence.map(f64::from))
    .bind(i64::from(row.extraction_partial))
    .execute(&mut **tx)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn update_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job_id: &JobId,
    company_id: &CompanyId,
    input: &PersistExtracted<'_>,
) -> Result<()> {
    let mut row = JobRow::from_input(input);
    // FR-E-03: a human title edit is never clobbered by a later extract.
    let manual_title: Option<String> = sqlx::query_scalar(
        "SELECT field FROM field_provenance
          WHERE entity_kind = 'job' AND entity_id = ?1 AND field = 'title' AND provenance = 'manual'",
    )
    .bind(job_id.as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(db_err)?;
    if manual_title.is_some() {
        let existing: (String, String, String) =
            sqlx::query_as("SELECT title, slug, title_normalized FROM job WHERE id = ?1")
                .bind(job_id.as_str())
                .fetch_one(&mut **tx)
                .await
                .map_err(db_err)?;
        row.title = existing.0;
        row.slug = existing.1;
        row.title_normalized = existing.2;
    }
    let ts = to_rfc3339(&now());
    sqlx::query(
        r#"UPDATE job SET
            company_id = ?2, slug = ?3, title = ?4, title_normalized = ?5,
            seniority = ?6, employment_type = ?7, work_mode = ?8,
            work_mode_detail = ?9, department = ?10,
            description_md = ?11, description_text = ?12, summary = ?13,
            responsibilities_md = ?14, benefits_md = ?15,
            salary_min_cents = ?16, salary_max_cents = ?17, salary_currency = ?18,
            salary_period = ?19, salary_is_estimate = ?20, salary_raw = ?21,
            posted_at = ?22, posted_at_precision = ?23, closes_at = ?24,
            closes_at_precision = ?25, last_seen_at = ?26, status = ?27,
            apply_url = ?28, apply_kind = ?29, requires_clearance = ?30,
            visa_sponsorship = ?31, travel_pct = ?32, education_min = ?33,
            years_experience_min = ?34, content_hash = ?35, extraction_model = ?36,
            extracted_at = ?37, extraction_confidence = ?38, extraction_partial = ?39,
            updated_at = ?26
          WHERE id = ?1"#,
    )
    .bind(job_id.as_str())
    .bind(company_id.as_str())
    .bind(&row.slug)
    .bind(&row.title)
    .bind(&row.title_normalized)
    .bind(row.seniority.as_str())
    .bind(row.employment.as_str())
    .bind(row.work_mode.as_str())
    .bind(row.work_mode_detail.as_deref())
    .bind(row.department.as_deref())
    .bind(&row.description_md)
    .bind(&row.description_text)
    .bind(row.summary.as_deref())
    .bind(row.responsibilities_md.as_deref())
    .bind(row.benefits_md.as_deref())
    .bind(row.salary_min_cents)
    .bind(row.salary_max_cents)
    .bind(row.salary_currency.as_deref())
    .bind(row.salary_period.as_str())
    .bind(i64::from(row.salary_is_estimate))
    .bind(row.salary_raw.as_deref())
    .bind(row.posted_at.as_deref())
    .bind(row.posted_precision.as_deref())
    .bind(row.closes_at.as_deref())
    .bind(row.closes_precision.as_deref())
    .bind(&ts)
    .bind(row.status.as_str())
    .bind(row.apply_url.as_deref())
    .bind(row.apply_kind.as_str())
    .bind(row.requires_clearance.as_deref())
    .bind(row.visa.as_str())
    .bind(row.travel_pct.map(i64::from))
    .bind(row.education_min.as_str())
    .bind(row.years_min.map(f64::from))
    .bind(&row.content_hash)
    .bind(row.extraction_model.as_deref())
    .bind(&row.extracted_at)
    .bind(row.extraction_confidence.map(f64::from))
    .bind(i64::from(row.extraction_partial))
    .execute(&mut **tx)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn replace_locations(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job_id: &JobId,
    extracted: &ExtractedJob,
) -> Result<()> {
    sqlx::query("DELETE FROM job_location WHERE job_id = ?1")
        .bind(job_id.as_str())
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;

    for (ordinal, loc) in extracted.locations.iter().enumerate() {
        let parsed = parse_location(&loc.value);
        let raw = loc.value.text.clone();
        let (city, region, country, postal, remote, tz) = match parsed {
            Some(p) => (
                p.city,
                p.region,
                p.country,
                p.postal_code,
                p.is_remote_scope,
                p.timezone_requirement,
            ),
            None => (None, None, None, None, loc.value.is_remote_hint, None),
        };
        let id = uuid::Uuid::now_v7().to_string();
        sqlx::query(
            r#"INSERT INTO job_location
                (id, job_id, raw, city, region, country, postal_code,
                 is_primary, is_remote_scope, timezone_requirement, ordinal)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"#,
        )
        .bind(&id)
        .bind(job_id.as_str())
        .bind(&raw)
        .bind(city)
        .bind(region)
        .bind(country)
        .bind(postal)
        .bind(i64::from(ordinal == 0))
        .bind(i64::from(remote))
        .bind(tz)
        .bind(ordinal as i64)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

async fn replace_requirements(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job_id: &JobId,
    requirements: &[AtomizedRequirement],
) -> Result<()> {
    sqlx::query("DELETE FROM requirement WHERE job_id = ?1")
        .bind(job_id.as_str())
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;

    let ts = to_rfc3339(&now());
    let mut seen = std::collections::HashSet::new();
    for (ordinal, req) in requirements.iter().enumerate() {
        if !seen.insert(req.normalized_text.clone()) {
            continue;
        }
        let id = RequirementId::new();
        let (span_start, span_end) = match req.source_span {
            Some((a, b)) => (Some(a as i64), Some(b as i64)),
            None => (None, None),
        };
        sqlx::query(
            r#"INSERT INTO requirement (
                id, job_id, ordinal, text, normalized_text, kind, necessity,
                min_years, max_years, education_level, is_blocker, quantity_raw,
                span_start, span_end, confidence, provenance, created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 'rules', ?16, ?16
            )"#,
        )
        .bind(id.as_str())
        .bind(job_id.as_str())
        .bind(ordinal as i64)
        .bind(&req.text)
        .bind(&req.normalized_text)
        .bind(req.kind.as_str())
        .bind(req.necessity.as_str())
        .bind(req.min_years.map(f64::from))
        .bind(req.max_years.map(f64::from))
        .bind(req.education_level.map(|e| e.as_str().to_string()))
        .bind(i64::from(req.is_blocker))
        .bind(req.quantity_raw.as_deref())
        .bind(span_start)
        .bind(span_end)
        .bind(0.6_f64)
        .bind(&ts)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

/// Write provenance for extracted fields without clobbering a manual edit.
pub async fn write_job_provenance(
    db: &Db,
    job_id: &JobId,
    extracted: &ExtractedJob,
    model: Option<&str>,
) -> Result<()> {
    let ts = to_rfc3339(&now());
    let pairs: Vec<(&str, Option<(jobseeker_core::provenance::Provenance, f32)>)> = vec![
        (
            "title",
            extracted
                .title
                .as_ref()
                .map(|s| (s.provenance, s.confidence.get())),
        ),
        (
            "company_name",
            extracted
                .company_name
                .as_ref()
                .map(|s| (s.provenance, s.confidence.get())),
        ),
        (
            "work_mode",
            extracted
                .work_mode
                .as_ref()
                .map(|s| (s.provenance, s.confidence.get())),
        ),
        (
            "salary",
            extracted
                .salary
                .as_ref()
                .map(|s| (s.provenance, s.confidence.get())),
        ),
        (
            "posted_at",
            extracted
                .posted_at
                .as_ref()
                .map(|s| (s.provenance, s.confidence.get())),
        ),
        (
            "apply_url",
            extracted
                .apply_url
                .as_ref()
                .map(|s| (s.provenance, s.confidence.get())),
        ),
    ];
    for (field, src) in pairs {
        let Some((prov, conf)) = src else { continue };
        let id = uuid::Uuid::now_v7().to_string();
        sqlx::query(
            r#"INSERT INTO field_provenance
                (id, entity_kind, entity_id, field, provenance, confidence, model, updated_at)
               VALUES (?1, 'job', ?2, ?3, ?4, ?5, ?6, ?7)
               ON CONFLICT (entity_kind, entity_id, field) DO UPDATE SET
                 provenance = excluded.provenance,
                 confidence = excluded.confidence,
                 model = excluded.model,
                 updated_at = excluded.updated_at
               WHERE field_provenance.provenance != 'manual'"#,
        )
        .bind(&id)
        .bind(job_id.as_str())
        .bind(field)
        .bind(prov.as_str())
        .bind(conf as f64)
        .bind(model)
        .bind(&ts)
        .execute(db.writer())
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

/// A job reconstructed from `job.json` + `requirements.json`.
#[derive(Debug, Clone)]
pub struct FileJobWrite {
    pub job_id: JobId,
    pub company_name: String,
    pub title: String,
    pub status: String,
    pub work_mode: String,
    pub seniority: String,
    pub employment_type: String,
    pub salary_raw: Option<String>,
    pub apply_url: Option<String>,
    pub posted_at: Option<String>,
    pub closes_at: Option<String>,
    pub locations: Vec<String>,
    pub extraction_partial: bool,
    pub content_hash: String,
    pub description_md: String,
    pub user_rating: Option<i64>,
    pub user_notes_md: Option<String>,
    pub is_archived: bool,
    pub file_path: String,
    pub requirements: Vec<FileReqWrite>,
}

#[derive(Debug, Clone)]
pub struct FileReqWrite {
    pub id: Option<RequirementId>,
    pub text: String,
    pub normalized_text: String,
    pub kind: String,
    pub necessity: String,
    pub min_years: Option<f64>,
    pub is_blocker: bool,
}

/// Record a deletion so `--from-files` cannot resurrect the entity (FR-S-07).
pub async fn record_tombstone(db: &Db, entity_kind: &str, entity_id: &str) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO tombstone (entity_kind, entity_id, deleted_at)
         VALUES (?1, ?2, ?3)",
    )
    .bind(entity_kind)
    .bind(entity_id)
    .bind(to_rfc3339(&now()))
    .execute(db.writer())
    .await
    .map(|_| ())
    .map_err(db_err)
}

/// True when a prior delete was recorded so `--from-files` must not resurrect it.
pub async fn is_tombstoned(db: &Db, entity_kind: &str, entity_id: &str) -> Result<bool> {
    let hit: Option<String> = sqlx::query_scalar(
        "SELECT entity_id FROM tombstone WHERE entity_kind = ?1 AND entity_id = ?2",
    )
    .bind(entity_kind)
    .bind(entity_id)
    .fetch_optional(db.reader())
    .await
    .map_err(db_err)?;
    Ok(hit.is_some())
}

/// Insert or replace a job from its on-disk files. Returns `(id, created)`.
pub async fn upsert_from_file(db: &Db, input: &FileJobWrite) -> Result<(JobId, bool)> {
    let company_id = company::resolve_or_create(db, &input.company_name, None).await?;
    let exists: Option<String> = sqlx::query_scalar("SELECT id FROM job WHERE id = ?1")
        .bind(input.job_id.as_str())
        .fetch_optional(db.reader())
        .await
        .map_err(db_err)?;
    let created = exists.is_none();
    let ts = to_rfc3339(&now());
    let title_normalized = normalize_job_title(&input.title);
    let slug = slugify(&input.title);
    let description_text = input.description_md.clone();
    let status = input.status.parse().unwrap_or(JobStatus::Open);
    let work_mode = input.work_mode.parse().unwrap_or(WorkMode::Unknown);
    let seniority = input.seniority.parse().unwrap_or(Seniority::Unknown);
    let employment = input
        .employment_type
        .parse()
        .unwrap_or(EmploymentType::Unknown);
    let parsed = input
        .salary_raw
        .as_deref()
        .and_then(|raw| parse_salary(raw, None, None));

    let mut tx = db.writer().begin().await.map_err(db_err)?;
    if created {
        sqlx::query(
            r#"INSERT INTO job (
                id, company_id, slug, title, title_normalized,
                seniority, employment_type, work_mode,
                description_md, description_text,
                salary_min_cents, salary_max_cents, salary_currency, salary_period,
                salary_is_estimate, salary_raw,
                posted_at, closes_at, apply_url, status, content_hash,
                extraction_partial, user_rating, user_notes_md, is_archived, file_path,
                first_seen_at, last_seen_at, created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21,
                ?22, ?23, ?24, ?25, ?26, ?27, ?27, ?27, ?27
            )"#,
        )
        .bind(input.job_id.as_str())
        .bind(company_id.as_str())
        .bind(&slug)
        .bind(&input.title)
        .bind(&title_normalized)
        .bind(seniority.as_str())
        .bind(employment.as_str())
        .bind(work_mode.as_str())
        .bind(&input.description_md)
        .bind(&description_text)
        .bind(parsed.as_ref().and_then(|s| s.min_cents))
        .bind(parsed.as_ref().and_then(|s| s.max_cents))
        .bind(parsed.as_ref().map(|s| s.currency.clone()))
        .bind(
            parsed
                .as_ref()
                .map(|s| s.period.as_str())
                .unwrap_or("unknown"),
        )
        .bind(i64::from(parsed.as_ref().is_some_and(|s| s.is_estimate)))
        .bind(input.salary_raw.as_deref())
        .bind(input.posted_at.as_deref())
        .bind(input.closes_at.as_deref())
        .bind(input.apply_url.as_deref())
        .bind(status.as_str())
        .bind(&input.content_hash)
        .bind(i64::from(input.extraction_partial))
        .bind(input.user_rating)
        .bind(input.user_notes_md.as_deref())
        .bind(i64::from(input.is_archived))
        .bind(&input.file_path)
        .bind(&ts)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    } else {
        sqlx::query(
            r#"UPDATE job SET
                company_id = ?2, slug = ?3, title = ?4, title_normalized = ?5,
                seniority = ?6, employment_type = ?7, work_mode = ?8,
                description_md = ?9, description_text = ?10,
                salary_min_cents = ?11, salary_max_cents = ?12, salary_currency = ?13,
                salary_period = ?14, salary_is_estimate = ?15, salary_raw = ?16,
                posted_at = ?17, closes_at = ?18, apply_url = ?19, status = ?20,
                content_hash = ?21, extraction_partial = ?22,
                user_rating = ?23, user_notes_md = ?24, is_archived = ?25,
                file_path = ?26, last_seen_at = ?27, updated_at = ?27, deleted_at = NULL
              WHERE id = ?1"#,
        )
        .bind(input.job_id.as_str())
        .bind(company_id.as_str())
        .bind(&slug)
        .bind(&input.title)
        .bind(&title_normalized)
        .bind(seniority.as_str())
        .bind(employment.as_str())
        .bind(work_mode.as_str())
        .bind(&input.description_md)
        .bind(&description_text)
        .bind(parsed.as_ref().and_then(|s| s.min_cents))
        .bind(parsed.as_ref().and_then(|s| s.max_cents))
        .bind(parsed.as_ref().map(|s| s.currency.clone()))
        .bind(
            parsed
                .as_ref()
                .map(|s| s.period.as_str())
                .unwrap_or("unknown"),
        )
        .bind(i64::from(parsed.as_ref().is_some_and(|s| s.is_estimate)))
        .bind(input.salary_raw.as_deref())
        .bind(input.posted_at.as_deref())
        .bind(input.closes_at.as_deref())
        .bind(input.apply_url.as_deref())
        .bind(status.as_str())
        .bind(&input.content_hash)
        .bind(i64::from(input.extraction_partial))
        .bind(input.user_rating)
        .bind(input.user_notes_md.as_deref())
        .bind(i64::from(input.is_archived))
        .bind(&input.file_path)
        .bind(&ts)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }

    replace_locations_from_raw(&mut tx, &input.job_id, &input.locations).await?;
    replace_requirements_from_file(&mut tx, &input.job_id, &input.requirements).await?;
    tx.commit().await.map_err(db_err)?;
    crate::repo::job::reindex_fts(db, &input.job_id).await?;
    Ok((input.job_id.clone(), created))
}

async fn replace_locations_from_raw(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job_id: &JobId,
    locations: &[String],
) -> Result<()> {
    sqlx::query("DELETE FROM job_location WHERE job_id = ?1")
        .bind(job_id.as_str())
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    for (ordinal, raw) in locations.iter().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let parsed = parse_location(&jobseeker_core::domain::location::RawLocation::new(
            raw.as_str(),
        ));
        let (city, region, country, remote) = match parsed {
            Some(p) => (p.city, p.region, p.country, p.is_remote_scope),
            None => (None, None, None, false),
        };
        let id = uuid::Uuid::now_v7().to_string();
        sqlx::query(
            r#"INSERT INTO job_location
                (id, job_id, raw, city, region, country, is_primary, is_remote_scope, ordinal)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
        )
        .bind(&id)
        .bind(job_id.as_str())
        .bind(raw)
        .bind(city)
        .bind(region)
        .bind(country)
        .bind(i64::from(ordinal == 0))
        .bind(i64::from(remote))
        .bind(ordinal as i64)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

async fn replace_requirements_from_file(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    job_id: &JobId,
    requirements: &[FileReqWrite],
) -> Result<()> {
    sqlx::query("DELETE FROM requirement WHERE job_id = ?1")
        .bind(job_id.as_str())
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    let ts = to_rfc3339(&now());
    let mut seen = std::collections::HashSet::new();
    for (ordinal, req) in requirements.iter().enumerate() {
        if req.normalized_text.is_empty() || !seen.insert(req.normalized_text.clone()) {
            continue;
        }
        let id = match &req.id {
            Some(id) => id.clone(),
            None => RequirementId::new(),
        };
        sqlx::query(
            r#"INSERT INTO requirement (
                id, job_id, ordinal, text, normalized_text, kind, necessity,
                min_years, is_blocker, confidence, provenance, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0.6, 'rules', ?10, ?10)"#,
        )
        .bind(id.as_str())
        .bind(job_id.as_str())
        .bind(ordinal as i64)
        .bind(&req.text)
        .bind(&req.normalized_text)
        .bind(&req.kind)
        .bind(&req.necessity)
        .bind(req.min_years)
        .bind(i64::from(req.is_blocker))
        .bind(&ts)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

/// Record the materialized file directory on the job row.
pub async fn set_file_path(db: &Db, job_id: &JobId, file_path: &str) -> Result<()> {
    sqlx::query("UPDATE job SET file_path = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(file_path)
        .bind(to_rfc3339(&now()))
        .bind(job_id.as_str())
        .execute(db.writer())
        .await
        .map(|_| ())
        .map_err(db_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::listing::{self, UpsertListing};
    use jobseeker_core::domain::enums::{Necessity, RequirementKind, SourceKind};
    use jobseeker_core::domain::location::RawLocation;
    use jobseeker_core::provenance::{Provenance, Sourced};

    fn atom(text: &str) -> AtomizedRequirement {
        AtomizedRequirement {
            text: text.into(),
            normalized_text: text.to_lowercase(),
            kind: RequirementKind::Skill,
            necessity: Necessity::Required,
            min_years: None,
            max_years: None,
            education_level: None,
            is_blocker: false,
            quantity_raw: None,
            source_span: None,
        }
    }

    #[tokio::test]
    async fn persist_round_trip_creates_then_updates() {
        let db = Db::open_in_memory().await.unwrap();
        let (listing_id, _) = listing::upsert_by_url(
            &db,
            &UpsertListing {
                source: SourceKind::Greenhouse,
                url: "https://boards.greenhouse.io/acme/jobs/1".into(),
                url_canonical: "https://boards.greenhouse.io/acme/jobs/1".into(),
                source_job_id: Some("1".into()),
                title_at_source: Some("Staff Engineer".into()),
                company_name_at_source: Some("Acme".into()),
            },
        )
        .await
        .unwrap();

        let mut extracted = ExtractedJob::default();
        extracted.title = Some(Sourced::new("Staff Engineer".into(), Provenance::Jsonld));
        extracted.company_name = Some(Sourced::new("Acme".into(), Provenance::Jsonld));
        extracted.locations.push(Sourced::new(
            RawLocation {
                text: "Remote - US".into(),
                is_remote_hint: true,
                country_hint: Some("US".into()),
            },
            Provenance::Jsonld,
        ));

        let first = persist_extracted(
            &db,
            PersistExtracted {
                listing_id: Some(&listing_id),
                job: &extracted,
                requirements: &[atom("Rust")],
                description_md: "Build things.",
                description_text: "Build things.",
                content_hash: "b3:abc",
                partial: false,
                model: None,
            },
        )
        .await
        .unwrap();
        assert!(first.created);

        extracted.title = Some(Sourced::new(
            "Staff Platform Engineer".into(),
            Provenance::Jsonld,
        ));
        let again = persist_extracted(
            &db,
            PersistExtracted {
                listing_id: Some(&listing_id),
                job: &extracted,
                requirements: &[atom("Rust"), atom("Kubernetes")],
                description_md: "Build the platform.",
                description_text: "Build the platform.",
                content_hash: "b3:def",
                partial: true,
                model: Some("mock"),
            },
        )
        .await
        .unwrap();
        assert!(!again.created);
        assert_eq!(again.job_id, first.job_id);

        let (title, reqs, locs, partial): (String, i64, i64, i64) = sqlx::query_as(
            "SELECT j.title,
                    (SELECT COUNT(*) FROM requirement r WHERE r.job_id = j.id),
                    (SELECT COUNT(*) FROM job_location l WHERE l.job_id = j.id),
                    j.extraction_partial
               FROM job j WHERE j.id = ?1",
        )
        .bind(first.job_id.as_str())
        .fetch_one(db.reader())
        .await
        .unwrap();
        assert_eq!(title, "Staff Platform Engineer");
        assert_eq!(reqs, 2);
        assert_eq!(locs, 1);
        assert_eq!(partial, 1);

        let attached: String =
            sqlx::query_scalar("SELECT job_id FROM job_source_listing WHERE id = ?1")
                .bind(listing_id.as_str())
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(attached, first.job_id.as_str());

        let prov: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM field_provenance WHERE entity_kind = 'job' AND entity_id = ?1",
        )
        .bind(first.job_id.as_str())
        .fetch_one(db.reader())
        .await
        .unwrap();
        assert!(prov >= 2, "title and company_name must have provenance");
    }

    #[tokio::test]
    async fn a_manual_title_survives_reextract() {
        let db = Db::open_in_memory().await.unwrap();
        let (listing_id, _) = listing::upsert_by_url(
            &db,
            &UpsertListing {
                source: SourceKind::Greenhouse,
                url: "https://boards.greenhouse.io/acme/jobs/9".into(),
                url_canonical: "https://boards.greenhouse.io/acme/jobs/9".into(),
                source_job_id: Some("9".into()),
                title_at_source: Some("Original".into()),
                company_name_at_source: Some("Acme".into()),
            },
        )
        .await
        .unwrap();
        let mut extracted = ExtractedJob::default();
        extracted.title = Some(Sourced::new("Original".into(), Provenance::Jsonld));
        extracted.company_name = Some(Sourced::new("Acme".into(), Provenance::Jsonld));
        let out = persist_extracted(
            &db,
            PersistExtracted {
                listing_id: Some(&listing_id),
                job: &extracted,
                requirements: &[],
                description_md: "",
                description_text: "",
                content_hash: "b3:a",
                partial: true,
                model: None,
            },
        )
        .await
        .unwrap();

        crate::repo::job::patch(
            &db,
            &out.job_id,
            &crate::repo::job::JobPatch {
                title: Some("Human title".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        extracted.title = Some(Sourced::new("Machine title".into(), Provenance::Jsonld));
        persist_extracted(
            &db,
            PersistExtracted {
                listing_id: Some(&listing_id),
                job: &extracted,
                requirements: &[],
                description_md: "",
                description_text: "",
                content_hash: "b3:b",
                partial: true,
                model: None,
            },
        )
        .await
        .unwrap();

        let detail = crate::repo::job::get(&db, &out.job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.title, "Human title");
        let source: String = sqlx::query_scalar(
            "SELECT provenance FROM field_provenance
              WHERE entity_kind = 'job' AND entity_id = ?1 AND field = 'title'",
        )
        .bind(out.job_id.as_str())
        .fetch_one(db.reader())
        .await
        .unwrap();
        assert_eq!(source, "manual");
    }

    #[tokio::test]
    async fn duplicate_normalized_requirements_are_collapsed() {
        let db = Db::open_in_memory().await.unwrap();
        let mut extracted = ExtractedJob::default();
        extracted.title = Some(Sourced::new("Engineer".into(), Provenance::Rules));
        extracted.company_name = Some(Sourced::new("Acme".into(), Provenance::Rules));

        let out = persist_extracted(
            &db,
            PersistExtracted {
                listing_id: None,
                job: &extracted,
                requirements: &[atom("Rust"), atom("rust")],
                description_md: "",
                description_text: "",
                content_hash: "b3:x",
                partial: true,
                model: None,
            },
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM requirement WHERE job_id = ?1")
            .bind(out.job_id.as_str())
            .fetch_one(db.reader())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
