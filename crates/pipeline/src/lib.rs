//! Task-queue worker and the ingest → extract → materialize spine.
//!
//! Handlers are idempotent: delivery is at-least-once (`docs/adr/0006-sqlite-queue.md`).
//! Raw bytes are persisted before any parsing, so an extraction bug is a re-run, not a
//! lost posting.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use jobseeker_acquire::Acquire;
use jobseeker_core::config::Config;
use jobseeker_core::domain::capture::{CaptureMethod, CaptureSubmission, ExtractStatus};
use jobseeker_core::domain::enums::SourceKind;
use jobseeker_core::domain::event::DomainEvent;
use jobseeker_core::domain::task::TaskKind;
use jobseeker_core::ids::{CaptureId, ListingId, TaskId};
use jobseeker_core::{Error, Result};
use jobseeker_db::persist::{self, PersistExtracted};
use jobseeker_db::queue::{ClaimedTask, NewTask, Queue};
use jobseeker_db::repo::{
    capture, company, event, experience, listing, listing::UpsertListing, profile, score,
};
use jobseeker_db::Db;
use jobseeker_extract::{extract, ExtractInput};
use jobseeker_llm::LlmClient;
use jobseeker_normalize::canonicalize;
use jobseeker_store::discover::{self, DiscoveredJob};
use jobseeker_store::jobfile::{self, FileJob, FileRequirement, JobDocument};
use jobseeker_store::{atomic_write, job_dir, read_blob, to_stable_json, write_blob};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast;

/// Runtime the worker and the HTTP handlers share.
#[derive(Clone)]
pub struct Pipeline {
    pub db: Db,
    pub config: Arc<Config>,
    pub acquire: Arc<Acquire>,
    pub llm: Option<Arc<dyn LlmClient>>,
    events: broadcast::Sender<DomainEvent>,
}

