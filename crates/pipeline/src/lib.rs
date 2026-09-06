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
use jobseeker_db::repo::{capture, company, event, listing, listing::UpsertListing};
use jobseeker_db::Db;
use jobseeker_extract::{extract, ExtractInput};
use jobseeker_llm::LlmClient;
use jobseeker_normalize::canonicalize;
use jobseeker_store::jobfile::{self, JobDocument};
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
            TaskKind::MaterializeJob => self.handle_materialize(&task.payload).await,
            TaskKind::ScoreMatch => self.handle_score_match(&task.payload).await,
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
            url: row.url.clone(),
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

    async fn handle_materialize(&self, payload: &serde_json::Value) -> Result<()> {
        let p: JobPayload = serde_json::from_value(payload.clone())?;
        let job_id: jobseeker_core::ids::JobId = p.job_id.parse()?;
        let detail = jobseeker_db::repo::job::get(&self.db, &job_id)
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
            &job_id,
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

        let job_json = json!({
            "id": detail.id,
            "title": detail.title,
            "company": company.name,
            "status": detail.status,
            "work_mode": detail.work_mode,
            "seniority": detail.seniority,
            "employment_type": detail.employment_type,
            "salary_raw": detail.salary_raw,
            "apply_url": detail.apply_url,
            "posted_at": detail.posted_at,
            "locations": detail.locations,
            "extraction_partial": detail.extraction_partial,
            "content_hash": detail.content_hash,
        });
        atomic_write(&dir.join("job.json"), &to_stable_json(&job_json)?)?;
        atomic_write(
            &dir.join("requirements.json"),
            &to_stable_json(&detail.requirements)?,
        )?;

        let rel = relative_path(self.data_dir(), &dir);
        persist::set_file_path(&self.db, &job_id, &rel).await?;
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

    async fn handle_score_match(&self, payload: &serde_json::Value) -> Result<()> {
        let p: JobPayload = serde_json::from_value(payload.clone())?;
        let job_id: jobseeker_core::ids::JobId = p.job_id.parse()?;
        let Some((job, requirements, locations)) =
            jobseeker_db::repo::job::scoring_inputs(&self.db, &job_id).await?
        else {
            return Err(Error::NotFound("job"));
        };
        let Some(profile_row) = jobseeker_db::repo::profile::get(&self.db, None).await? else {
            tracing::warn!("score_match skipped: no default profile");
            return Ok(());
        };

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
            seniority: Some(jobseeker_core::domain::enums::Seniority::Senior),
            years_experience: Some(10.0),
            target_comp_min_cents: profile_row.target_comp_min_cents,
            accepts_remote: profile_row.accepts_remote,
            willing_to_relocate: profile_row.willing_to_relocate,
            target_locations: profile_row.target_locations,
            clearances: vec![],
            education: None,
        };
        let score = jobseeker_matching::score(&snapshot, &profile, &self.config.matching)?;
        jobseeker_db::repo::score::upsert(&self.db, &score).await?;
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
}
