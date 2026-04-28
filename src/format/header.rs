use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// First record in every log file. Identifies the format and the session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionHeader {
    /// Format version. Currently 1.
    pub version: u32,

    /// Magic string. Currently "redo".
    pub format: String,

    /// UUIDv7 — time-ordered, globally unique.
    pub session_id: Uuid,

    /// ISO 8601 wall-clock timestamp at session start.
    pub created_at: String,

    /// The command line that was recorded (argv).
    pub cmd: Vec<String>,

    /// Terminal dimensions at session start.
    pub env_term: TermSize,

    /// Claude Code version, if detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_version: Option<String>,

    /// `redo` version that produced this log.
    pub redo_version: String,

    /// Working directory at session start.
    pub cwd: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}