/// What an ingest endpoint returns: the work is queued, not finished.
#[derive(Debug, Clone, Serialize)]
pub struct IngestAccepted {
    pub task_id: TaskId,
    pub listing_id: Option<ListingId>,
    pub capture_id: Option<CaptureId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IngestUrlPayload {
    url: String,
    listing_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExtractPayload {
    capture_id: String,
    listing_id: Option<String>,
    #[serde(default)]
    page_meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JobPayload {
    job_id: String,
}

impl Pipeline {
    pub async fn open(config: Config) -> Result<Self> {
        let data_dir = &config.data.dir;
        std::fs::create_dir_all(data_dir)?;
        let db = Db::open(&config.db_path(), 4).await?;
        if config.data.run_migrations_on_start {
            db.migrate().await?;
        }
        db.assert_schema_not_newer().await?;
        jobseeker_db::repo::profile::ensure_default(&db).await?;
        let acquire = Acquire::new(&config.acquire)?;
        let llm = jobseeker_llm::from_config(&config.llm)?;
        let (events, _) = broadcast::channel(256);
        Ok(Self {
            db,
            config: Arc::new(config),
            acquire: Arc::new(acquire),
            llm,
            events,
        })
    }

    /// Subscribe to live domain events. Lagged receivers skip ahead; the SSE
    /// endpoint backfills from `event_log` so a dropped frame is not lost work.
    pub fn subscribe(&self) -> broadcast::Receiver<DomainEvent> {
        self.events.subscribe()
    }

    /// Persist an event, then notify live subscribers. Broadcast failure (no
    /// listeners) is ignored — the log is the source of truth for reconnects.
    pub async fn emit(
        &self,
        kind: &str,
        entity_kind: &str,
        entity_id: &str,
        payload: serde_json::Value,
    ) -> Result<DomainEvent> {
        let ev = event::append(&self.db, kind, entity_kind, entity_id, payload).await?;
        let _ = self.events.send(ev.clone());
        Ok(ev)
    }

    async fn emit_task_updated(&self, task_id: &TaskId) {
        match self.queue().get(task_id).await {
            Ok(Some(view)) => {
                let payload = match serde_json::to_value(&view) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(error = %e, "task.updated payload");
                        return;
                    }
                };
                if let Err(e) = self
                    .emit(DomainEvent::TASK_UPDATED, "task", task_id.as_str(), payload)
                    .await
                {
                    tracing::warn!(error = %e, "failed to emit task.updated");
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "task.updated lookup"),
        }
    }

    pub fn queue(&self) -> Queue<'_> {
        Queue::new(
            &self.db,
            self.config.worker.lease_seconds,
            self.config.worker.max_attempts,
        )
    }

    pub fn data_dir(&self) -> &Path {
        &self.config.data.dir
    }

    /// Plan + enqueue a public URL. Authenticated aggregators fail here with
    /// [`Error::NeedsBrowser`] rather than being queued to fail later.
    pub async fn ingest_url(&self, url: &str) -> Result<IngestAccepted> {
        let planned = jobseeker_acquire::plan(url)?;
        let (listing_id, _) = listing::upsert_by_url(
            &self.db,
            &UpsertListing {
                source: planned.source,
                url: planned.canonical.original.clone(),
                url_canonical: planned.canonical.canonical.clone(),
                source_job_id: planned.canonical.source_job_id.clone(),
                title_at_source: None,
                company_name_at_source: None,
            },
        )
        .await?;
        let task_id = self
            .queue()
            .enqueue(
                NewTask::new(
                    TaskKind::IngestUrl,
                    json!({
                        "url": url,
                        "listing_id": listing_id.as_str(),
                    }),
                )
                .dedupe(format!("ingest_url:{}", planned.canonical.url_hash())),
            )
            .await?;
        Ok(IngestAccepted {
            task_id,
            listing_id: Some(listing_id),
            capture_id: None,
        })
    }

    /// Persist pasted text/HTML first, then queue extraction.
    pub async fn ingest_paste(&self, body: &str, url: Option<&str>) -> Result<IngestAccepted> {
        let listing_id = if let Some(url) = url {
            Some(self.upsert_listing(url).await?.0)
        } else {
            None
        };
        let bytes = body.as_bytes();
        let ext = if body.trim_start().starts_with('<') {
            "html"
        } else {
            "txt"
        };
        let (hash, path) = write_blob(self.data_dir(), bytes, ext)?;
        let capture_id = capture::insert(
            &self.db,
            &capture::NewCapture {
                listing_id: listing_id.clone(),
                url: url.map(str::to_string),
                method: CaptureMethod::Paste,
                http_status: None,
                content_type: Some(if ext == "html" {
                    "text/html".into()
                } else {
                    "text/plain".into()
                }),
                byte_len: bytes.len() as i64,
                content_hash: hash,
                storage_path: relative_path(self.data_dir(), &path),
                screenshot_path: None,
                captured_at: None,
                user_agent: None,
                client_version: None,
                notes: None,
            },
        )
        .await?;
        let task_id = self
            .enqueue_extract(&capture_id, listing_id.as_ref(), None)
            .await?;
        Ok(IngestAccepted {
            task_id,
            listing_id,
            capture_id: Some(capture_id),
        })
    }

    /// Persist an extension capture first, then queue extraction.
    pub async fn ingest_capture(&self, sub: CaptureSubmission) -> Result<IngestAccepted> {
        let (listing_id, _) = self.upsert_listing(&sub.url).await?;
        let html = sub.selected_html.as_deref().unwrap_or(&sub.html);
        let (hash, path) = write_blob(self.data_dir(), html.as_bytes(), "html")?;
        let capture_id = capture::insert(
            &self.db,
            &capture::NewCapture {
                listing_id: Some(listing_id.clone()),
                url: Some(sub.url.clone()),
                method: CaptureMethod::Extension,
                http_status: Some(200),
                content_type: Some("text/html".into()),
                byte_len: html.len() as i64,
                content_hash: hash,
                storage_path: relative_path(self.data_dir(), &path),
                screenshot_path: None,
                captured_at: sub.captured_at,
                user_agent: sub.client.as_ref().and_then(|c| c.browser.clone()),
                client_version: sub
                    .client
                    .as_ref()
                    .map(|c| format!("{}/{}", c.name, c.version)),
                notes: sub.notes.clone(),
            },
        )
        .await?;
        let task_id = self
            .enqueue_extract(&capture_id, Some(&listing_id), sub.page_meta)
            .await?;
        Ok(IngestAccepted {
            task_id,
            listing_id: Some(listing_id),
            capture_id: Some(capture_id),
        })
    }

    async fn upsert_listing(&self, url: &str) -> Result<(ListingId, bool)> {
        let canonical = canonicalize(url)?;
        listing::upsert_by_url(
            &self.db,
            &UpsertListing {
                source: canonical.source,
                url: canonical.original.clone(),
                url_canonical: canonical.canonical.clone(),
                source_job_id: canonical.source_job_id.clone(),
                title_at_source: None,
                company_name_at_source: None,
            },
        )
        .await
    }

    async fn enqueue_extract(
        &self,
        capture_id: &CaptureId,
        listing_id: Option<&ListingId>,
        page_meta: Option<serde_json::Value>,
    ) -> Result<TaskId> {
        self.queue()
            .enqueue(
                NewTask::new(
                    TaskKind::ExtractJob,
                    json!({
                        "capture_id": capture_id.as_str(),
                        "listing_id": listing_id.map(|l| l.as_str().to_string()),
                        "page_meta": page_meta,
                    }),
                )
                .dedupe(format!("extract:{}", capture_id.as_str())),
            )
            .await
    }

    /// Claim and run every currently available task. Used by the CLI so `add` is synchronous.
    pub async fn drain(&self) -> Result<u32> {
        let mut n = 0;
        while self.drain_one().await? {
            n += 1;
        }
        Ok(n)
    }

    /// Claim at most one task. Returns `true` when work was done.
    pub async fn drain_one(&self) -> Result<bool> {
        let q = self.queue();
        q.reclaim_expired_leases().await?;
        let Some(task) = q.claim().await? else {
            return Ok(false);
        };
        match self.handle(&task).await {
            Ok(()) => q.complete(&task.id).await?,
            Err(e) => {
                tracing::warn!(task = %task.id, kind = ?task.kind, error = %e, "task failed");
                q.fail(&task, &e).await?;
            }
        }
        self.emit_task_updated(&task.id).await;
        Ok(true)
    }

    /// Poll the queue until `stop` is set.
    pub async fn run_worker(&self, mut stop: tokio::sync::watch::Receiver<bool>) {
        let interval = Duration::from_millis(self.config.worker.poll_interval_ms.max(50));
        loop {
            if *stop.borrow() {
                break;
            }
            match self.drain_one().await {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => tracing::error!(error = %e, "worker loop error"),
            }
            tokio::select! {
                _ = stop.changed() => {
                    if *stop.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(interval) => {}
            }
        }
    }

    async fn handle(&self, task: &ClaimedTask) -> Result<()> {
        match task.kind {
            TaskKind::IngestUrl => self.handle_ingest_url(&task.payload).await,
            TaskKind::IngestCapture => {
                let capture_id: CaptureId = task
                    .payload
                    .get("capture_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::BadRequest("ingest_capture missing capture_id".into()))?
                    .parse()?;
                self.enqueue_extract(&capture_id, None, None).await?;
                Ok(())
            }
            TaskKind::ExtractJob => self.handle_extract(&task.payload).await,
            TaskKind::MaterializeJob => {
                let p: JobPayload = serde_json::from_value(task.payload.clone())?;
                let job_id: jobseeker_core::ids::JobId = p.job_id.parse()?;
                let rel = self.materialize_job(&job_id).await?;
                if let Err(e) = self
                    .emit(
                        DomainEvent::JOB_UPDATED,
                        "job",
                        job_id.as_str(),
                        json!({ "file_path": rel }),
                    )
                    .await
                {
                    tracing::warn!(error = %e, "failed to emit job.updated");
                }
                Ok(())
            }
            TaskKind::ScoreMatch => self.handle_score_match(&task.payload).await,
            TaskKind::ImportResume => {
                let text = task
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::BadRequest("import_resume missing text".into()))?;
                self.import_resume(text).await?;
                Ok(())
            }
            other => {
                tracing::info!(kind = other.as_str(), "no handler yet; marking done");
                Ok(())
            }
        }
    }

    async fn handle_ingest_url(&self, payload: &serde_json::Value) -> Result<()> {
        let p: IngestUrlPayload = serde_json::from_value(payload.clone())?;
        let listing_id: ListingId = p.listing_id.parse()?;
        let outcome = self.acquire.fetch_url(&p.url).await?;
        if outcome.looks_closed() {
            listing::mark_closed(&self.db, &listing_id).await?;
            return Ok(());
        }
        let hash = outcome.content_hash();
        if capture::content_seen(&self.db, &listing_id, &hash).await? {
            if let Some(existing) = capture::latest_for_listing(&self.db, &listing_id).await? {
                self.enqueue_extract(&existing, Some(&listing_id), None)
                    .await?;
            }
            return Ok(());
        }
        let ext = content_ext(outcome.content_type.as_deref());
        let (stored_hash, path) = write_blob(self.data_dir(), &outcome.body, ext)?;
        let capture_id = capture::insert(
            &self.db,
            &capture::NewCapture {
                listing_id: Some(listing_id.clone()),
                url: Some(outcome.final_url.clone()),
                method: if ext == "json" {
                    CaptureMethod::Api
                } else {
                    CaptureMethod::Http
                },
                http_status: Some(outcome.status),
                content_type: outcome.content_type.clone(),
                byte_len: outcome.body.len() as i64,
                content_hash: stored_hash,
                storage_path: relative_path(self.data_dir(), &path),
                screenshot_path: None,
                captured_at: None,
                user_agent: Some(self.config.acquire.user_agent.clone()),
                client_version: Some(format!("jobseeker/{}", jobseeker_core::VERSION)),
                notes: None,
            },
        )
        .await?;
        self.enqueue_extract(&capture_id, Some(&listing_id), None)
            .await?;
        Ok(())
    }

    async fn handle_extract(&self, payload: &serde_json::Value) -> Result<()> {
        let p: ExtractPayload = serde_json::from_value(payload.clone())?;
        let capture_id: CaptureId = p.capture_id.parse()?;
        let row = capture::get(&self.db, &capture_id)
            .await?
            .ok_or(Error::NotFound("capture"))?;
        let abs = self.data_dir().join(&row.storage_path);
        let bytes = read_blob(&abs)?;
        let body = String::from_utf8_lossy(&bytes).into_owned();

        let listing_id = match &row.listing_id {
            Some(id) => Some(id.clone()),
            None => p.listing_id.as_deref().and_then(|s| s.parse().ok()),
        };
        let listing = match listing_id.as_ref() {
            Some(id) => listing::get(&self.db, id).await?,
            None => None,
        };
        let source = listing
            .as_ref()
            .map(|l| l.source)
            .or_else(|| {
                row.url
                    .as_deref()
                    .and_then(|u| canonicalize(u).ok())
                    .map(|c| c.source)
            })
            .unwrap_or(SourceKind::Manual);

        let input = ExtractInput {
            // Prefer the listing URL the user asked for. The capture URL is often an
            // ATS JSON endpoint (Workday CXS, Greenhouse board API) and must not become
            // the apply link.
            url: listing.as_ref().map(|l| l.url.clone()).or(row.url.clone()),
            body,
            content_type: row.content_type.clone(),
            method: row.method,
            source,
            page_meta: p.page_meta,
        };
        let llm = self.llm.as_deref();
        let out = extract(&input, llm, self.config.llm.max_input_tokens).await?;

        let listing_id = listing.as_ref().map(|l| l.id.clone());
        let persisted = persist::persist_extracted(
            &self.db,
            PersistExtracted {
                listing_id: listing_id.as_ref(),
                job: &out.job,
                requirements: &out.requirements,
                description_md: &out.description_md,
                description_text: &out.description_text,
                content_hash: &row.content_hash,
                partial: out.partial,
                model: out.model.as_deref(),
            },
        )
        .await?;

        capture::set_extract_status(&self.db, &capture_id, ExtractStatus::Ok, None).await?;

        self.queue()
            .enqueue(
                NewTask::new(
                    TaskKind::MaterializeJob,
                    json!({ "job_id": persisted.job_id.as_str() }),
                )
                .dedupe(format!("materialize:{}", persisted.job_id.as_str())),
            )
            .await?;
        self.queue()
            .enqueue(
                NewTask::new(
                    TaskKind::ScoreMatch,
                    json!({ "job_id": persisted.job_id.as_str() }),
                )
                .dedupe(format!("score:{}", persisted.job_id.as_str())),
            )
            .await?;

        let kind = if persisted.created {
            DomainEvent::JOB_CREATED
        } else {
            DomainEvent::JOB_UPDATED
        };
        if let Err(e) = self
            .emit(
                kind,
                "job",
                persisted.job_id.as_str(),
                json!({ "job_id": persisted.job_id.as_str() }),
            )
            .await
        {
            tracing::warn!(error = %e, "failed to emit job event");
        }
        Ok(())
    }

    /// Write `job.md` / `job.json` / `requirements.json` for one job.
    pub async fn materialize_job(&self, job_id: &jobseeker_core::ids::JobId) -> Result<String> {
        let detail = jobseeker_db::repo::job::get(&self.db, job_id)
            .await?
            .ok_or(Error::NotFound("job"))?;
        let company = company::get(&self.db, &detail.company_id.parse()?)
            .await?
            .ok_or(Error::NotFound("company"))?;

        let posted = detail
            .posted_at
            .as_deref()
            .map(|s| s.chars().take(10).collect::<String>());
        let dir = job_dir(
            self.data_dir(),
            &company.slug,
            posted.as_deref(),
            &detail.title,
            job_id,
        );

        let salary = match (
            detail.salary_min_cents,
            detail.salary_max_cents,
            detail.salary_raw.as_deref(),
        ) {
            (_, _, Some(raw)) if !raw.is_empty() => Some(raw.to_string()),
            (Some(min), Some(max), _) => Some(format!("{min}–{max} {}", detail.salary_period)),
            _ => None,
        };
        let doc = JobDocument {
            title: detail.title.clone(),
            company: company.name.clone(),
            status: detail.status.clone(),
            work_mode: Some(detail.work_mode.clone()),
            employment_type: Some(detail.employment_type.clone()),
            seniority: Some(detail.seniority.clone()),
            salary,
            apply_url: detail.apply_url.clone(),
            posted_at: posted.clone(),
            closes_at: detail.closes_at.clone(),
            content_hash: detail.content_hash.clone(),
            extraction_partial: detail.extraction_partial,
        };

        let atoms: Vec<jobseeker_normalize::requirement::AtomizedRequirement> = detail
            .requirements
            .iter()
            .filter_map(|r| {
                Some(jobseeker_normalize::requirement::AtomizedRequirement {
                    text: r.text.clone(),
                    normalized_text: r.normalized_text.clone(),
                    kind: r.kind.parse().ok()?,
                    necessity: r.necessity.parse().ok()?,
                    min_years: r.min_years.map(|y| y as f32),
                    max_years: None,
                    education_level: None,
                    is_blocker: r.is_blocker,
                    quantity_raw: None,
                    source_span: None,
                })
            })
            .collect();

        let md = jobfile::render_markdown(&doc, &detail.description_md, &atoms);
        atomic_write(&dir.join("job.md"), md.as_bytes())?;

        let file_job = FileJob {
            id: detail.id.clone(),
            title: detail.title.clone(),
            company: company.name.clone(),
            status: detail.status.clone(),
            work_mode: detail.work_mode.clone(),
            seniority: detail.seniority.clone(),
            employment_type: detail.employment_type.clone(),
            salary_raw: detail.salary_raw.clone(),
            apply_url: detail.apply_url.clone(),
            posted_at: detail.posted_at.clone(),
            closes_at: detail.closes_at.clone(),
            locations: detail.locations.clone(),
            extraction_partial: detail.extraction_partial,
            content_hash: detail.content_hash.clone(),
            description_md: detail.description_md.clone(),
            user_rating: detail.user_rating,
            user_notes_md: detail.user_notes_md.clone(),
            is_archived: detail.is_archived,
        };
        atomic_write(&dir.join("job.json"), &to_stable_json(&file_job)?)?;
        let reqs: Vec<FileRequirement> = detail
            .requirements
            .iter()
            .map(|r| FileRequirement {
                id: r.id.clone(),
                text: r.text.clone(),
                normalized_text: r.normalized_text.clone(),
                kind: r.kind.clone(),
                necessity: r.necessity.clone(),
                min_years: r.min_years,
                is_blocker: r.is_blocker,
            })
            .collect();
        atomic_write(&dir.join("requirements.json"), &to_stable_json(&reqs)?)?;

        let rel = relative_path(self.data_dir(), &dir);
        persist::set_file_path(&self.db, job_id, &rel).await?;
        Ok(rel)
    }

    async fn handle_score_match(&self, payload: &serde_json::Value) -> Result<()> {
        let p: JobPayload = serde_json::from_value(payload.clone())?;
        let job_id: jobseeker_core::ids::JobId = p.job_id.parse()?;
        let Some((job, requirements, locations)) =
            jobseeker_db::repo::job::scoring_inputs(&self.db, &job_id).await?
        else {
            return Err(Error::NotFound("job"));
        };
        let Some(profile_row) = profile::get(&self.db, None).await? else {
            tracing::warn!("score_match skipped: no default profile");
            return Ok(());
        };
        let view = experience::get_view(&self.db, &profile_row.id).await?;
        let (seniority, years_experience, education) =
            scoring_identity(&profile_row, view.as_ref());

        let snapshot = jobseeker_matching::JobSnapshot {
            job_id: Some(job.id.clone()),
            requirements,
            seniority: job.seniority,
            work_mode: job.work_mode,
            salary: job.salary,
            locations,
            requires_clearance: job.requires_clearance,
        };
        let profile = jobseeker_matching::ProfileSnapshot {
            profile_id: Some(profile_row.id.clone()),
            skills: profile_row
                .skills
                .into_iter()
                .map(|s| jobseeker_matching::SkillEvidence {
                    skill_id: s.skill_id,
                    slug: s.slug,
                    years: s.years,
                    last_used_year: s.last_used_year,
                    excerpt: None,
                })
                .collect(),
            seniority: Some(seniority),
            years_experience: Some(years_experience),
            target_comp_min_cents: profile_row.target_comp_min_cents,
            accepts_remote: profile_row.accepts_remote,
            willing_to_relocate: profile_row.willing_to_relocate,
            target_locations: profile_row.target_locations,
            clearances: vec![],
            education,
        };
        let score = jobseeker_matching::score(&snapshot, &profile, &self.config.matching)?;
        score::upsert(&self.db, &score).await?;
        if let Err(e) = self
            .emit(
                DomainEvent::MATCH_UPDATED,
                "job",
                job_id.as_str(),
                json!({
                    "job_id": job_id.as_str(),
                    "profile_id": profile_row.id.as_str(),
                    "overall": score.overall,
                    "blocker_count": score.blockers.len(),
                }),
            )
            .await
        {
            tracing::warn!(error = %e, "failed to emit match.updated");
        }
        Ok(())
    }

    /// Parse a Markdown evidence bank or resume, replace the default profile, and rescore.
    pub async fn import_resume(&self, markdown: &str) -> Result<experience::ImportReport> {
        let parsed = jobseeker_resume::import::parse_markdown(markdown);
        if parsed.items.is_empty() && parsed.skills.is_empty() {
            return Err(Error::BadRequest(
                "no experience items or skills found in the Markdown".into(),
            ));
        }
        let profile_id = profile::ensure_default(&self.db).await?;
        let write = bank_write(&parsed)?;
        let report = experience::replace_bank(&self.db, &profile_id, &write).await?;
        self.materialize_profile(&profile_id).await?;
        score::mark_stale_for_profile(&self.db, &profile_id).await?;
        let job_ids = jobseeker_db::repo::job::list_ids(&self.db).await?;
        // Score here, not via the queue: `score:{job}` is already a completed
        // dedupe key from first ingest, so a re-enqueue would be dropped.
        for job_id in &job_ids {
            self.handle_score_match(&json!({ "job_id": job_id.as_str() }))
                .await?;
        }
        if let Err(e) = self
            .emit(
                DomainEvent::PROFILE_UPDATED,
                "profile",
                profile_id.as_str(),
                json!({
                    "items": report.items,
                    "accomplishments": report.accomplishments,
                    "skills": report.skills,
                    "rescored_jobs": job_ids.len(),
                }),
            )
            .await
        {
            tracing::warn!(error = %e, "failed to emit profile.updated");
        }
        Ok(report)
    }

    async fn materialize_profile(&self, profile_id: &jobseeker_core::ids::ProfileId) -> Result<()> {
        let Some(view) = experience::get_view(&self.db, profile_id).await? else {
            return Ok(());
        };
        let dir = self.data_dir().join("profiles").join("default");
        atomic_write(&dir.join("profile.json"), &to_stable_json(&view)?)?;
        atomic_write(
            &dir.join("experience.json"),
            &to_stable_json(&view.experience)?,
        )?;
        atomic_write(&dir.join("skills.json"), &to_stable_json(&view.skills)?)?;
        Ok(())
    }

    /// Compare the database to `jobs/**/job.json`. Changes nothing.
    pub async fn reconcile_check(&self) -> Result<ReconcileReport> {
        let files = discover::discover_jobs(self.data_dir())?;
        let db_rows = jobseeker_db::repo::job::list_reconcile_rows(&self.db).await?;
        Ok(diff_reconcile(&db_rows, &files))
    }

    /// Rewrite every job's files from the database.
    pub async fn reconcile_to_files(&self) -> Result<ReconcileReport> {
        let db_rows = jobseeker_db::repo::job::list_reconcile_rows(&self.db).await?;
        let mut written = 0u32;
        for row in &db_rows {
            let id: jobseeker_core::ids::JobId = row.id.parse()?;
            self.materialize_job(&id).await?;
            written += 1;
        }
        let mut report = self.reconcile_check().await?;
        report.written = written;
        Ok(report)
    }

    /// Upsert jobs from `jobs/**/job.json`. Tombstoned ids are skipped (FR-S-07).
    pub async fn reconcile_from_files(&self) -> Result<ReconcileReport> {
        let files = discover::discover_jobs(self.data_dir())?;
        let mut restored = 0u32;
        let mut skipped_tombstone = 0u32;
        for discovered in &files {
            if persist::is_tombstoned(&self.db, "job", &discovered.job.id).await? {
                skipped_tombstone += 1;
                continue;
            }
            let write = file_job_write(discovered)?;
            persist::upsert_from_file(&self.db, &write).await?;
            restored += 1;
        }
        let mut report = self.reconcile_check().await?;
        report.restored = restored;
        report.skipped_tombstone = skipped_tombstone;
        Ok(report)
    }
}

/// What `reconcile --check` prints: which side is missing a job, and whose hash drifted.
#[derive(Debug, Clone, Default)]
pub struct ReconcileReport {
    pub db_jobs: u32,
    pub file_jobs: u32,
    pub written: u32,
    pub restored: u32,
    pub skipped_tombstone: u32,
    pub db_only: Vec<String>,
    pub file_only: Vec<String>,
    pub divergent: Vec<String>,
}

impl ReconcileReport {
    pub fn is_clean(&self) -> bool {
        self.db_only.is_empty() && self.file_only.is_empty() && self.divergent.is_empty()
    }
}

fn diff_reconcile(
    db_rows: &[jobseeker_db::repo::job::ReconcileRow],
    files: &[DiscoveredJob],
) -> ReconcileReport {
    use std::collections::BTreeMap;
    let db_map: BTreeMap<&str, &jobseeker_db::repo::job::ReconcileRow> =
        db_rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let file_map: BTreeMap<&str, &DiscoveredJob> =
        files.iter().map(|f| (f.job.id.as_str(), f)).collect();
    let mut report = ReconcileReport {
        db_jobs: db_rows.len() as u32,
        file_jobs: files.len() as u32,
        ..Default::default()
    };
    for (id, row) in &db_map {
        match file_map.get(id) {
            None => report.db_only.push((*id).to_string()),
            Some(file) => {
                let hash_drift = file.job.content_hash != row.content_hash;
                let title_drift = file.job.title != row.title;
                if hash_drift || title_drift {
                    report.divergent.push((*id).to_string());
                }
            }
        }
    }
    for id in file_map.keys() {
        if !db_map.contains_key(id) {
            report.file_only.push((*id).to_string());
        }
    }
    report
}

fn file_job_write(discovered: &DiscoveredJob) -> Result<persist::FileJobWrite> {
    let job_id: jobseeker_core::ids::JobId = discovered.job.id.parse()?;
    let requirements = discovered
        .requirements
        .iter()
        .map(|r| persist::FileReqWrite {
            id: r.id.parse().ok(),
            text: r.text.clone(),
            normalized_text: r.normalized_text.clone(),
            kind: r.kind.clone(),
            necessity: r.necessity.clone(),
            min_years: r.min_years,
            is_blocker: r.is_blocker,
        })
        .collect();
    Ok(persist::FileJobWrite {
        job_id,
        company_name: discovered.job.company.clone(),
        title: discovered.job.title.clone(),
        status: discovered.job.status.clone(),
        work_mode: discovered.job.work_mode.clone(),
        seniority: discovered.job.seniority.clone(),
        employment_type: discovered.job.employment_type.clone(),
        salary_raw: discovered.job.salary_raw.clone(),
        apply_url: discovered.job.apply_url.clone(),
        posted_at: discovered.job.posted_at.clone(),
        closes_at: discovered.job.closes_at.clone(),
        locations: discovered.job.locations.clone(),
        extraction_partial: discovered.job.extraction_partial,
        content_hash: discovered.job.content_hash.clone(),
        description_md: discovered.job.description_md.clone(),
        user_rating: discovered.job.user_rating,
        user_notes_md: discovered.job.user_notes_md.clone(),
        is_archived: discovered.job.is_archived,
        file_path: discovered.rel_dir.clone(),
        requirements,
    })
}

fn bank_write(parsed: &jobseeker_resume::import::ParsedBank) -> Result<experience::BankWrite> {
    Ok(experience::BankWrite {
        full_name: parsed.identity.full_name.clone(),
        headline: parsed.identity.headline.clone(),
        email: parsed.identity.email.clone(),
        phone: parsed.identity.phone.clone(),
        location: parsed.identity.location.clone(),
        links_json: serde_json::to_string(&parsed.identity.links)?,
        summary_md: parsed.identity.summary_md.clone(),
        target_titles_json: serde_json::to_string(&parsed.identity.target_titles)?,
        target_comp_min_cents: parsed.identity.target_comp_min_cents,
        target_locations_json: serde_json::to_string(&parsed.identity.target_locations)?,
        accepts_remote: parsed.identity.accepts_remote,
        willing_to_relocate: parsed.identity.willing_to_relocate,
        items: parsed
            .items
            .iter()
            .map(|item| experience::ItemWrite {
                kind: item.kind,
                org: item.org.clone(),
                title: item.title.clone(),
                location: item.location.clone(),
                start_date: item.start_date.clone(),
                end_date: item.end_date.clone(),
                is_current: item.is_current,
                description_md: item.description_md.clone(),
                accomplishments: item
                    .accomplishments
                    .iter()
                    .map(|a| experience::AccomplishmentWrite {
                        text: a.text.clone(),
                        variants: a.variants.clone(),
                        strength: a.strength,
                        verified: a.verified,
                        skill_slugs: a.skill_slugs.clone(),
                    })
                    .collect(),
            })
            .collect(),
        skills: parsed
            .skills
            .iter()
            .map(|s| experience::SkillWrite {
                slug: s.slug.clone(),
                years: s.years,
                last_used_year: s.last_used_year,
                is_primary: s.is_primary,
                evidence_count: s.evidence_count,
            })
            .collect(),
    })
}

fn scoring_identity(
    row: &profile::ProfileRow,
    view: Option<&experience::ProfileView>,
) -> (
    jobseeker_core::domain::enums::Seniority,
    f32,
    Option<jobseeker_core::domain::enums::EducationLevel>,
) {
    use jobseeker_core::domain::enums::{EducationLevel, Seniority};
    let years = view
        .and_then(|v| v.years_experience)
        .or_else(|| {
            row.skills
                .iter()
                .filter_map(|s| s.years)
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        })
        .unwrap_or(0.0);
    let headline = view
        .and_then(|v| v.headline.as_deref())
        .unwrap_or("")
        .to_ascii_lowercase();
    let titles = view
        .map(|v| v.target_titles.join(" ").to_ascii_lowercase())
        .unwrap_or_default();
    let seniority = if headline.contains("mid") || titles.contains("mid-level") {
        Seniority::Mid
    } else {
        Seniority::from_years(years)
    };
    let education = view.and_then(|v| {
        let mut best = None;
        for item in &v.experience {
            if item.kind != "education" || item.is_current {
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
            let level = if blob.contains("master") {
                EducationLevel::Master
            } else if blob.contains("bachelor") {
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
    });
    (seniority, years, education)
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn content_ext(content_type: Option<&str>) -> &'static str {
    match content_type {
        Some(ct) if ct.contains("json") => "json",
        Some(ct) if ct.contains("html") => "html",
        _ => "bin",
    }
}

/// Used by tests that want a pipeline against a temp directory without opening a file DB
/// twice.
pub async fn for_test(dir: PathBuf) -> Result<Pipeline> {
    for_test_with(dir, |_| {}).await
}

/// Build a test pipeline after tweaking config (auth mode, bind, …).
pub async fn for_test_with(dir: PathBuf, tweak: impl FnOnce(&mut Config)) -> Result<Pipeline> {
    let mut config = Config::default();
    config.data.dir = dir;
    config.data.run_migrations_on_start = true;
    config.worker.poll_interval_ms = 50;
    tweak(&mut config);
    Pipeline::open(config).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREENHOUSE_HTML: &str =
        include_str!("../../../fixtures/greenhouse-platform-engineer.html");
    const LINKEDIN_HTML: &str = include_str!("../../../fixtures/linkedin-job-capture.html");

    #[tokio::test]
    async fn paste_travels_the_spine_to_files_and_a_queryable_row() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = for_test(dir.path().to_path_buf()).await.unwrap();

        let accepted = pipe
            .ingest_paste(
                GREENHOUSE_HTML,
                Some("https://boards.greenhouse.io/acmerobotics/jobs/5512034"),
            )
            .await
            .unwrap();
        assert!(accepted.capture_id.is_some());
        assert!(accepted.listing_id.is_some());

        let n = pipe.drain().await.unwrap();
        assert!(n >= 2, "extract + materialize (+ score) must run, got {n}");

        let page =
            jobseeker_db::repo::job::list(&pipe.db, &jobseeker_db::repo::job::JobFilter::default())
                .await
                .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].title, "Senior Platform Engineer");
        assert_eq!(page.items[0].company_name, "Acme Robotics");

        let id: jobseeker_core::ids::JobId = page.items[0].id.parse().unwrap();
        let detail = jobseeker_db::repo::job::get(&pipe.db, &id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            detail
                .requirements
                .iter()
                .any(|r| r.text.contains("Kubernetes")),
            "atomized requirements must be persisted: {:?}",
            detail.requirements
        );
        let file_path = detail.file_path.expect("materialize must set file_path");
        let job_md = dir.path().join(&file_path).join("job.md");
        let job_json = dir.path().join(&file_path).join("job.json");
        let reqs = dir.path().join(&file_path).join("requirements.json");
        assert!(job_md.is_file(), "missing {}", job_md.display());
        assert!(job_json.is_file());
        assert!(reqs.is_file());
        let md = std::fs::read_to_string(&job_md).unwrap();
        assert!(md.contains("Senior Platform Engineer"));
        assert!(md.contains("Acme Robotics"));

        let scored = jobseeker_db::repo::score::latest_for_job(&pipe.db, &id, None)
            .await
            .unwrap()
            .expect("score_match must persist a row");
        assert!(
            scored.overall > 0.0,
            "default profile must produce a positive score, got {}",
            scored.overall
        );
        assert!(
            scored.verdicts.iter().any(|v| {
                v.score > 0.0
                    && (v.rationale.to_ascii_lowercase().contains("rust")
                        || v.rationale.to_ascii_lowercase().contains("kubernetes")
                        || v.status == "met")
            }),
            "rust/k8s evidence must score above 0: {:?}",
            scored.verdicts
        );

        let events = jobseeker_db::repo::event::since(&pipe.db, 0, 200)
            .await
            .unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert!(
            kinds.contains(&DomainEvent::JOB_CREATED),
            "extract must emit job.created: {kinds:?}"
        );
        assert!(
            kinds.contains(&DomainEvent::MATCH_UPDATED),
            "score must emit match.updated: {kinds:?}"
        );
        assert!(
            kinds.contains(&DomainEvent::TASK_UPDATED),
            "drain must emit task.updated: {kinds:?}"
        );
    }

    #[tokio::test]
    async fn authenticated_urls_are_refused_before_a_task_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = for_test(dir.path().to_path_buf()).await.unwrap();
        let err = pipe
            .ingest_url("https://www.linkedin.com/jobs/view/4123456789")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "needs_browser");
        let depth = pipe.queue().depth().await.unwrap();
        assert!(depth.is_empty(), "nothing should have been queued");
    }

