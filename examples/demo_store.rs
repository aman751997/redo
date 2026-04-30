//! Tiny demo: write a session, truncate it, read it back.
//!
//! Run with: cargo run --example demo_store

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom};

use redo::format::{Event, SessionHeader, TermSize};
use redo::store::{ReadResult, SessionReader, SessionStore, SessionWriter};
use serde_json::Map;
use uuid::Uuid;

fn main() -> anyhow::Result<()> {
    let root = std::env::temp_dir().join("redo-demo");
    let _ = std::fs::remove_dir_all(&root);

    let session_id = Uuid::now_v7();
    let store = SessionStore::new(&root, session_id);
    store.create()?;

    println!("session id: {session_id}");
    println!("session dir: {}", store.session_dir().display());

    let header = SessionHeader {
        version: 1,
        format: "redo".into(),
        session_id,
        created_at: "2026-04-29T00:00:00Z".into(),
        cmd: vec!["claude".into()],
        env_term: TermSize {
            cols: 120,
            rows: 40,
        },
        claude_version: Some("1.0.0".into()),
        redo_version: env!("CARGO_PKG_VERSION").into(),
        cwd: "/tmp".into(),
    };

    // Write 250 marker events. Default flush cadence (100 events) closes
    // two complete frames; the trailing 50 events live in an unflushed frame.
    {
        let mut writer = SessionWriter::create(store.log_path(), &header)?;
        for i in 0..250 {
            writer.write_event(&Event::Marker {
                seq: i,
                t_ns: i * 1_000_000,
                label: format!("step-{i}"),
                extras: Map::new(),
            })?;
        }
        writer.finish()?;
    }

    let size = std::fs::metadata(store.log_path())?.len();
    println!("\nclean write:");
    println!(
        "  log file size: {size} bytes ({:.1} KB)",
        size as f64 / 1024.0
    );

    let ReadResult {
        events, is_partial, ..
    } = SessionReader::read(store.log_path())?;
    println!("  read back: {} events, partial={is_partial}", events.len());
    println!("  first: {:?}", events.first().map(|e| (e.seq(), e.t_ns())));
    println!("  last:  {:?}", events.last().map(|e| (e.seq(), e.t_ns())));

    // Now simulate a crash by lopping off the last 200 bytes.
    println!("\nsimulating crash (truncate last 200 bytes)...");
    {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(store.log_path())?;
        let len = f.seek(SeekFrom::End(0))?;
        f.set_len(len.saturating_sub(200))?;
    }

    let ReadResult {
        events, is_partial, ..
    } = SessionReader::read(store.log_path())?;
    println!("  recovered: {} events, partial={is_partial}", events.len());
    println!("  first: {:?}", events.first().map(|e| (e.seq(), e.t_ns())));
    println!("  last:  {:?}", events.last().map(|e| (e.seq(), e.t_ns())));

    // Show the on-disk JSON of one event so the format is visible.
    if let Some(e) = events.last() {
        let json = serde_json::to_string_pretty(e)?;
        println!("\nlast recovered event as JSON:\n{json}");
    }

    Ok(())
}
