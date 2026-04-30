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
}

impl Envelope {
    pub fn now(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            source: "hook".into(),
            kind: kind.into(),
            received_t_ns: now_ns(),
            payload,
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

    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("read hook stdin")?;

    // Empty stdin is allowed — some hooks fire with no payload. Default to {}.
    let payload: Value = if buf.trim().is_empty() {
        Value::Object(Default::default())
    } else {
        serde_json::from_str(&buf).with_context(|| format!("parse hook stdin as JSON: {buf}"))?
    };

    let env = Envelope::now(kind, payload);
    write_envelope(&dropbox, &env)
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
    crate::format::Event::Marker {
        seq,
        t_ns: env.received_t_ns,
        label: format!("hook:{}", env.kind),
        extras,
    }
}
