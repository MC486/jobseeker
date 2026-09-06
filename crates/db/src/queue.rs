//! The task queue (ADR-0006).
//!
//! Claiming is a single atomic statement so several workers — or several processes — cannot
//! take the same task. Leases make crash recovery automatic. Delivery is at-least-once, so
//! every handler must be idempotent.

use jobseeker_core::domain::task::{retry_delay_seconds, TaskKind, TaskStatus};
use jobseeker_core::ids::TaskId;
use jobseeker_core::time::{now, to_rfc3339, Timestamp};
use jobseeker_core::{Error, Result};
use sqlx::Row;

use crate::{db_err, Db};

/// A task as the worker sees it.
#[derive(Debug, Clone)]
pub struct ClaimedTask {
    pub id: TaskId,
    pub kind: TaskKind,
    pub payload: serde_json::Value,
    pub attempts: u32,
    pub max_attempts: u32,
    pub trace_id: Option<String>,
    pub lease_expires_at: Timestamp,
}

/// What to enqueue.
#[derive(Debug, Clone)]
pub struct NewTask {
    pub kind: TaskKind,
    pub payload: serde_json::Value,
    /// `None` uses the kind's default priority.
    pub priority: Option<i64>,
    /// Coalescing key. A second enqueue with the same key is a no-op while the first is
    /// still pending, which is what keeps a profile edit from queueing 5,000 duplicate
    /// rescores.
    pub dedupe_key: Option<String>,
    pub available_at: Option<Timestamp>,
    pub max_attempts: Option<u32>,
    pub parent_task_id: Option<TaskId>,
    pub trace_id: Option<String>,
}

impl NewTask {
    pub fn new(kind: TaskKind, payload: serde_json::Value) -> Self {
        Self {
            kind,
            payload,
            priority: None,
            dedupe_key: None,
            available_at: None,
            max_attempts: None,
            parent_task_id: None,
            trace_id: None,
        }
    }

    pub fn dedupe(mut self, key: impl Into<String>) -> Self {
        self.dedupe_key = Some(key.into());
        self
    }

    pub fn priority(mut self, priority: i64) -> Self {
        self.priority = Some(priority);
        self
    }

    pub fn delay(mut self, seconds: i64) -> Self {
        self.available_at = Some(now() + chrono::Duration::seconds(seconds));
        self
    }

    pub fn trace(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }
}

/// Public view of a task row.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskView {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub priority: i64,
    pub attempts: u32,
    pub max_attempts: u32,
    pub last_error: Option<String>,
    pub progress: Option<f64>,
    pub progress_message: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

pub struct Queue<'a> {
    db: &'a Db,
    lease_seconds: i64,
    default_max_attempts: u32,
}

impl<'a> Queue<'a> {
    pub fn new(db: &'a Db, lease_seconds: u64, default_max_attempts: u32) -> Self {
        Self {
            db,
            lease_seconds: lease_seconds as i64,
            default_max_attempts,
        }
    }

    /// Enqueue, returning the id of the new task — or of the existing one when the dedupe
    /// key collides with something still pending.
    pub async fn enqueue(&self, task: NewTask) -> Result<TaskId> {
        let id = TaskId::new();
        let ts = to_rfc3339(&now());
        let available_at = to_rfc3339(&task.available_at.unwrap_or_else(now));
        let priority = task
            .priority
            .unwrap_or_else(|| task.kind.default_priority());
        let max_attempts = task.max_attempts.unwrap_or(self.default_max_attempts);
        let payload = serde_json::to_string(&task.payload)?;

        // `DO NOTHING` rather than `DO UPDATE`: a pending task will pick up the current state
        // when it runs, so re-enqueueing the same work is genuinely a no-op.
        let row = sqlx::query(
            "INSERT INTO task (id, kind, payload_json, status, priority, attempts, max_attempts,
                               dedupe_key, available_at, parent_task_id, trace_id,
                               created_at, updated_at)
             VALUES (?1, ?2, ?3, 'queued', ?4, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
             ON CONFLICT (dedupe_key) DO NOTHING
             RETURNING id",
        )
        .bind(id.as_str())
        .bind(task.kind.as_str())
        .bind(&payload)
        .bind(priority)
        .bind(max_attempts as i64)
        .bind(task.dedupe_key.as_deref())
        .bind(&available_at)
        .bind(task.parent_task_id.as_ref().map(|t| t.as_str()))
        .bind(task.trace_id.as_deref())
        .bind(&ts)
        .fetch_optional(self.db.writer())
        .await
        .map_err(db_err)?;

        match row {
            Some(row) => Ok(row
                .try_get::<String, _>("id")
                .map_err(db_err)?
                .parse()
                .unwrap_or(id)),
            None => {
                // Coalesced into an existing pending task.
                let key = task.dedupe_key.unwrap_or_default();
                let existing: String =
                    sqlx::query_scalar("SELECT id FROM task WHERE dedupe_key = ?1")
                        .bind(&key)
                        .fetch_one(self.db.reader())
                        .await
                        .map_err(db_err)?;
                existing.parse()
            }
        }
    }

