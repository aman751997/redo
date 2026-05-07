//! Dropbox watcher.
//!
//! Linux: `inotify` on `IN_MOVED_TO | IN_CLOSE_WRITE | IN_CREATE`. The bridge
//! always rename(2)-s into place so `IN_MOVED_TO` is the primary signal; the
//! others are belt-and-suspenders.
//!
//! Non-Linux: poll the directory every 100 ms. Good enough for tests and dev
//! on macOS.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Watch `dir` for new dropbox files and call `on_file` per file. Returns when
/// `stop` is set. The closure may unlink the file after ingestion.
pub fn watch_until_stopped<F>(dir: &Path, stop: &Arc<AtomicBool>, mut on_file: F) -> Result<()>
where
    F: FnMut(&Path) -> Result<()>,
{
    // Always drain anything that landed before we attached.
    drain_now(dir, &mut on_file)?;

    #[cfg(target_os = "linux")]
    {
        watch_linux(dir, stop, &mut on_file)
    }
    #[cfg(not(target_os = "linux"))]
    {
        watch_polling(dir, stop, &mut on_file)
    }
}

fn is_visible(name: &str) -> bool {
    !(name.starts_with('.') || name.ends_with(".tmp"))
}

fn drain_now<F>(dir: &Path, on_file: &mut F) -> Result<()>
where
    F: FnMut(&Path) -> Result<()>,
{
    let mut entries: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.file_name()
                .and_then(|s| s.to_str())
                .map(is_visible)
                .unwrap_or(false)
            {
                entries.push(p);
            }
        }
    }
    entries.sort();
    for p in entries {
        on_file(&p)?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn watch_polling<F>(dir: &Path, stop: &Arc<AtomicBool>, on_file: &mut F) -> Result<()>
where
    F: FnMut(&Path) -> Result<()>,
{
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(POLL_INTERVAL);
        drain_now(dir, on_file)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn watch_linux<F>(dir: &Path, stop: &Arc<AtomicBool>, on_file: &mut F) -> Result<()>
where
    F: FnMut(&Path) -> Result<()>,
{
    use inotify::{Inotify, WatchMask};

    let mut ino = Inotify::init().map_err(|e| anyhow::anyhow!("inotify init: {e}"))?;
    ino.watches()
        .add(
            dir,
            WatchMask::MOVED_TO | WatchMask::CLOSE_WRITE | WatchMask::CREATE,
        )
        .map_err(|e| anyhow::anyhow!("inotify watch {}: {e}", dir.display()))?;

    let mut buf = [0u8; 4096];
    while !stop.load(Ordering::Relaxed) {
        // Short read so we wake to check the stop flag.
        match ino.read_events(&mut buf) {
            Ok(events) => {
                let mut new_paths: Vec<PathBuf> = Vec::new();
                for ev in events {
                    let Some(name) = ev.name.and_then(|n| n.to_str()) else {
                        continue;
                    };
                    if !is_visible(name) {
                        continue;
                    }
                    new_paths.push(dir.join(name));
                }
                new_paths.sort();
                for p in new_paths {
                    on_file(&p)?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(anyhow::anyhow!("inotify read: {e}")),
        }
    }
    Ok(())
}
