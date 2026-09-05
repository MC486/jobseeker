//! Background work.
//!
//! Tasks live in the same SQLite database as the data they operate on, so enqueueing is
//! transactional with the write that caused it (ADR-0006). Handlers must be idempotent:
//! delivery is at-least-once.

use serde::{Deserialize, Serialize};

use crate::ids::TaskId;
use crate::time::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// Fetch a public URL, persist the raw bytes, then extract.
    IngestUrl,
    /// Process a capture the extension already delivered.
    IngestCapture,
    /// Run the extraction pipeline over a stored capture.
    ExtractJob,
    /// Conditional re-fetch to detect edits and closure.
    RefreshListing,
    /// Decide whether this job is a cross-post of an existing one.
    DedupeJob,
    /// Write the job's files (`job.md`, `job.json`, ...).
    MaterializeJob,
    Embed,
    ScoreMatch,
    AggregateGaps,
    RenderDocument,
    ImportResume,
    ReconcileFiles,
    /// Poll a named *public* ATS board. Never an authenticated site.
    PollBoard,
    /// Housekeeping: expire leases, prune caches, refresh derived counters.
    Maintenance,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::IngestUrl => "ingest_url",
            TaskKind::IngestCapture => "ingest_capture",
            TaskKind::ExtractJob => "extract_job",
            TaskKind::RefreshListing => "refresh_listing",
            TaskKind::DedupeJob => "dedupe_job",
            TaskKind::MaterializeJob => "materialize_job",
            TaskKind::Embed => "embed",
            TaskKind::ScoreMatch => "score_match",
            TaskKind::AggregateGaps => "aggregate_gaps",
            TaskKind::RenderDocument => "render_document",
            TaskKind::ImportResume => "import_resume",
            TaskKind::ReconcileFiles => "reconcile_files",
            TaskKind::PollBoard => "poll_board",
            TaskKind::Maintenance => "maintenance",
        }
    }

    pub const ALL: &'static [TaskKind] = &[
        TaskKind::IngestUrl,
        TaskKind::IngestCapture,
        TaskKind::ExtractJob,
        TaskKind::RefreshListing,
        TaskKind::DedupeJob,
        TaskKind::MaterializeJob,
        TaskKind::Embed,
        TaskKind::ScoreMatch,
        TaskKind::AggregateGaps,
        TaskKind::RenderDocument,
        TaskKind::ImportResume,
        TaskKind::ReconcileFiles,
        TaskKind::PollBoard,
        TaskKind::Maintenance,
    ];

    /// Higher runs first. Work the user is waiting on outranks background enrichment.
    pub fn default_priority(self) -> i64 {
        match self {
            TaskKind::IngestCapture | TaskKind::IngestUrl => 100,
            TaskKind::ExtractJob => 90,
            TaskKind::DedupeJob | TaskKind::MaterializeJob => 80,
            TaskKind::RenderDocument | TaskKind::ImportResume => 70,
            TaskKind::ScoreMatch => 50,
            TaskKind::Embed => 40,
            TaskKind::RefreshListing => 30,
            TaskKind::AggregateGaps | TaskKind::PollBoard => 20,
            TaskKind::ReconcileFiles => 10,
            TaskKind::Maintenance => 0,
        }
    }

    /// Whether a task of this kind reaches out to the network, and so needs the rate limiter
    /// and a longer lease.
    pub fn touches_network(self) -> bool {
        matches!(
            self,
            TaskKind::IngestUrl | TaskKind::RefreshListing | TaskKind::PollBoard
        )
    }
}

impl std::str::FromStr for TaskKind {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        TaskKind::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| crate::Error::BadRequest(format!("unknown task kind: {s}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Done,
    /// Terminal after `max_attempts`. The error is retained for diagnosis.
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Queued => "queued",
            TaskStatus::Running => "running",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled
        )
    }
}

impl std::str::FromStr for TaskStatus {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        Ok(match s {
            "queued" => TaskStatus::Queued,
            "running" => TaskStatus::Running,
            "done" => TaskStatus::Done,
            "failed" => TaskStatus::Failed,
            "cancelled" => TaskStatus::Cancelled,
            other => {
                return Err(crate::Error::BadRequest(format!(
                    "unknown task status: {other}"
                )))
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub kind: TaskKind,
    pub payload: serde_json::Value,
    pub status: TaskStatus,
    pub priority: i64,
    pub attempts: u32,
    pub max_attempts: u32,
    pub last_error: Option<String>,
    pub progress: Option<f32>,
    pub progress_message: Option<String>,
    /// Coalescing key: enqueueing "extract job X" five times must produce one row.
    pub dedupe_key: Option<String>,
    pub available_at: Timestamp,
    pub lease_expires_at: Option<Timestamp>,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub parent_task_id: Option<TaskId>,
    pub trace_id: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Task {
    pub fn has_attempts_left(&self) -> bool {
        self.attempts < self.max_attempts
    }
}

/// Exponential backoff with deterministic jitter derived from the attempt number and task id,
/// so a thundering herd of retries spreads out without needing an RNG in the hot path.
pub fn retry_delay_seconds(attempt: u32, id: &TaskId) -> u64 {
    let base = 5u64.saturating_mul(2u64.saturating_pow(attempt.min(8)));
    let capped = base.min(3600);
    // Up to 25% jitter, stable per (task, attempt).
    let seed = crate::hash::hash_parts(&[id.as_str(), &attempt.to_string()]);
    let nibble = u64::from_str_radix(&seed[3..5], 16).unwrap_or(0);
    let jitter = capped * (nibble % 26) / 100;
    capped + jitter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_facing_work_outranks_background_work() {
        assert!(TaskKind::IngestCapture.default_priority() > TaskKind::Embed.default_priority());
        assert!(TaskKind::ExtractJob.default_priority() > TaskKind::ScoreMatch.default_priority());
        assert!(
            TaskKind::Maintenance.default_priority() < TaskKind::RefreshListing.default_priority()
        );
    }

    #[test]
    fn all_kinds_round_trip() {
        for k in TaskKind::ALL {
            assert_eq!(k.as_str().parse::<TaskKind>().unwrap(), *k);
        }
        assert!("invent_jobs".parse::<TaskKind>().is_err());
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let id = TaskId::new();
        let d0 = retry_delay_seconds(0, &id);
        let d3 = retry_delay_seconds(3, &id);
        let d20 = retry_delay_seconds(20, &id);
        assert!(d0 >= 5 && d0 <= 7, "first retry is prompt, got {d0}");
        assert!(d3 > d0);
        assert!(d20 <= 4500, "capped at an hour plus jitter, got {d20}");
    }

    #[test]
    fn backoff_is_deterministic_per_task_and_attempt() {
        let id = TaskId::new();
        assert_eq!(retry_delay_seconds(2, &id), retry_delay_seconds(2, &id));
    }

    #[test]
    fn terminal_statuses_are_identified() {
        assert!(TaskStatus::Done.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(!TaskStatus::Queued.is_terminal());
    }
}
