//! Recorder daemon.
//!
//! Owns the on-disk session directory for the duration of a recording. Watches
//! `<session>/dropbox/` for hook envelopes, projects them into `Event`s, and
//! appends them to the framed log. Handles SIGINT/SIGTERM by draining the
//! dropbox, flushing the writer, and finalising `meta.json`.
//!
//! Linux uses `inotify` to wake on new files; other platforms (used for tests
//! and local dev) fall back to a 100 ms polling loop.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::format::{SessionHeader, TermSize};
use crate::hook::{Envelope, SESSION_DIR_ENV};
use crate::store::{Meta, SessionState, SessionStore, SessionWriter};

mod watcher;

/// How often to rewrite `meta.json` with the current frame count.
const META_UPDATE_INTERVAL: Duration = Duration::from_secs(2);

/// Maximum dropbox queue drain per wake-up. Keeps a single burst from
/// blocking the shutdown path indefinitely.
const MAX_DRAIN_PER_TICK: usize = 1024;

/// Tunables for `run`. All optional with sensible defaults.
#[derive(Debug, Clone)]
pub struct Config {
    pub root: PathBuf,
    /// If `Some`, used for the new session id. If `None`, a fresh UUIDv7.
    pub session_id: Option<Uuid>,
    /// Print the session info banner on stdout for the launching shell to read.
    pub print_banner: bool,
    /// Optional external shutdown flag. If `None`, the recorder installs
    /// SIGINT/SIGTERM handlers itself.
    pub stop: Option<Arc<AtomicBool>>,
}

/// Per-recorder runtime context shared with the watcher.
struct Recorder {
    store: SessionStore,
    writer: SessionWriter,
    next_seq: u64,
    last_meta_write: Instant,
    meta: Meta,
}

/// Run the recorder until SIGINT/SIGTERM. Returns the session id.
pub fn run(cfg: Config) -> Result<Uuid> {
    let session_id = cfg.session_id.unwrap_or_else(Uuid::now_v7);
    let store = SessionStore::new(&cfg.root, session_id);
    store.create().context("create session directory")?;

    let header = build_header(session_id);
    let mut writer =
        SessionWriter::create(store.log_path(), &header).context("open session log for writing")?;

    let meta = Meta {
        session_id,
        state: SessionState::Recording,
        pid: std::process::id(),
        pid_starttime: 0,
        discarded_late_events: 0,
        ingest_errors: 0,
        frame_count: 0,
        created_at: header.created_at.clone(),
        parent_session_id: None,
        forked_at_frame: None,
    };
    store.write_meta(&meta).context("write initial meta.json")?;

    if cfg.print_banner {
        // Stable banner consumed by tests and by humans wiring up hooks.
        println!("session_id={session_id}");
        println!("session_dir={}", store.session_dir().display());
        println!("dropbox={}", store.dropbox_dir().display());
        println!("env {}={}", SESSION_DIR_ENV, store.session_dir().display());
    }
    tracing::info!(%session_id, dir = %store.session_dir().display(), "recorder started");

    // Force-flush the header frame so a watcher can already read this session
    // even before the first event arrives.
    writer.flush_frame().ok();

    let stop = match cfg.stop.clone() {
        Some(s) => s,
        None => install_signal_handlers()?,
    };

    let mut rec = Recorder {
        store: store.clone(),
        writer,
        next_seq: 0,
        last_meta_write: Instant::now(),
        meta,
    };

    let dropbox = store.dropbox_dir();
    // Catch ingest errors inside the closure so a single bad file (transient
    // EIO, writer hiccup) doesn't kill the loop. The watcher itself can still
    // fail (e.g. inotify init), in which case we propagate after finalize.
    let watch_result = watcher::watch_until_stopped(&dropbox, &stop, |path| {
        if let Err(e) = rec.ingest_one(path) {
            tracing::warn!(file = %path.display(), error = %e, "ingest failed; continuing");
            rec.meta.ingest_errors = rec.meta.ingest_errors.saturating_add(1);
        }
        Ok(())
    });

    // Drain anything that landed between the last wake-up and the signal.
    // Same swallow-and-count policy: we want finalize to run no matter what.
    if let Err(e) = rec.drain_dropbox(MAX_DRAIN_PER_TICK) {
        tracing::warn!(error = %e, "post-stop drain failed; continuing to finalize");
    }
    rec.finalize()?;

    // Surface a watcher-level failure only after the log is closed cleanly.
    watch_result?;

    tracing::info!(%session_id, "recorder shut down clean");
    Ok(session_id)
}

