//! `redo fork` — branch a session at a specific frame.
//!
//! Reads the source session, copies frames `[0..=at]` into a new session
//! directory under the same root, and writes a fresh `meta.json` whose
//! `parent_session_id` and `forked_at_frame` point back at the parent. A
//! closing `Marker` with label `fork-from <parent>:<frame>` is appended after
//! the copied prefix so a replayer sees a clear boundary.
//!
//! The new session id is returned so callers can chain `redo replay <id>`.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use uuid::Uuid;

use crate::format::{Event, SessionHeader};
use crate::store::{Meta, SessionReader, SessionState, SessionStore, SessionWriter};

/// Build a new session by copying `parent`'s frames `[0..=at]`. Returns the
/// fresh session id.
pub fn run(root: &Path, parent: Uuid, at: u64, label: Option<String>) -> Result<Uuid> {
    let parent_store = SessionStore::new(root, parent);
    let result =
        SessionReader::read(parent_store.log_path()).context("read parent session log for fork")?;

    let parent_events = &result.events;
    let take = (at as usize).min(parent_events.len().saturating_sub(1));
    if parent_events.is_empty() {
        return Err(anyhow!("parent session has no frames to fork from"));
    }
    if (at as usize) >= parent_events.len() {
        // Soft-clamp: forking past the last frame is treated as fork at the
        // last frame, so the user gets a complete copy plus the fork marker.
        tracing::warn!(
            requested = at,
            available = parent_events.len(),
            "fork frame beyond end; clamping"
        );
    }

    let new_id = Uuid::now_v7();
    let new_store = SessionStore::new(root, new_id);
    new_store.create().context("create forked session dir")?;

    // Build a header derived from the parent so the cmdline / cwd survive,
    // but with a fresh session_id and timestamp.
    let header = SessionHeader {
        version: result.header.version,
        format: result.header.format.clone(),
        session_id: new_id,
        created_at: iso8601_now(),
        cmd: result.header.cmd.clone(),
        env_term: result.header.env_term,
        claude_version: result.header.claude_version.clone(),
        redo_version: env!("CARGO_PKG_VERSION").into(),
        cwd: result.header.cwd.clone(),
    };

    let mut writer =
        SessionWriter::create(new_store.log_path(), &header).context("create fork session log")?;

    let mut next_seq: u64 = 0;
    for ev in parent_events.iter().take(take + 1) {
        let copied = reseq(ev, next_seq);
        writer.write_event(&copied).context("copy parent frame")?;
        next_seq += 1;
    }

    // Closing fork-from marker.
    let mut extras = serde_json::Map::new();
    extras.insert(
        "parent_session_id".into(),
        serde_json::Value::String(parent.to_string()),
    );
    extras.insert(
        "forked_at_frame".into(),
        serde_json::Value::Number((take as u64).into()),
    );
    if let Some(lbl) = &label {
        extras.insert("user_label".into(), serde_json::Value::String(lbl.clone()));
    }
    let fork_marker = Event::Marker {
        seq: next_seq,
        t_ns: now_ns(),
        label: format!("fork-from {parent}:{take}"),
        extras,
    };
    writer
        .write_event(&fork_marker)
        .context("write fork marker")?;
    writer.flush_frame().ok();
    writer.finish().context("finish fork log")?;

    let meta = Meta {
        session_id: new_id,
        state: SessionState::Complete,
        pid: std::process::id(),
        pid_starttime: 0,
        discarded_late_events: 0,
        created_at: header.created_at.clone(),
        parent_session_id: Some(parent),
        forked_at_frame: Some(take as u64),
    };
    new_store
        .write_meta(&meta)
        .context("write forked session meta")?;

    Ok(new_id)
}

fn reseq(e: &Event, new_seq: u64) -> Event {
    let mut copy = e.clone();
    match &mut copy {
        Event::Output { seq, .. }
        | Event::Input { seq, .. }
        | Event::Resize { seq, .. }
        | Event::Marker { seq, .. } => *seq = new_seq,
    }
    copy
}

fn now_ns() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn iso8601_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let nanos = d.subsec_nanos();
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{nanos:09}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}