    #[tokio::test]
    async fn pasted_linkedin_html_is_extracted_via_the_adapter() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = for_test(dir.path().to_path_buf()).await.unwrap();
        pipe.ingest_paste(
            LINKEDIN_HTML,
            Some("https://www.linkedin.com/jobs/view/4294967296"),
        )
        .await
        .unwrap();
        pipe.drain().await.unwrap();
        let page = jobseeker_db::repo::job::list(&pipe.db, &Default::default())
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].title, "Staff Platform Engineer");
        assert_eq!(page.items[0].company_name, "Acme LinkedIn");
        assert!(
            page.items[0].salary_is_estimate,
            "LinkedIn compensation is an estimate"
        );

        let id: jobseeker_core::ids::JobId = page.items[0].id.parse().unwrap();
        let detail = jobseeker_db::repo::job::get(&pipe.db, &id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            detail
                .provenance
                .iter()
                .any(|p| p.field == "title" && p.provenance == "adapter"),
            "title must be tagged adapter, got {:?}",
            detail.provenance
        );
        assert!(
            detail
                .requirements
                .iter()
                .any(|r| r.text.contains("Kubernetes")),
            "got {:?}",
            detail.requirements
        );
    }

    #[tokio::test]
    async fn paste_without_a_url_still_creates_a_job() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = for_test(dir.path().to_path_buf()).await.unwrap();
        pipe.ingest_paste(GREENHOUSE_HTML, None).await.unwrap();
        pipe.drain().await.unwrap();
        let page = jobseeker_db::repo::job::list(&pipe.db, &Default::default())
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
    }

    #[tokio::test]
    async fn reconcile_check_is_clean_after_the_spine() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = for_test(dir.path().to_path_buf()).await.unwrap();
        pipe.ingest_paste(GREENHOUSE_HTML, None).await.unwrap();
        pipe.drain().await.unwrap();
        let report = pipe.reconcile_check().await.unwrap();
        assert!(
            report.is_clean(),
            "fresh materialize must not drift: db_only={:?} file_only={:?} divergent={:?}",
            report.db_only,
            report.file_only,
            report.divergent
        );
        assert_eq!(report.db_jobs, 1);
        assert_eq!(report.file_jobs, 1);
    }

    #[tokio::test]
    async fn reconcile_from_files_rebuilds_after_the_db_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_path_buf();
        let pipe = for_test(data.clone()).await.unwrap();
        pipe.ingest_paste(GREENHOUSE_HTML, None).await.unwrap();
        pipe.drain().await.unwrap();
        let before = jobseeker_db::repo::job::entity_counts(&pipe.db)
            .await
            .unwrap();
        let before_hash = jobseeker_db::repo::job::list_reconcile_rows(&pipe.db)
            .await
            .unwrap()[0]
            .content_hash
            .clone();
        pipe.db.close().await;
        drop(pipe);

        for name in ["jobseeker.db", "jobseeker.db-wal", "jobseeker.db-shm"] {
            let _ = std::fs::remove_file(data.join(name));
        }
        assert!(
            discover::discover_jobs(&data).unwrap().len() == 1,
            "files must survive deleting the database"
        );

        let pipe = for_test(data).await.unwrap();
        let report = pipe.reconcile_from_files().await.unwrap();
        assert!(
            report.is_clean(),
            "rebuild must match files: db_only={:?} file_only={:?} divergent={:?}",
            report.db_only,
            report.file_only,
            report.divergent
        );
        assert_eq!(report.restored, 1);
        let after = jobseeker_db::repo::job::entity_counts(&pipe.db)
            .await
            .unwrap();
        assert_eq!(after.jobs, before.jobs);
        assert_eq!(after.requirements, before.requirements);
        assert!(after.companies >= 1);
        let after_hash = jobseeker_db::repo::job::list_reconcile_rows(&pipe.db)
            .await
            .unwrap()[0]
            .content_hash
            .clone();
        assert_eq!(after_hash, before_hash);
    }

    #[tokio::test]
    async fn reconcile_check_reports_a_divergent_hash() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = for_test(dir.path().to_path_buf()).await.unwrap();
        pipe.ingest_paste(GREENHOUSE_HTML, None).await.unwrap();
        pipe.drain().await.unwrap();
        let files = discover::discover_jobs(dir.path()).unwrap();
        let json_path = files[0].abs_dir.join("job.json");
        let mut job: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
        job["content_hash"] = serde_json::json!("b3:drifted");
        std::fs::write(&json_path, serde_json::to_string_pretty(&job).unwrap()).unwrap();
        let report = pipe.reconcile_check().await.unwrap();
        assert_eq!(report.divergent.len(), 1, "{report:?}");
        assert!(!report.is_clean());
    }

    #[tokio::test]
    async fn reconcile_from_files_skips_tombstoned_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = for_test(dir.path().to_path_buf()).await.unwrap();
        pipe.ingest_paste(GREENHOUSE_HTML, None).await.unwrap();
        pipe.drain().await.unwrap();
        let id = jobseeker_db::repo::job::list_reconcile_rows(&pipe.db)
            .await
            .unwrap()[0]
            .id
            .clone();
        persist::record_tombstone(&pipe.db, "job", &id)
            .await
            .unwrap();

        let files = discover::discover_jobs(dir.path()).unwrap();
        let json_path = files[0].abs_dir.join("job.json");
        let mut job: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
        job["content_hash"] = serde_json::json!("b3:should-not-apply");
        std::fs::write(&json_path, serde_json::to_string_pretty(&job).unwrap()).unwrap();

        let report = pipe.reconcile_from_files().await.unwrap();
        assert_eq!(report.skipped_tombstone, 1);
        assert_eq!(report.restored, 0);
        assert_eq!(
            report.divergent.len(),
            1,
            "tombstoned files must not overwrite the database"
        );
    }

    const EVIDENCE_BANK: &str = include_str!("../../../fixtures/evidence-bank.md");
    const DS_HTML: &str = include_str!("../../../fixtures/applied-data-scientist.html");

    #[tokio::test]
    async fn importing_an_evidence_bank_replaces_placeholder_skills_and_rescores() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = for_test(dir.path().to_path_buf()).await.unwrap();
        pipe.ingest_paste(
            GREENHOUSE_HTML,
            Some("https://boards.greenhouse.io/acmerobotics/jobs/5512034"),
        )
        .await
        .unwrap();
        pipe.ingest_paste(
            DS_HTML,
            Some("https://boards.greenhouse.io/harbormedia/jobs/88001"),
        )
        .await
        .unwrap();
        pipe.drain().await.unwrap();

        let before = jobseeker_db::repo::job::list(&pipe.db, &Default::default())
            .await
            .unwrap();
        let pe_before = before
            .items
            .iter()
            .find(|j| j.title.contains("Platform"))
            .and_then(|j| j.match_overall)
            .expect("platform engineer should already be scored");

        let report = pipe.import_resume(EVIDENCE_BANK).await.unwrap();
        assert!(report.accomplishments >= 5, "{report:?}");
        assert!(report.skills >= 4, "{report:?}");
        pipe.drain().await.unwrap();

        let view =
            jobseeker_db::repo::experience::get_view(&pipe.db, &report.profile_id.parse().unwrap())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(view.full_name.as_deref(), Some("Alex Rivera"));
        assert!(view.skills.iter().any(|s| s.slug == "python"));
        assert!(view.skills.iter().any(|s| s.slug == "dataiku"));
        assert!(
            !view.skills.iter().any(|s| s.slug == "kubernetes"),
            "placeholder k8s must not survive import"
        );
        assert!(dir.path().join("profiles/default/profile.json").is_file());

        let after = jobseeker_db::repo::job::list(&pipe.db, &Default::default())
            .await
            .unwrap();
        let pe = after
            .items
            .iter()
            .find(|j| j.title.contains("Platform"))
            .unwrap();
        let ds = after
            .items
            .iter()
            .find(|j| j.title.contains("Data Scientist"))
            .unwrap();
        assert!(
            pe.match_overall.unwrap() < pe_before,
            "a DS bank should score a Rust/K8s platform role lower than the placeholder profile: before={pe_before} after={:?}",
            pe.match_overall
        );
        assert!(
            ds.match_overall.unwrap() > pe.match_overall.unwrap(),
            "Applied Data Scientist should outrank Senior Platform Engineer, got ds={:?} pe={:?}",
            ds.match_overall,
            pe.match_overall
        );
    }
}
