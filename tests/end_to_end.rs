//! End-to-end coverage of the record/hook/inspect pipeline.
//!
//! These tests exercise the public library surface — `recorder::run` driven
//! by an external stop flag, plus `hook::write_envelope` for staging events —
//! rather than spawning the binary, so they run cleanly in `cargo test`
//! without depending on $PATH or build outputs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use redo::format::Event;
use redo::hook::{write_envelope, Envelope};
use redo::recorder::{self, Config};
use redo::store::{SessionReader, SessionStore};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

fn drop_synthetic_event(dropbox: &std::path::Path, kind: &str, payload: serde_json::Value) {
    let env = Envelope::now(kind, payload);
    write_envelope(dropbox, &env).expect("write envelope");
}

#[test]
fn records_dropbox_events_in_order() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let session_id = Uuid::now_v7();

    // Pre-create the directory layout so the bridge has a target before the
    // recorder thread races to do it itself.
    let store = SessionStore::new(&root, session_id);
    store.create().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let cfg = Config {
        root: root.clone(),
        session_id: Some(session_id),
        print_banner: false,
        stop: Some(stop.clone()),
    };

    let handle = thread::spawn(move || recorder::run(cfg).expect("recorder run"));

    let dropbox = store.dropbox_dir();
    // Wait for the recorder to finish creating the session and the dropbox.
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dropbox.is_dir() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(dropbox.is_dir(), "recorder did not create dropbox in time");

    // Drop a handful of synthetic hook events.
    let kinds = ["PreToolUse", "PostToolUse", "UserPromptSubmit", "Stop"];
    for (i, kind) in kinds.iter().enumerate() {
        drop_synthetic_event(
            &dropbox,
            kind,
            json!({"tool_name": "Bash", "iter": i, "command": format!("echo {i}")}),
        );
        // Tiny gap so received_t_ns differs and lexicographic ordering matches
        // logical ordering even on slow CI clocks.
        thread::sleep(Duration::from_millis(2));
    }

    // Wait for them to be drained out of the dropbox.
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
            panic!("dropbox never drained: {remaining} files left");
        }
        thread::sleep(Duration::from_millis(50));
    }

    // Tell the recorder to shut down and wait.
    stop.store(true, Ordering::Relaxed);
    let returned_id = handle.join().expect("recorder thread panic");
    assert_eq!(returned_id, session_id);

    // Read it back and confirm we captured one frame per dropped envelope,
    // in the order they were dropped, plus the closing `session_end` marker.
    let result = SessionReader::read(store.log_path()).expect("read log");
    assert!(
        !result.is_partial,
        "log should not be partial after clean shutdown"
    );
    assert_eq!(result.header.session_id, session_id);

    let labels: Vec<String> = result
        .events
        .iter()
        .map(|e| match e {
            Event::Marker { label, .. } => label.clone(),
            _ => "(non-marker)".into(),
        })
        .collect();

    assert_eq!(
        labels,
        vec![
            "hook:PreToolUse".to_string(),
            "hook:PostToolUse".into(),
            "hook:UserPromptSubmit".into(),
            "hook:Stop".into(),
            "session_end".into(),
        ],
        "events should be ingested in dropbox order with a trailing session_end marker"
    );

    // Sequence numbers should be a contiguous prefix.
    for (i, e) in result.events.iter().enumerate() {
        assert_eq!(e.seq(), i as u64, "seq must be contiguous from 0");
    }

    // Meta should land in Complete state.
    let meta = store.read_meta().expect("read meta");
    assert!(matches!(meta.state, redo::store::SessionState::Complete));
}

#[test]
fn malformed_dropbox_file_is_skipped() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let session_id = Uuid::now_v7();
    let store = SessionStore::new(&root, session_id);
    store.create().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let cfg = Config {
        root: root.clone(),
        session_id: Some(session_id),
        print_banner: false,
        stop: Some(stop.clone()),
    };
    let handle = thread::spawn(move || recorder::run(cfg).expect("recorder run"));

    let dropbox = store.dropbox_dir();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dropbox.is_dir() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }

    // One bad file followed by one good one. The bad one must be dropped and
    // not poison the recorder; the good one must still land in the log.
    std::fs::write(dropbox.join("00000000000000000001-bad.json"), b"not json {").unwrap();
    drop_synthetic_event(&dropbox, "PreToolUse", json!({"ok": true}));

    // Wait for drain.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let count = std::fs::read_dir(&dropbox)
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
        if count == 0 {
            break;
        }
        if Instant::now() > deadline {
            panic!("dropbox never drained");
        }
        thread::sleep(Duration::from_millis(50));
    }

    stop.store(true, Ordering::Relaxed);
    handle.join().unwrap();

    let result = SessionReader::read(store.log_path()).unwrap();
    let labels: Vec<&str> = result
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Marker { label, .. } => Some(label.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        labels,
        vec!["hook:PreToolUse", "session_end"],
        "the bad file must not produce a frame"
    );
}

