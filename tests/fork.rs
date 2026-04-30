//! Integration coverage for `redo::cli::fork::run`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use redo::cli::fork;
use redo::format::Event;
use redo::hook::{write_envelope, Envelope};
use redo::recorder::{self, Config};
use redo::store::{SessionReader, SessionStore};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

fn record_session_with_n_events(root: &std::path::Path, n: usize) -> Uuid {
    let session_id = Uuid::now_v7();
    let store = SessionStore::new(root, session_id);
    store.create().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let cfg = Config {
        root: root.to_path_buf(),
        session_id: Some(session_id),
        print_banner: false,
        stop: Some(stop.clone()),
    };
    let handle = thread::spawn(move || recorder::run(cfg).unwrap());

    let dropbox = store.dropbox_dir();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dropbox.is_dir() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    for i in 0..n {
        let env = Envelope::now("PreToolUse", json!({"i": i}));
        write_envelope(&dropbox, &env).unwrap();
        thread::sleep(Duration::from_millis(2));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = std::fs::read_dir(&dropbox)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        e.file_name()
                            .to_str()
                            .map(|s| !s.starts_with('.') && !s.ends_with(".tmp"))
                            .unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(0);
        if remaining == 0 {
            break;
        }
        if Instant::now() > deadline {
            panic!("dropbox never drained");
        }
        thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::Relaxed);
    handle.join().unwrap();
    session_id
}

#[test]
fn fork_copies_prefix_and_appends_marker() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let parent = record_session_with_n_events(root, 5);

    // Fork at frame 2 — should copy frames 0..=2 plus the closing fork marker.
    let new_id = fork::run(root, parent, 2, Some("experiment".into())).expect("fork");
    assert_ne!(new_id, parent);

    let new_store = SessionStore::new(root, new_id);
    let res = SessionReader::read(new_store.log_path()).expect("read forked log");
    assert_eq!(res.events.len(), 4, "3 copied frames + 1 fork marker");

    // Sequence numbers must be a contiguous prefix from 0.
    for (i, e) in res.events.iter().enumerate() {
        assert_eq!(e.seq(), i as u64);
    }

    // Last event is the fork marker.
    match res.events.last().unwrap() {
        Event::Marker { label, extras, .. } => {
            assert!(label.starts_with(&format!("fork-from {parent}:2")));
            assert_eq!(
                extras.get("parent_session_id").and_then(|v| v.as_str()),
                Some(parent.to_string().as_str())
            );
            assert_eq!(
                extras.get("forked_at_frame").and_then(|v| v.as_u64()),
                Some(2)
            );
            assert_eq!(
                extras.get("user_label").and_then(|v| v.as_str()),
                Some("experiment")
            );
        }
        _ => panic!("last event is not a marker"),
    }

    // Meta points back at parent.
    let meta = new_store.read_meta().unwrap();
    assert_eq!(meta.parent_session_id, Some(parent));
    assert_eq!(meta.forked_at_frame, Some(2));
}

#[test]
fn fork_clamps_frame_past_end() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let parent = record_session_with_n_events(root, 3);

    // Asking for frame 999 should clamp; the user gets the full prefix.
    let new_id = fork::run(root, parent, 999, None).expect("fork clamp");
    let new_store = SessionStore::new(root, new_id);
    let res = SessionReader::read(new_store.log_path()).expect("read forked log");
    let parent_store = SessionStore::new(root, parent);
    let parent_res = SessionReader::read(parent_store.log_path()).unwrap();
    // Forked log = parent_len + 1 (fork marker).
    assert_eq!(res.events.len(), parent_res.events.len() + 1);
}