    /// Atomically claim the highest-priority available task.
    ///
    /// The whole claim is one statement, so two workers cannot select the same row: SQLite
    /// serializes writers, and the subquery is evaluated inside the same write transaction.
    pub async fn claim(&self) -> Result<Option<ClaimedTask>> {
        let now_ts = now();
        let lease_until = now_ts + chrono::Duration::seconds(self.lease_seconds);

        let row = sqlx::query(
            "UPDATE task
                SET status = 'running',
                    lease_expires_at = ?1,
                    started_at = COALESCE(started_at, ?2),
                    attempts = attempts + 1,
                    updated_at = ?2
              WHERE id = (
                    SELECT id FROM task
                     WHERE status = 'queued' AND available_at <= ?2
                     ORDER BY priority DESC, available_at ASC, created_at ASC
                     LIMIT 1
              )
              RETURNING id, kind, payload_json, attempts, max_attempts, trace_id",
        )
        .bind(to_rfc3339(&lease_until))
        .bind(to_rfc3339(&now_ts))
        .fetch_optional(self.db.writer())
        .await
        .map_err(db_err)?;

        let Some(row) = row else { return Ok(None) };
        let payload: String = row.try_get("payload_json").map_err(db_err)?;
        let kind: String = row.try_get("kind").map_err(db_err)?;
        let id: String = row.try_get("id").map_err(db_err)?;
        Ok(Some(ClaimedTask {
            id: id.parse()?,
            kind: kind.parse()?,
            payload: serde_json::from_str(&payload)?,
            attempts: row.try_get::<i64, _>("attempts").map_err(db_err)? as u32,
            max_attempts: row.try_get::<i64, _>("max_attempts").map_err(db_err)? as u32,
            trace_id: row.try_get("trace_id").map_err(db_err)?,
            lease_expires_at: lease_until,
        }))
    }

    /// Extend the lease of a long-running task. Handlers that can exceed the lease must call
    /// this periodically or risk being executed twice.
    pub async fn renew_lease(&self, id: &TaskId) -> Result<()> {
        let lease_until = now() + chrono::Duration::seconds(self.lease_seconds);
        sqlx::query(
            "UPDATE task SET lease_expires_at = ?1, updated_at = ?1
              WHERE id = ?2 AND status = 'running'",
        )
        .bind(to_rfc3339(&lease_until))
        .bind(id.as_str())
        .execute(self.db.writer())
        .await
        .map(|_| ())
        .map_err(db_err)
    }

    pub async fn report_progress(&self, id: &TaskId, fraction: f32, message: &str) -> Result<()> {
        sqlx::query(
            "UPDATE task SET progress = ?1, progress_message = ?2, updated_at = ?3 WHERE id = ?4",
        )
        .bind(fraction.clamp(0.0, 1.0))
        .bind(message)
        .bind(to_rfc3339(&now()))
        .bind(id.as_str())
        .execute(self.db.writer())
        .await
        .map(|_| ())
        .map_err(db_err)
    }

    pub async fn complete(&self, id: &TaskId) -> Result<()> {
        let ts = to_rfc3339(&now());
        // Drop the dedupe key once the work is done so a later extract can
        // enqueue `score:{job}` / `materialize:{job}` again. UNIQUE(dedupe_key)
        // treats NULL as distinct, so completed history stays queryable.
        sqlx::query(
            "UPDATE task
                SET status = 'done', finished_at = ?1, lease_expires_at = NULL,
                    progress = 1.0, updated_at = ?1, dedupe_key = NULL
              WHERE id = ?2",
        )
        .bind(&ts)
        .bind(id.as_str())
        .execute(self.db.writer())
        .await
        .map(|_| ())
        .map_err(db_err)
    }

