//! `redo list` — enumerate sessions under a storage root.

use std::path::Path;

use anyhow::Result;
use uuid::Uuid;

use crate::store::{SessionReader, SessionStore};

/// Summary printed by `redo list`.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: Uuid,
    pub started_at: String,
    pub frame_count: usize,
    pub byte_size: u64,
    pub state: Option<String>,
    pub is_partial: bool,
}

/// Walk `<root>/sessions/` and produce a summary for each child directory.
/// Entries that fail to load are skipped; the function logs and continues so
/// a single broken session doesn't poison the listing.
pub fn collect(root: &Path) -> Result<Vec<SessionSummary>> {
    let dirs = SessionStore::list_session_dirs(root)?;
    let mut out = Vec::new();
    for dir in dirs {
        let Some(name) = dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(name) else {
            continue;
        };
        let store = SessionStore::new(root, id);

        let log_path = store.log_path();
        let byte_size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);

        // Prefer meta.json: cheap, no decompression. The cached `frame_count`
        // and `created_at` are the source of truth for completed sessions.
        let meta = store.read_meta().ok();

        let state = meta.as_ref().map(|m| match m.state {
            crate::store::SessionState::Recording => "recording".to_string(),
            crate::store::SessionState::Finalizing => "finalizing".to_string(),
            crate::store::SessionState::Complete => "complete".to_string(),
            crate::store::SessionState::Crashed => "crashed".to_string(),
        });

        // Fast path: meta has a non-zero frame_count (or the session is still
        // recording, in which case meta is the freshest snapshot we have).
        // Slow path falls back to a streaming scan only for legacy sessions
        // (frame_count == 0 and not actively recording).
        let needs_scan = match &meta {
            Some(m) => {
                m.frame_count == 0 && !matches!(m.state, crate::store::SessionState::Recording)
            }
            None => true,
        };

        let (started_at, frame_count, is_partial) = if needs_scan {
            match SessionReader::read(&log_path) {
                Ok(r) => (r.header.created_at, r.events.len(), r.is_partial),
                Err(e) => {
                    tracing::warn!(session = %id, error = %e, "skipping unreadable session");
                    continue;
                }
            }
        } else {
            // Pull `created_at` from meta if we have it; otherwise read just
            // the header off the log via the streaming reader.
            let created_at = match meta.as_ref() {
                Some(m) => m.created_at.clone(),
                None => match crate::store::EventStream::open(&log_path) {
                    Ok(s) => s.header().created_at.clone(),
                    Err(e) => {
                        tracing::warn!(session = %id, error = %e, "skipping unreadable session");
                        continue;
                    }
                },
            };
            let frames = meta.as_ref().map(|m| m.frame_count as usize).unwrap_or(0);
            (created_at, frames, false)
        };

        out.push(SessionSummary {
            session_id: id,
            started_at,
            frame_count,
            byte_size,
            state,
            is_partial,
        });
    }
    // Newest first (UUIDv7 sorts lexicographically by time).
    out.sort_by_key(|s| std::cmp::Reverse(s.session_id));
    Ok(out)
}

/// Pretty-print summaries to stdout in a fixed-width table.
pub fn print(summaries: &[SessionSummary]) {
    if summaries.is_empty() {
        println!("(no sessions)");
        return;
    }
    println!(
        "{:<36}  {:<24}  {:>8}  {:>10}  {:<10}  PARTIAL",
        "SESSION", "STARTED", "FRAMES", "BYTES", "STATE"
    );
    for s in summaries {
        println!(
            "{:<36}  {:<24}  {:>8}  {:>10}  {:<10}  {}",
            s.session_id,
            s.started_at,
            s.frame_count,
            s.byte_size,
            s.state.as_deref().unwrap_or("?"),
            if s.is_partial { "yes" } else { "no" },
        );
    }
}
