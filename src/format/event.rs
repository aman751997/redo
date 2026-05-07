use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Which standard stream an `Output` record came from. Optional: legacy
/// recordings (and PTY captures that don't separate the streams) leave it
/// unset.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// One log record. Internally tagged on `kind`.
///
/// Every variant carries `seq` (authoritative ordering, strictly increasing),
/// `t_ns` (wall-clock nanoseconds from `CLOCK_REALTIME`, clamped non-decreasing),
/// and `extras` — a catch-all that preserves unknown JSON fields verbatim
/// across roundtrips so a newer producer's records survive an older reader.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum Event {
    /// Captured stdout/stderr, projected from `PostToolUse[Bash]` hook payloads.
    Output {
        seq: u64,
        t_ns: u64,
        /// Base64-encoded bytes (capped at `MAX_INLINE_PAYLOAD`).
        bytes: String,
        /// Which stream the bytes came from, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream: Option<OutputStream>,
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

    /// Filesystem write captured from `PostToolUse[Edit|Write|MultiEdit]`. The
    /// recorder re-reads the file at the path the hook reports, blake3-hashes
    /// the bytes, and inlines them when small enough.
    ///
    /// Internally tagged as `"file_write"` (snake-cased, unlike the other
    /// variants) so the on-disk shape stays close to the canonical-line text.
    #[serde(rename = "file_write")]
    FileWrite {
        seq: u64,
        t_ns: u64,
        /// Absolute path the hook reported. Stored verbatim — no canonicalisation
        /// or symlink resolution.
        path: String,
        /// blake3 hex digest of the file content at re-read time.
        content_hash: String,
        /// Byte count of the file content at re-read time.
        size: u64,
        /// `true` when content exceeded `MAX_INLINE_PAYLOAD` and was not
        /// inlined. The hash + size still survive.
        #[serde(default)]
        truncated: bool,
        /// Original byte count when `truncated == true`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        truncated_original_size: Option<u64>,
        /// Base64-encoded file bytes when `size <= MAX_INLINE_PAYLOAD` and the
        /// re-read succeeded; absent otherwise.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inline_payload: Option<String>,
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
            | Event::Marker { seq, .. }
            | Event::FileWrite { seq, .. } => *seq,
        }
    }

    /// Wall-clock nanoseconds (`CLOCK_REALTIME`, clamped non-decreasing).
    pub fn t_ns(&self) -> u64 {
        match self {
            Event::Output { t_ns, .. }
            | Event::Input { t_ns, .. }
            | Event::Resize { t_ns, .. }
            | Event::Marker { t_ns, .. }
            | Event::FileWrite { t_ns, .. } => *t_ns,
        }
    }
}