    /// Record a failure: either schedule a retry with backoff, or move to the terminal
    /// `failed` state retaining the error for diagnosis.
    pub async fn fail(&self, task: &ClaimedTask, error: &Error) -> Result<TaskStatus> {
        let retryable = error.is_retryable() && task.attempts < task.max_attempts;
        let ts = to_rfc3339(&now());

        if retryable {
            let delay = retry_delay_seconds(task.attempts, &task.id) as i64;
            let available_at = now() + chrono::Duration::seconds(delay);
            sqlx::query(
                "UPDATE task
                    SET status = 'queued', last_error = ?1, available_at = ?2,
                        lease_expires_at = NULL, updated_at = ?3
                  WHERE id = ?4",
            )
            .bind(error.to_string())
            .bind(to_rfc3339(&available_at))
            .bind(&ts)
            .bind(task.id.as_str())
            .execute(self.db.writer())
            .await
            .map_err(db_err)?;
            Ok(TaskStatus::Queued)
        } else {
            sqlx::query(
                "UPDATE task
                    SET status = 'failed', last_error = ?1, finished_at = ?2,
                        lease_expires_at = NULL, updated_at = ?2
                  WHERE id = ?3",
            )
            .bind(error.to_string())
            .bind(&ts)
            .bind(task.id.as_str())
            .execute(self.db.writer())
            .await
            .map_err(db_err)?;
            Ok(TaskStatus::Failed)
        }
    }

    /// Return tasks whose lease expired to the queue. This is the whole of crash recovery:
    /// a `kill -9` mid-handler leaves a row that the scheduler reclaims.
    pub async fn reclaim_expired_leases(&self) -> Result<u64> {
        let ts = to_rfc3339(&now());
        let result = sqlx::query(
            "UPDATE task
                SET status = 'queued', lease_expires_at = NULL, updated_at = ?1,
                    last_error = COALESCE(last_error, 'lease expired; worker presumed dead')
              WHERE status = 'running' AND lease_expires_at IS NOT NULL
                AND lease_expires_at < ?1",
        )
        .bind(&ts)
        .execute(self.db.writer())
        .await
        .map_err(db_err)?;
        Ok(result.rows_affected())
    }

    pub async fn cancel(&self, id: &TaskId) -> Result<()> {
        let ts = to_rfc3339(&now());
        sqlx::query(
            "UPDATE task SET status = 'cancelled', finished_at = ?1, lease_expires_at = NULL,
                             updated_at = ?1
              WHERE id = ?2 AND status IN ('queued', 'running')",
        )
        .bind(&ts)
        .bind(id.as_str())
        .execute(self.db.writer())
        .await
        .map(|_| ())
        .map_err(db_err)
    }

    /// Re-queue a failed task, clearing its attempt count.
    pub async fn retry(&self, id: &TaskId) -> Result<()> {
        let ts = to_rfc3339(&now());
        sqlx::query(
            "UPDATE task
                SET status = 'queued', attempts = 0, last_error = NULL, available_at = ?1,
                    finished_at = NULL, updated_at = ?1
              WHERE id = ?2 AND status IN ('failed', 'cancelled')",
        )
        .bind(&ts)
        .bind(id.as_str())
        .execute(self.db.writer())
        .await
        .map(|_| ())
        .map_err(db_err)
    }

