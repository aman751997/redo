use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One log record. Internally tagged on `kind`.
///
/// Every variant carries `seq` (authoritative ordering, strictly increasing),
/// `t_ns` (wall-clock nanoseconds from `CLOCK_REALTIME`, clamped non-decreasing),
/// and `extras` — a catch-all that preserves unknown JSON fields verbatim
/// across roundtrips so a newer producer's records survive an older reader.
///
/// The variants here are a starting subset; the rest land in follow-up commits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum Event {
    /// PTY stdout/stderr from the recorded process.
    Output {
        seq: u64,
        t_ns: u64,
        /// Base64-encoded bytes (capped at `MAX_INLINE_PAYLOAD`).
        bytes: String,
        /// Set to `true` when the original payload exceeded the cap.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        truncated: Option<bool>,
        /// Original byte count, only present when truncated.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        truncated_original_size: Option<usize>,
        #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
        extras: Map<String, Value>,
    },

    /// PTY stdin written to the recorded process.
    Input {
        seq: u64,
        t_ns: u64,
        bytes: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        truncated: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        truncated_original_size: Option<usize>,
        #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
        extras: Map<String, Value>,
    },

    /// Terminal resize (SIGWINCH).
    Resize {
        seq: u64,
        t_ns: u64,
        cols: u16,
        rows: u16,
        #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
        extras: Map<String, Value>,
    },

    /// User-visible interruption: SIGINT, SIGTERM, force-kill, etc.
    Marker {
        seq: u64,
        t_ns: u64,
        label: String,
        #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
        extras: Map<String, Value>,
    },
}

impl Event {
    /// Sequence number (authoritative ordering within a session).
    pub fn seq(&self) -> u64 {
        match self {
            Event::Output { seq, .. }
            | Event::Input { seq, .. }
            | Event::Resize { seq, .. }
            | Event::Marker { seq, .. } => *seq,
        }
    }

    /// Wall-clock nanoseconds (`CLOCK_REALTIME`, clamped non-decreasing).
    pub fn t_ns(&self) -> u64 {
        match self {
            Event::Output { t_ns, .. }
            | Event::Input { t_ns, .. }
            | Event::Resize { t_ns, .. }
            | Event::Marker { t_ns, .. } => *t_ns,
        }
    }
}
