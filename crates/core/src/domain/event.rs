//! Durable domain events. The pipeline writes these to `event_log` and fans them
//! out over an in-process broadcast so the SSE endpoint can backfill and stream.

use serde::{Deserialize, Serialize};

/// A progress notification. Nothing depends on these for correctness — they are
/// how the UI learns that a job appeared, a score landed, or a task finished.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomainEvent {
    pub id: i64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
    pub created_at: String,
}

impl DomainEvent {
    pub const JOB_CREATED: &'static str = "job.created";
    pub const JOB_UPDATED: &'static str = "job.updated";
    pub const MATCH_UPDATED: &'static str = "match.updated";
    pub const TASK_UPDATED: &'static str = "task.updated";
}
