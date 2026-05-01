//! Hook bridge: parses a Claude Code hook JSON envelope from stdin and writes
//! a single line into a session's `dropbox/`. The recorder watches that
//! directory and ingests each file into the framed log.
//!
//! The on-disk dropbox format is intentionally stable across the bridge and
//! the recorder. See [`Envelope`].

pub mod schema;

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

/// Build the catch-all `Marker` event that every hook envelope projects to.
/// The full payload is preserved under `extras.payload` for forensic use, and
/// truncation flags propagate so canonicalisation can surface them.
fn envelope_to_marker(env: &Envelope, seq: u64) -> crate::format::Event {
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

/// Single-event projection: the parent `Marker` only. Retained for callers
/// (tests, external scripting) that just want the boundary event.
pub fn envelope_to_event(env: &Envelope, seq: u64) -> crate::format::Event {
    envelope_to_marker(env, seq)
}

/// Full projection: zero or more side events (for example a Bash `Output` or
/// an `Edit`/`Write`/`MultiEdit` `FileWrite`) followed by the catch-all
/// `Marker`. The recorder writes the events in the returned order.
///
/// Sequence numbers start at `start_seq` and increase by one per event.
pub fn envelope_to_events(env: &Envelope, start_seq: u64) -> Vec<crate::format::Event> {
    let mut out: Vec<crate::format::Event> = Vec::new();
    let mut seq = start_seq;

    // Validate the payload shape against the contract pinned in
    // `schema::expected_fields`. Drift is logged but not fatal.
    let report = schema::validate(&env.kind, &env.payload);
    if !report.ok() {
        schema::warn_drift(&env.kind, &report.missing_fields);
    }

    // Side events are derived only from PostToolUse[Bash|Edit|Write|MultiEdit].
    if env.kind == "PostToolUse" {
        let tool_name = env
            .payload
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        match tool_name {
            "Bash" => {
                if let Some(ev) = project_bash_output(env, seq) {
                    out.push(ev);
                    seq += 1;
                }
            }
            "Edit" | "Write" | "MultiEdit" => {
                if let Some(ev) = project_file_write(env, seq) {
                    out.push(ev);
                    seq += 1;
                }
            }
            _ => {}
        }
    }

    out.push(envelope_to_marker(env, seq));
    out
}

/// Pull `tool_response.stdout` (and fall back to `stderr` if stdout is empty
/// or missing) out of a `PostToolUse[Bash]` envelope and project it onto an
/// `Output` event tagged with `extras.source = "bash"`.
fn project_bash_output(env: &Envelope, seq: u64) -> Option<crate::format::Event> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use serde_json::{Map, Value as JsonValue};

    let resp = env.payload.get("tool_response")?;
    let (stream_kind, raw): (crate::format::OutputStream, &str) = match (
        resp.get("stdout").and_then(|v| v.as_str()),
        resp.get("stderr").and_then(|v| v.as_str()),
    ) {
        (Some(s), _) if !s.is_empty() => (crate::format::OutputStream::Stdout, s),
        (_, Some(s)) if !s.is_empty() => (crate::format::OutputStream::Stderr, s),
        _ => return None,
    };

    let cap = crate::format::MAX_INLINE_PAYLOAD;
    let bytes = raw.as_bytes();
    let (encoded, truncated, original_size) = if bytes.len() > cap {
        (B64.encode(&bytes[..cap]), Some(true), Some(bytes.len()))
    } else {
        (B64.encode(bytes), None, None)
    };

    let mut extras: Map<String, JsonValue> = Map::new();
    extras.insert("source".into(), JsonValue::String("bash".into()));

    Some(crate::format::Event::Output {
        seq,
        t_ns: env.received_t_ns,
        bytes: encoded,
        stream: Some(stream_kind),
        truncated,
        truncated_original_size: original_size,
        extras,
    })
}

/// Pull `tool_input.file_path` out of a `PostToolUse[Edit|Write|MultiEdit]`
/// envelope, re-read the file from disk, blake3-hash it, and project a
/// `FileWrite` event. IO errors are logged and skipped — the parent marker
/// still ships, so a failed re-read is loud but not fatal.
fn project_file_write(env: &Envelope, seq: u64) -> Option<crate::format::Event> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use serde_json::Map;

    let path_str = env
        .payload
        .get("tool_input")
        .and_then(|v| v.get("file_path"))
        .and_then(|v| v.as_str())?;

    // TODO(v0.2): close the race window between hook firing and re-read by
    // reconstructing content from `tool_input.content` / `(old_string,
    // new_string)` over the previous snapshot. See
    // docs/audits/2026-05-01-state-of-the-project.md.
    let bytes = match std::fs::read(path_str) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                path = path_str,
                error = %e,
                "FileWrite re-read failed; emitting marker only"
            );
            return None;
        }
    };

    let cap = crate::format::MAX_INLINE_PAYLOAD;
    let size = bytes.len() as u64;
    let content_hash = blake3::hash(&bytes).to_hex().to_string();
    let (inline_payload, truncated, truncated_original_size) = if bytes.len() > cap {
        (None, true, Some(size))
    } else {
        (Some(B64.encode(&bytes)), false, None)
    };

    Some(crate::format::Event::FileWrite {
        seq,
        t_ns: env.received_t_ns,
        path: path_str.to_string(),
        content_hash,
        size,
        truncated,
        truncated_original_size,
        inline_payload,
        extras: Map::new(),
    })
}
