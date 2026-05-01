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
    /// Hook ingest attempts that failed mid-loop (read error, writer error,
    /// transient IO). The recorder logs and continues rather than dying, so
    /// this counter surfaces how often that path fired. Distinct from
    /// `discarded_late_events`, which counts envelopes dropped *after* the
    /// stop signal.
    #[serde(default)]
    pub ingest_errors: u64,
    /// Number of `Event` records written to the log so far. Cached in meta so
    /// `redo list` can show frame counts without decompressing the log.
    /// Updated on every `maybe_update_meta` tick and on finalize.
    #[serde(default)]
    pub frame_count: u64,
    /// Count of hook payloads that failed schema validation against the
    /// pinned shape in `crate::hook::schema`. Surfaces silent fidelity loss
    /// from a Claude Code hook payload change as a loud counter on the
    /// session.
    #[serde(default)]
    pub schema_drift_events: u64,
    /// ISO 8601 timestamp at session creation.
    pub created_at: String,
    /// Session id this one was forked from, if any. `None` for organic
    /// recordings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<Uuid>,
    /// Frame index in the parent at which this session was forked. The first
    /// frame in this session corresponds to seq 0 of the parent's `[0..=N]`
    /// prefix. `None` for organic recordings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_at_frame: Option<u64>,
}
