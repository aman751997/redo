//! `redo inspect` — emit a session's frames as JSON, one per line, on stdout.

use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::store::{SessionReader, SessionStore};

pub fn run(root: &Path, session_id: Uuid) -> Result<()> {
    let store = SessionStore::new(root, session_id);
    let result =
        SessionReader::read(store.log_path()).context("read session log for inspection")?;

    let stdout = io::stdout();
    let mut out = stdout.lock();

    // First line: the header, tagged so consumers can tell it apart.
    let header_line = serde_json::json!({ "kind": "__header", "header": result.header });
    writeln!(out, "{}", serde_json::to_string(&header_line)?)?;

    for ev in &result.events {
        writeln!(out, "{}", serde_json::to_string(ev)?)?;
    }

    if result.is_partial {
        let trailer = serde_json::json!({ "kind": "__trailer", "is_partial": true });
        writeln!(out, "{}", serde_json::to_string(&trailer)?)?;
    }
    Ok(())
}
