//! Command-line surface.
//!
//! Subcommands:
//!
//! * `record` — run the recorder daemon, watch a session's dropbox, append
//!   incoming hook events to the framed log.
//! * `list` — enumerate sessions under `<root>/sessions/`.
//! * `replay` — open the TUI on a session.
//! * `inspect` — dump a session's frames as JSON for scripting.
//! * `hook` — one-shot bridge invoked by Claude Code hooks; reads stdin,
//!   writes one dropbox file.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};

pub mod inspect;
pub mod list;

/// Top-level CLI entry point.
#[derive(Debug, Parser)]
#[command(
    name = "redo",
    version,
    about = "Time-travel debugger for Claude Code agent sessions"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the recorder daemon for a fresh session.
    Record {
        /// Session storage root. Defaults to `$XDG_DATA_HOME/redo` or
        /// `~/.local/share/redo`.
        #[arg(long)]
        root: Option<PathBuf>,
    },

    /// List recorded sessions.
    List {
        #[arg(long)]
        root: Option<PathBuf>,
    },

    /// Launch the TUI on a recorded session.
    Replay {
        /// Session UUID.
        session_id: String,
        #[arg(long)]
        root: Option<PathBuf>,
    },

    /// Print a session's frames as JSON, one per line.
    Inspect {
        session_id: String,
        #[arg(long)]
        root: Option<PathBuf>,
    },

    /// Hook bridge: read a Claude Code hook payload from stdin and stage it
    /// into the active session's dropbox. The session is resolved from the
    /// `REDO_SESSION_DIR` env var that `redo record` exports.
    Hook {
        /// Hook event name (e.g. `PreToolUse`, `PostToolUse`, `Stop`).
        kind: String,
        /// Override session dir instead of reading `REDO_SESSION_DIR`.
        #[arg(long)]
        session_dir: Option<PathBuf>,
    },
}

/// Compute the default storage root.
///
/// Order:
/// 1. `$XDG_DATA_HOME/redo`
/// 2. `$HOME/.local/share/redo`
pub fn default_root() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("redo"));
        }
    }
    let home = std::env::var("HOME").context("HOME not set; pass --root explicitly")?;
    if home.is_empty() {
        return Err(anyhow!("HOME is empty; pass --root explicitly"));
    }
    Ok(PathBuf::from(home).join(".local/share/redo"))
}

pub fn resolve_root(arg: Option<PathBuf>) -> Result<PathBuf> {
    match arg {
        Some(p) => Ok(p),
        None => default_root(),
    }
}