    /// A task as the API and CLI report it.
    pub async fn get(&self, id: &TaskId) -> Result<Option<TaskView>> {
        let row = sqlx::query(
            "SELECT id, kind, payload_json, status, priority, attempts, max_attempts,
                    last_error, progress, progress_message, created_at, updated_at, finished_at
               FROM task WHERE id = ?1",
        )
        .bind(id.as_str())
        .fetch_optional(self.db.writer())
        .await
        .map_err(db_err)?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(TaskView {
            id: row.try_get::<String, _>("id").map_err(db_err)?,
            kind: row.try_get("kind").map_err(db_err)?,
            status: row.try_get("status").map_err(db_err)?,
            priority: row.try_get("priority").map_err(db_err)?,
            attempts: row.try_get::<i64, _>("attempts").map_err(db_err)? as u32,
            max_attempts: row.try_get::<i64, _>("max_attempts").map_err(db_err)? as u32,
            last_error: row.try_get("last_error").map_err(db_err)?,
            progress: row.try_get("progress").map_err(db_err)?,
            progress_message: row.try_get("progress_message").map_err(db_err)?,
            payload: serde_json::from_str(
                &row.try_get::<String, _>("payload_json").map_err(db_err)?,
            )?,
            created_at: row.try_get("created_at").map_err(db_err)?,
            updated_at: row.try_get("updated_at").map_err(db_err)?,
            finished_at: row.try_get("finished_at").map_err(db_err)?,
        }))
    }

    /// Queue depth per status, for `/metrics` and `/readyz`.
    pub async fn depth(&self) -> Result<Vec<(String, i64)>> {
        sqlx::query_as("SELECT status, COUNT(*) FROM task GROUP BY status")
            .fetch_all(self.db.reader())
            .await
            .map_err(db_err)
    }

    /// Drop old terminal tasks. Failures are kept longer than successes because the error is
    /// the useful part.
    pub async fn prune(
        &self,
        done_older_than_days: i64,
        failed_older_than_days: i64,
    ) -> Result<u64> {
        let done_cutoff = to_rfc3339(&(now() - chrono::Duration::days(done_older_than_days)));
        let failed_cutoff = to_rfc3339(&(now() - chrono::Duration::days(failed_older_than_days)));
        let result = sqlx::query(
            "DELETE FROM task
              WHERE (status = 'done' AND finished_at < ?1)
                 OR (status IN ('failed','cancelled') AND finished_at < ?2)",
        )
        .bind(&done_cutoff)
        .bind(&failed_cutoff)
        .execute(self.db.writer())
        .await
        .map_err(db_err)?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn queue_fixture() -> Db {
        Db::open_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn enqueue_then_claim_round_trips_the_payload() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);

        let id = q
            .enqueue(NewTask::new(
                TaskKind::ExtractJob,
                json!({"capture_id": "abc"}),
            ))
            .await
            .unwrap();

        let claimed = q
            .claim()
            .await
            .unwrap()
            .expect("a task should be available");
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.kind, TaskKind::ExtractJob);
        assert_eq!(claimed.payload["capture_id"], "abc");
        assert_eq!(claimed.attempts, 1);

        // Claiming again finds nothing: the first claim took it.
        assert!(q.claim().await.unwrap().is_none());

        q.complete(&claimed.id).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM task WHERE id = ?1")
            .bind(claimed.id.as_str())
            .fetch_one(db.reader())
            .await
            .unwrap();
        assert_eq!(status, "done");
    }

    #[tokio::test]
    async fn dedupe_key_coalesces_repeat_enqueues() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);

        let first = q
            .enqueue(NewTask::new(TaskKind::ScoreMatch, json!({"job": 1})).dedupe("score:job1"))
            .await
            .unwrap();
        let second = q
            .enqueue(NewTask::new(TaskKind::ScoreMatch, json!({"job": 1})).dedupe("score:job1"))
            .await
            .unwrap();
        assert_eq!(first, second, "the same key must not create a second task");

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task")
            .fetch_one(db.reader())
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn a_completed_dedupe_key_can_be_enqueued_again() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);
        let first = q
            .enqueue(NewTask::new(TaskKind::ScoreMatch, json!({"job": 1})).dedupe("score:job1"))
            .await
            .unwrap();
        let claimed = q.claim().await.unwrap().unwrap();
        q.complete(&claimed.id).await.unwrap();
        let second = q
            .enqueue(NewTask::new(TaskKind::ScoreMatch, json!({"job": 1})).dedupe("score:job1"))
            .await
            .unwrap();
        assert_ne!(
            first, second,
            "a finished score must not block the next extract"
        );
    }

    #[tokio::test]
    async fn higher_priority_is_claimed_first() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);

        q.enqueue(NewTask::new(TaskKind::Embed, json!({})).priority(10))
            .await
            .unwrap();
        q.enqueue(NewTask::new(TaskKind::IngestCapture, json!({})).priority(100))
            .await
            .unwrap();

        let first = q.claim().await.unwrap().unwrap();
        assert_eq!(
            first.kind,
            TaskKind::IngestCapture,
            "work the user is waiting on must come first"
        );
    }

    #[tokio::test]
    async fn delayed_tasks_are_not_claimable_yet() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);
        q.enqueue(NewTask::new(TaskKind::RefreshListing, json!({})).delay(3600))
            .await
            .unwrap();
        assert!(q.claim().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn retryable_failures_are_requeued_with_backoff() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 3);

        q.enqueue(NewTask::new(TaskKind::IngestUrl, json!({})))
            .await
            .unwrap();
        let claimed = q.claim().await.unwrap().unwrap();

        let status = q.fail(&claimed, &Error::FetchTimeout(20)).await.unwrap();
        assert_eq!(status, TaskStatus::Queued);

        // Backoff means it is not immediately claimable again.
        assert!(q.claim().await.unwrap().is_none());
        let (available_at, err): (String, Option<String>) =
            sqlx::query_as("SELECT available_at, last_error FROM task WHERE id = ?1")
                .bind(claimed.id.as_str())
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert!(available_at > to_rfc3339(&now()));
        assert!(err.unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn permanent_failures_go_straight_to_failed() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);
        q.enqueue(NewTask::new(TaskKind::IngestUrl, json!({})))
            .await
            .unwrap();
        let claimed = q.claim().await.unwrap().unwrap();

        // A robots.txt refusal will never succeed on retry.
        let status = q
            .fail(&claimed, &Error::RobotsDisallowed("example.com".into()))
            .await
            .unwrap();
        assert_eq!(status, TaskStatus::Failed);
    }

    #[tokio::test]
    async fn attempts_are_exhausted_then_the_task_fails() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 2);
        q.enqueue(NewTask::new(TaskKind::IngestUrl, json!({})))
            .await
            .unwrap();

        // First attempt: retryable.
        let c1 = q.claim().await.unwrap().unwrap();
        assert_eq!(
            q.fail(&c1, &Error::FetchTimeout(1)).await.unwrap(),
            TaskStatus::Queued
        );

        // Make it available again, then exhaust the budget.
        sqlx::query("UPDATE task SET available_at = '2000-01-01T00:00:00Z'")
            .execute(db.writer())
            .await
            .unwrap();
        let c2 = q.claim().await.unwrap().unwrap();
        assert_eq!(c2.attempts, 2);
        assert_eq!(
            q.fail(&c2, &Error::FetchTimeout(1)).await.unwrap(),
            TaskStatus::Failed
        );
    }

    #[tokio::test]
    async fn expired_leases_are_reclaimed() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);
        q.enqueue(NewTask::new(TaskKind::ExtractJob, json!({})))
            .await
            .unwrap();
        let claimed = q.claim().await.unwrap().unwrap();

        // Simulate a worker that died mid-handler.
        sqlx::query("UPDATE task SET lease_expires_at = '2000-01-01T00:00:00Z' WHERE id = ?1")
            .bind(claimed.id.as_str())
            .execute(db.writer())
            .await
            .unwrap();

        assert_eq!(q.reclaim_expired_leases().await.unwrap(), 1);
        let reclaimed = q.claim().await.unwrap().expect("task returns to the queue");
        assert_eq!(reclaimed.id, claimed.id);
        assert_eq!(reclaimed.attempts, 2, "the retry counts as an attempt");
    }

    #[tokio::test]
    async fn a_live_lease_is_not_reclaimed() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);
        q.enqueue(NewTask::new(TaskKind::ExtractJob, json!({})))
            .await
            .unwrap();
        q.claim().await.unwrap().unwrap();
        assert_eq!(q.reclaim_expired_leases().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn renewing_a_lease_pushes_the_deadline_out() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);
        q.enqueue(NewTask::new(TaskKind::RenderDocument, json!({})))
            .await
            .unwrap();
        let claimed = q.claim().await.unwrap().unwrap();

        sqlx::query("UPDATE task SET lease_expires_at = '2000-01-01T00:00:00Z' WHERE id = ?1")
            .bind(claimed.id.as_str())
            .execute(db.writer())
            .await
            .unwrap();
        q.renew_lease(&claimed.id).await.unwrap();
        assert_eq!(
            q.reclaim_expired_leases().await.unwrap(),
            0,
            "a renewed lease must survive reclamation"
        );
    }

    #[tokio::test]
    async fn cancel_and_retry_move_between_states() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);
        let id = q
            .enqueue(NewTask::new(TaskKind::Embed, json!({})))
            .await
            .unwrap();

        q.cancel(&id).await.unwrap();
        assert!(q.claim().await.unwrap().is_none());

        q.retry(&id).await.unwrap();
        let claimed = q.claim().await.unwrap().expect("retry re-queues the task");
        assert_eq!(claimed.attempts, 1, "retry resets the attempt counter");
    }

    #[tokio::test]
    async fn depth_reports_per_status_counts() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);
        q.enqueue(NewTask::new(TaskKind::Embed, json!({})).dedupe("a"))
            .await
            .unwrap();
        q.enqueue(NewTask::new(TaskKind::Embed, json!({})).dedupe("b"))
            .await
            .unwrap();
        q.claim().await.unwrap().unwrap();

        let depth = q.depth().await.unwrap();
        let map: std::collections::HashMap<_, _> = depth.into_iter().collect();
        assert_eq!(map.get("queued"), Some(&1));
        assert_eq!(map.get("running"), Some(&1));
    }

    #[tokio::test]
    async fn progress_is_recorded_and_clamped() {
        let db = queue_fixture().await;
        let q = Queue::new(&db, 120, 5);
        let id = q
            .enqueue(NewTask::new(TaskKind::ImportResume, json!({})))
            .await
            .unwrap();
        q.report_progress(&id, 5.0, "parsing").await.unwrap();
        let (progress, message): (f64, String) =
            sqlx::query_as("SELECT progress, progress_message FROM task WHERE id = ?1")
                .bind(id.as_str())
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(progress, 1.0);
        assert_eq!(message, "parsing");
    }
}
