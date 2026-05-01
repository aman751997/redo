//! Hook bridge: parses a Claude Code hook JSON envelope from stdin and writes
//! a single line into a session's `dropbox/`. The recorder watches that
//! directory and ingests each file into the framed log.
//!
//! The on-disk dropbox format is intentionally stable across the bridge and
//! the recorder. See [`Envelope`].

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Environment variable the recorder sets so spawned hooks know where to drop
/// their JSON. Points at the session directory (containing `dropbox/`).
pub const SESSION_DIR_ENV: &str = "REDO_SESSION_DIR";

/// On-disk dropbox JSON envelope.
///
/// The bridge writes one of these per hook invocation. The `payload` is the
/// raw stdin JSON — the recorder is responsible for projecting it onto our
/// `Event` variants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Envelope {
    /// Always "hook" today; reserved for future producer kinds (pty, sdk, ...).
    pub source: String,
    /// Free-form short tag. For Claude Code hooks this is the hook event name
    /// such as `PreToolUse`, `PostToolUse`, `Stop`, `Notification`, etc.
    pub kind: String,
    /// Wall-clock receive time in nanoseconds since UNIX epoch.
    pub received_t_ns: u64,
    /// Raw hook stdin, parsed as JSON.
    pub payload: Value,
    /// `true` when the bridge stopped reading stdin at `MAX_INLINE_PAYLOAD`.
    /// In that case `payload` is `Value::Null` and the original byte count is
    /// surfaced in `truncated_original_size`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// Original payload byte count when `truncated == Some(true)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated_original_size: Option<usize>,
}

impl Envelope {
    pub fn now(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            source: "hook".into(),
            kind: kind.into(),
            received_t_ns: now_ns(),
            payload,
            truncated: None,
            truncated_original_size: None,
        }
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Read hook stdin and write a dropbox file. Returns the path written.
pub fn run(kind: &str, session_dir_override: Option<PathBuf>) -> Result<PathBuf> {
    let session_dir = match session_dir_override {
        Some(p) => p,
        None => std::env::var(SESSION_DIR_ENV)
            .map(PathBuf::from)
            .map_err(|_| anyhow!("{SESSION_DIR_ENV} not set; nothing to do"))?,
    };
    let dropbox = session_dir.join("dropbox");
    if !dropbox.is_dir() {
        return Err(anyhow!("dropbox dir does not exist: {}", dropbox.display()));
    }

    let mut env = build_envelope(kind, &mut std::io::stdin().lock())?;
    // received_t_ns was set to now() by build_envelope already.
    let _ = &mut env;
    write_envelope(&dropbox, &env)
}

/// Read a hook payload from `r`, capped at `MAX_INLINE_PAYLOAD` bytes. Builds
/// an `Envelope` with `received_t_ns = now()`. Pulled out of `run` so tests
/// can drive it without needing a real stdin.
pub fn build_envelope<R: Read>(kind: &str, r: &mut R) -> Result<Envelope> {
    let cap = crate::format::MAX_INLINE_PAYLOAD;
    let mut buf = Vec::with_capacity(cap.min(8192));
    r.take((cap as u64) + 1)
        .read_to_end(&mut buf)
        .context("read hook stdin")?;

    let (payload, truncated, original): (Value, Option<bool>, Option<usize>) = if buf.len() > cap {
        // Oversized: drop the body, but record the original (over-cap) size so
        // canonicalisation surfaces "N bytes (truncated)".
        let original = buf.len();
        tracing::warn!(
            cap,
            original,
            "hook stdin exceeded MAX_INLINE_PAYLOAD; dropping payload"
        );
        (Value::Null, Some(true), Some(original))
    } else if buf.is_empty() || buf.iter().all(|b| b.is_ascii_whitespace()) {
        // Empty stdin is allowed — some hooks fire with no payload.
        (Value::Object(Default::default()), None, None)
    } else {
        let s = std::str::from_utf8(&buf).context("hook stdin not valid UTF-8")?;
        let v: Value =
            serde_json::from_str(s).with_context(|| format!("parse hook stdin as JSON: {s}"))?;
        (v, None, None)
    };

    let mut env = Envelope::now(kind, payload);
    env.truncated = truncated;
    env.truncated_original_size = original;
    Ok(env)
}

/// Atomically write an envelope into `<dropbox>/`. The filename is
/// `<received_t_ns>-<uuidv7>.json`; we write to a `.tmp` sibling and rename so
/// the recorder never sees a half-written file.
pub fn write_envelope(dropbox: &Path, env: &Envelope) -> Result<PathBuf> {
    let id = Uuid::now_v7();
    let final_name = format!("{:020}-{}.json", env.received_t_ns, id);
    let tmp_name = format!(".{final_name}.tmp");
    let final_path = dropbox.join(&final_name);
    let tmp_path = dropbox.join(&tmp_name);

    let bytes = serde_json::to_vec(env).context("serialize envelope")?;
    {
        let mut f = fs::File::create(&tmp_path)
            .with_context(|| format!("create {}", tmp_path.display()))?;
        f.write_all(&bytes)
            .with_context(|| format!("write {}", tmp_path.display()))?;
        f.sync_data().ok();
    }
    fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), final_path.display()))?;
    Ok(final_path)
}

/// Project a dropbox envelope onto a `Marker` event with the hook payload
/// preserved verbatim in `extras`.
pub fn envelope_to_event(env: &Envelope, seq: u64) -> crate::format::Event {
    use serde_json::{Map, Value as JsonValue};
    let mut extras: Map<String, JsonValue> = Map::new();
    extras.insert("source".into(), JsonValue::String(env.source.clone()));
    extras.insert("payload".into(), env.payload.clone());
    if let Some(t) = env.truncated {
        extras.insert("truncated".into(), JsonValue::Bool(t));
    }
    if let Some(n) = env.truncated_original_size {
        extras.insert(
            "truncated_original_size".into(),
            JsonValue::Number((n as u64).into()),
        );
    }
    crate::format::Event::Marker {
        seq,
        t_ns: env.received_t_ns,
        label: format!("hook:{}", env.kind),
        extras,
    }
}