#[test]
fn ingest_error_increments_counter_and_finalizes() {
    // Inject a path the recorder can't read (a directory in place of a file).
    // The watcher loop must log + count + continue, then finalize cleanly.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let session_id = Uuid::now_v7();
    let store = SessionStore::new(&root, session_id);
    store.create().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let cfg = Config {
        root: root.clone(),
        session_id: Some(session_id),
        print_banner: false,
        stop: Some(stop.clone()),
    };
    let handle = thread::spawn(move || recorder::run(cfg).expect("recorder run"));

    let dropbox = store.dropbox_dir();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dropbox.is_dir() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }

    // A subdir whose filename matches the dropbox naming convention. The
    // recorder will try `read()` on it and fail with `IsADirectory`. That
    // path used to kill the loop; now it should be counted and skipped.
    std::fs::create_dir(dropbox.join("00000000000000000001-baddir.json")).unwrap();

    // Follow it up with a real envelope so we can prove the loop survived.
    drop_synthetic_event(&dropbox, "PreToolUse", json!({"ok": true}));

    // Wait for the good envelope to be ingested (the bad dir won't be).
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let count = std::fs::read_dir(&dropbox)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        let n = e.file_name();
                        let s = n.to_str().unwrap_or("");
                        !s.starts_with('.') && !s.ends_with(".tmp") && !s.contains("baddir")
                    })
                    .count()
            })
            .unwrap_or(0);
        if count == 0 {
            break;
        }
        if Instant::now() > deadline {
            panic!("good dropbox file never drained");
        }
        thread::sleep(Duration::from_millis(50));
    }

    stop.store(true, Ordering::Relaxed);
    handle.join().unwrap();

    let meta = store.read_meta().expect("read meta");
    assert!(
        matches!(meta.state, redo::store::SessionState::Complete),
        "meta should be Complete (finalize ran), got {:?}",
        meta.state
    );
    assert!(
        meta.ingest_errors >= 1,
        "ingest_errors should have been incremented at least once, got {}",
        meta.ingest_errors
    );
    // Frame count cached in meta should match what the log actually holds:
    // one good envelope + the session_end marker = 2.
    let result = SessionReader::read(store.log_path()).expect("read log");
    assert_eq!(meta.frame_count as usize, result.events.len());
}

#[test]
fn list_uses_meta_frame_count_without_decompressing_log() {
    // After a clean recording, meta.frame_count should equal events.len() and
    // `list::collect` should report it (proxy for the no-decompression path,
    // since meta is the only source of that count when meta.frame_count > 0).
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let session_id = Uuid::now_v7();
    let store = SessionStore::new(&root, session_id);
    store.create().unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let cfg = Config {
        root: root.clone(),
        session_id: Some(session_id),
        print_banner: false,
        stop: Some(stop.clone()),
    };
    let handle = thread::spawn(move || recorder::run(cfg).expect("recorder run"));

    let dropbox = store.dropbox_dir();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !dropbox.is_dir() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }

    for i in 0..5 {
        drop_synthetic_event(&dropbox, "PreToolUse", json!({"i": i}));
    }
    // Drain.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let count = std::fs::read_dir(&dropbox)
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
        if count == 0 {
            break;
        }
        if Instant::now() > deadline {
            panic!("dropbox never drained");
        }
        thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::Relaxed);
    handle.join().unwrap();

    let meta = store.read_meta().unwrap();
    let result = SessionReader::read(store.log_path()).unwrap();
    assert_eq!(meta.frame_count as usize, result.events.len());
    assert!(meta.frame_count >= 6); // 5 hooks + session_end marker

    // Now corrupt the log file so any code path that decompresses it would
    // fail. `list::collect` should still succeed (and report the cached
    // frame count) because it only reads meta.
    std::fs::write(store.log_path(), b"GARBAGE NOT ZSTD").unwrap();

    let summaries = redo::cli::list::collect(&root).expect("list collect");
    let s = summaries
        .iter()
        .find(|s| s.session_id == session_id)
        .expect("our session in the listing");
    assert_eq!(s.frame_count, meta.frame_count as usize);
}

#[test]
fn oversize_hook_payload_is_truncated_and_flagged() {
    use redo::format::{Event, MAX_INLINE_PAYLOAD};
    use redo::hook::{build_envelope, envelope_to_event};

    // Synthesise a JSON payload comfortably above the cap.
    let big_string: String = "a".repeat(MAX_INLINE_PAYLOAD + 1024);
    let payload = format!("{{\"big\":\"{big_string}\"}}");
    let mut cursor = std::io::Cursor::new(payload.as_bytes());

    let env = build_envelope("PostToolUse", &mut cursor).expect("build envelope");
    assert_eq!(env.truncated, Some(true));
    let original = env.truncated_original_size.expect("original size set");
    assert!(original > MAX_INLINE_PAYLOAD);
    // Body should have been dropped.
    assert!(env.payload.is_null());

    // Project to a Marker and confirm the truncated flag survives.
    let event = envelope_to_event(&env, 0);
    let extras = match &event {
        Event::Marker { extras, .. } => extras,
        _ => panic!("expected Marker"),
    };
    assert_eq!(
        extras.get("truncated").and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        extras
            .get("truncated_original_size")
            .and_then(|v| v.as_u64()),
        Some(original as u64)
    );

    // Canonical summary should say "(N bytes truncated)".
    let line = redo::format::canonicalize(&event);
    assert!(
        line.summary.contains("truncated"),
        "summary missing truncated marker: {}",
        line.summary
    );
}

#[test]
fn small_hook_payload_round_trips_unchanged() {
    use redo::hook::build_envelope;
    let body = b"{\"tool_name\":\"Bash\",\"command\":\"ls\"}";
    let mut cursor = std::io::Cursor::new(&body[..]);
    let env = build_envelope("PreToolUse", &mut cursor).unwrap();
    assert!(env.truncated.is_none());
    assert!(env.truncated_original_size.is_none());
    assert_eq!(env.payload["tool_name"], "Bash");
}
