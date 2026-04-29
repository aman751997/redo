use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Lifecycle state of a session, tracked in `meta.json`.
///
/// Transitions: `Recording` → `Finalizing` (on child exit) → `Complete`.
/// `Crashed` is written best-effort on a panic / SIGKILL; readers also infer
/// it lazily by checking that the recording owner pid is still alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Recording,
    Finalizing,
    Complete,
    Crashed,
}

/// `meta.json` payload. Small enough to rewrite atomically on every change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub session_id: Uuid,
    pub state: SessionState,
    /// Recorder pid. Combined with `pid_starttime` it identifies the owning
    /// process unambiguously even after PID reuse.
    pub pid: u32,
    /// Process start-time ticks from `/proc/<pid>/stat` (Linux) or 0 elsewhere.
    /// Used together with `pid` for liveness checks during lazy crash detection.
    #[serde(default)]
    pub pid_starttime: u64,
    /// Hook events that arrived after the post-exit grace window closed and
    /// were therefore dropped. Surfaced in `redo ls`.
    #[serde(default)]
    pub discarded_late_events: u64,
    /// ISO 8601 timestamp at session creation.
    pub created_at: String,
}
