//! Crash-safety properties of the session store.
//!
//! The writer emits a fresh zstd frame on every flush so that the reader can
//! salvage events from complete frames even if the trailing frame was lost
//! to a truncated file.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};

use redo::format::{Event, SessionHeader, TermSize};
use redo::store::{ReadResult, SessionReader, SessionStore, SessionWriter};
use serde_json::Map;
use tempfile::TempDir;
use uuid::Uuid;

fn header() -> SessionHeader {
    SessionHeader {
        version: 1,
        format: "redo".into(),
        session_id: Uuid::now_v7(),
        created_at: "2026-04-29T00:00:00Z".into(),
        cmd: vec!["claude".into()],
        env_term: TermSize { cols: 80, rows: 24 },
        claude_version: None,
        redo_version: "0.0.0".into(),
        cwd: "/".into(),
    }
}

fn marker(seq: u64) -> Event {
    Event::Marker {
        seq,
        t_ns: seq * 1000,
        label: format!("m{seq}"),
        extras: Map::new(),
    }
}

#[test]
fn create_lays_out_session_directory() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path(), Uuid::now_v7());
    store.create().unwrap();

    let dir = store.session_dir();
    assert!(dir.is_dir(), "session dir not created");
    assert!(store.dropbox_dir().is_dir(), "dropbox dir not created");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "session dir should be 0700, got {mode:o}");
    }
}

#[test]
fn meta_roundtrip_atomic() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path(), Uuid::now_v7());
    store.create().unwrap();

    let meta = redo::store::Meta {
        session_id: store.session_id(),
        state: redo::store::SessionState::Recording,
        pid: 12345,
        pid_starttime: 67890,
        discarded_late_events: 0,
        created_at: "2026-04-29T00:00:00Z".into(),
        parent_session_id: None,
        forked_at_frame: None,
    };
    store.write_meta(&meta).unwrap();
    let back = store.read_meta().unwrap();
    assert_eq!(meta.session_id, back.session_id);
    assert_eq!(meta.pid, back.pid);
    assert_eq!(meta.state, back.state);
}

#[test]
fn write_then_read_roundtrip_clean() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path(), Uuid::now_v7());
    store.create().unwrap();
    let h = header();

    {
        let mut w = SessionWriter::create(store.log_path(), &h).unwrap();
        for i in 0..1000 {
            w.write_event(&marker(i)).unwrap();
        }
        w.finish().unwrap();
    }

    let ReadResult {
        header: read_h,
        events,
        is_partial,
    } = SessionReader::read(store.log_path()).unwrap();
    assert_eq!(read_h.version, h.version);
    assert_eq!(events.len(), 1000);
    assert_eq!(events.first().unwrap().seq(), 0);
    assert_eq!(events.last().unwrap().seq(), 999);
    assert!(!is_partial, "clean log should not be flagged partial");
}

#[test]
fn truncated_log_recovers_complete_frames() {
    // Write 500 events with the default flush cadence (100 events / 250 ms).
    // That guarantees at least 5 internal frames before we truncate.
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path(), Uuid::now_v7());
    store.create().unwrap();
    let h = header();

    {
        let mut w = SessionWriter::create(store.log_path(), &h).unwrap();
        for i in 0..500 {
            w.write_event(&marker(i)).unwrap();
        }
        w.finish().unwrap();
    }

    // Lop off the trailing 200 bytes to simulate a crash mid-frame.
    {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(store.log_path())
            .unwrap();
        let len = f.seek(SeekFrom::End(0)).unwrap();
        let new_len = len.saturating_sub(200);
        f.set_len(new_len).unwrap();
        f.flush().unwrap();
    }

    let ReadResult {
        events, is_partial, ..
    } = SessionReader::read(store.log_path()).unwrap();
    assert!(
        is_partial,
        "truncated log should be flagged partial, got {} events",
        events.len()
    );
    assert!(
        events.len() >= 100,
        "expected at least one full frame's worth, got {}",
        events.len()
    );
    assert!(
        events.len() < 500,
        "should not recover all 500 events from a truncated file, got {}",
        events.len()
    );
    // Recovered events should be a strict prefix of the original sequence.
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.seq(), i as u64, "events must be a contiguous prefix");
    }
}

#[test]
fn force_flush_creates_recoverable_frame() {
    // Write only a handful of events but force a flush so the frame closes.
    // Even after truncation, we should recover those events.
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path(), Uuid::now_v7());
    store.create().unwrap();

    {
        let mut w = SessionWriter::create(store.log_path(), &header()).unwrap();
        for i in 0..5 {
            w.write_event(&marker(i)).unwrap();
        }
        w.flush_frame().unwrap();
        // Write five more without flushing -- they may be lost if we truncate.
        for i in 5..10 {
            w.write_event(&marker(i)).unwrap();
        }
        w.finish().unwrap();
    }

    // Truncate aggressively to drop the trailing frame.
    {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(store.log_path())
            .unwrap();
        let len = f.seek(SeekFrom::End(0)).unwrap();
        f.set_len(len.saturating_sub(50)).unwrap();
    }

    let ReadResult {
        events, is_partial, ..
    } = SessionReader::read(store.log_path()).unwrap();
    assert!(is_partial);
    assert!(
        events.len() >= 5,
        "the explicitly flushed frame must survive truncation, got {}",
        events.len()
    );
}

#[test]
fn empty_log_rejects_with_clear_error() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("empty.zst");
    std::fs::write(&p, b"").unwrap();
    let err = SessionReader::read(&p).unwrap_err().to_string();
    assert!(err.contains("no header"), "got: {err}");
}

#[test]
fn meta_deserialises_old_session_without_fork_fields() {
    // Older meta.json files predate `parent_session_id` and `forked_at_frame`.
    // The reader must accept them and default the missing fields to None.
    let json = r#"{
        "session_id": "00000000-0000-0000-0000-000000000000",
        "state": "complete",
        "pid": 1,
        "pid_starttime": 0,
        "discarded_late_events": 0,
        "created_at": "2026-01-01T00:00:00Z"
    }"#;
    let meta: redo::store::Meta = serde_json::from_str(json).expect("parse old meta.json");
    assert!(meta.parent_session_id.is_none());
    assert!(meta.forked_at_frame.is_none());
}

#[test]
fn meta_with_fork_fields_roundtrips() {
    let parent = Uuid::now_v7();
    let meta = redo::store::Meta {
        session_id: Uuid::now_v7(),
        state: redo::store::SessionState::Complete,
        pid: 1,
        pid_starttime: 0,
        discarded_late_events: 0,
        created_at: "2026-05-01T00:00:00Z".into(),
        parent_session_id: Some(parent),
        forked_at_frame: Some(42),
    };
    let s = serde_json::to_string(&meta).unwrap();
    let back: redo::store::Meta = serde_json::from_str(&s).unwrap();
    assert_eq!(back.parent_session_id, Some(parent));
    assert_eq!(back.forked_at_frame, Some(42));
}