impl Recorder {
    fn ingest_one(&mut self, path: &Path) -> Result<()> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).with_context(|| format!("read dropbox {}", path.display())),
        };
        let env: Envelope = match serde_json::from_slice(&bytes) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "discarding malformed dropbox file");
                let _ = std::fs::remove_file(path);
                return Ok(());
            }
        };
        let event = crate::hook::envelope_to_event(&env, self.next_seq);
        self.writer
            .write_event(&event)
            .context("append hook event")?;
        self.next_seq += 1;
        // Best-effort unlink — leftover dropbox files are harmless on a retry,
        // but worth surfacing.
        if let Err(e) = std::fs::remove_file(path) {
            tracing::debug!(file = %path.display(), error = %e, "remove dropbox failed");
        }
        self.maybe_update_meta()?;
        Ok(())
    }

    fn drain_dropbox(&mut self, max: usize) -> Result<usize> {
        let mut entries: Vec<PathBuf> = Vec::new();
        let dir = self.store.dropbox_dir();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                let name = match p.file_name().and_then(|s| s.to_str()) {
                    Some(n) => n,
                    None => continue,
                };
                // Skip the bridge's in-flight temp files.
                if name.starts_with('.') || name.ends_with(".tmp") {
                    continue;
                }
                entries.push(p);
            }
        }
        // Lexicographic sort matches the bridge's filename layout
        // `<received_t_ns>-<uuid>.json`, so we drain in arrival order.
        entries.sort();
        let drained = entries.len().min(max);
        for p in entries.into_iter().take(max) {
            self.ingest_one(&p)?;
        }
        Ok(drained)
    }

    fn maybe_update_meta(&mut self) -> Result<()> {
        if self.last_meta_write.elapsed() < META_UPDATE_INTERVAL {
            return Ok(());
        }
        self.last_meta_write = Instant::now();
        self.meta.frame_count = self.next_seq;
        self.store
            .write_meta(&self.meta)
            .context("rewrite meta.json")?;
        // Force a flush so a crash within the next interval loses at most
        // META_UPDATE_INTERVAL of recent events.
        self.writer.flush_frame().ok();
        Ok(())
    }

    fn finalize(mut self) -> Result<()> {
        // Closing marker so the replayer can show a clean end-of-session.
        let marker = crate::format::Event::Marker {
            seq: self.next_seq,
            t_ns: now_ns(),
            label: "session_end".into(),
            extras: serde_json::Map::new(),
        };
        let mut wrote_marker = false;
        if self.writer.write_event(&marker).is_ok() {
            wrote_marker = true;
            self.next_seq += 1;
        }
        self.writer.flush_frame().ok();
        self.writer.finish().context("finish writer")?;

        self.meta.state = SessionState::Complete;
        self.meta.frame_count = self.next_seq;
        let _ = wrote_marker; // future: surface via a counter if useful
        self.store
            .write_meta(&self.meta)
            .context("write final meta.json")?;
        Ok(())
    }
}

fn build_header(session_id: Uuid) -> SessionHeader {
    SessionHeader {
        version: crate::format::FORMAT_VERSION,
        format: crate::format::FORMAT_NAME.into(),
        session_id,
        created_at: iso8601_now(),
        cmd: std::env::args().collect(),
        env_term: detect_term_size(),
        claude_version: std::env::var("CLAUDE_CODE_VERSION").ok(),
        redo_version: env!("CARGO_PKG_VERSION").into(),
        cwd: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    }
}

fn detect_term_size() -> TermSize {
    // crossterm is already a dep; fall back to 80x24 if we're not on a tty.
    crossterm::terminal::size()
        .map(|(cols, rows)| TermSize { cols, rows })
        .unwrap_or(TermSize { cols: 80, rows: 24 })
}

fn iso8601_now() -> String {
    // Hand-roll a minimal ISO-8601 to avoid pulling in chrono. Sufficient for
    // human-readable headers; consumers parse with off-the-shelf libs.
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let nanos = d.subsec_nanos();
    format_unix_seconds(secs, nanos)
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn format_unix_seconds(mut secs: u64, nanos: u32) -> String {
    // Days since epoch.
    let days = secs / 86_400;
    secs %= 86_400;
    let hour = secs / 3_600;
    let minute = (secs % 3_600) / 60;
    let second = secs % 60;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:09}Z",
        nanos
    )
}

// Howard Hinnant's date algorithm. Public-domain. Converts days since
// 1970-01-01 to (year, month, day) on the proleptic Gregorian calendar.
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

fn install_signal_handlers() -> Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGINT, SIGTERM};
        signal_hook::flag::register(SIGINT, stop.clone()).context("install SIGINT handler")?;
        signal_hook::flag::register(SIGTERM, stop.clone()).context("install SIGTERM handler")?;
    }
    Ok(stop)
}
