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

        let (started_at, frame_count, is_partial) = match SessionReader::read(&log_path) {
            Ok(r) => (r.header.created_at, r.events.len(), r.is_partial),
            Err(e) => {
                tracing::warn!(session = %id, error = %e, "skipping unreadable session");
                continue;
            }
        };

        let state = store.read_meta().ok().map(|m| match m.state {
            crate::store::SessionState::Recording => "recording".to_string(),
            crate::store::SessionState::Finalizing => "finalizing".to_string(),
            crate::store::SessionState::Complete => "complete".to_string(),
            crate::store::SessionState::Crashed => "crashed".to_string(),
        });

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
