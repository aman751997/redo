//! Coverage for the multi-event projection that turns hook envelopes into
//! Output / FileWrite side events alongside the catch-all Marker.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use redo::format::{canonicalize, Event, OutputStream, MAX_INLINE_PAYLOAD};
use redo::hook::{envelope_to_events, write_envelope, Envelope};
use redo::recorder::{self, Config};
use redo::store::{SessionReader, SessionStore};
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

fn drop_envelope(dropbox: &std::path::Path, kind: &str, payload: serde_json::Value) {
    let env = Envelope::now(kind, payload);
    write_envelope(dropbox, &env).expect("write envelope");
}

fn wait_for_drain(dropbox: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let count = std::fs::read_dir(dropbox)
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
}

#[test]
fn post_bash_projects_output_then_marker() {
    let env = Envelope::now(
        "PostToolUse",
        json!({
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_response": {"stdout": "total 0\nfoo\n", "stderr": ""}
        }),
    );
    let evs = envelope_to_events(&env, 0);
    assert_eq!(
        evs.len(),
        2,
        "Bash PostToolUse should yield Output + Marker"
    );

    match &evs[0] {
        Event::Output {
            seq,
            stream,
            extras,
            ..
        } => {
            assert_eq!(*seq, 0);
            assert_eq!(*stream, Some(OutputStream::Stdout));
            assert_eq!(
                extras.get("source").and_then(|v| v.as_str()),
                Some("bash"),
                "extras.source must be \"bash\""
            );
        }
        other => panic!("expected Output first, got {other:?}"),
    }

    match &evs[1] {
        Event::Marker { seq, label, .. } => {
            assert_eq!(*seq, 1);
            assert_eq!(label, "hook:PostToolUse");
        }
        other => panic!("expected Marker second, got {other:?}"),
    }

    // Canonical line for the Output frame surfaces source + stream + preview.
    let line = canonicalize(&evs[0]);
    assert_eq!(line.kind, "Output");
    assert!(
        line.summary.starts_with("bash stdout "),
        "expected `bash stdout` prefix, got: {}",
        line.summary
    );
    assert!(
        line.summary.contains("total 0"),
        "expected stdout preview, got: {}",
        line.summary
    );
}

#[test]
fn post_bash_falls_back_to_stderr_when_stdout_empty() {
    let env = Envelope::now(
        "PostToolUse",
        json!({
            "tool_name": "Bash",
            "tool_input": {"command": "false"},
            "tool_response": {"stdout": "", "stderr": "bash: false\n"}
        }),
    );
    let evs = envelope_to_events(&env, 0);
    assert_eq!(evs.len(), 2);
    if let Event::Output { stream, .. } = &evs[0] {
        assert_eq!(*stream, Some(OutputStream::Stderr));
    } else {
        panic!("expected Output");
    }
}

#[test]
fn post_bash_with_no_output_emits_marker_only() {
    let env = Envelope::now(
        "PostToolUse",
        json!({
            "tool_name": "Bash",
            "tool_input": {"command": "true"},
            "tool_response": {"stdout": "", "stderr": ""}
        }),
    );
    let evs = envelope_to_events(&env, 0);
    assert_eq!(evs.len(), 1);
    assert!(matches!(evs[0], Event::Marker { .. }));
}

#[test]
fn post_write_projects_file_write_then_marker() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("foo.rs");
    let content = b"pub fn main() { println!(\"hi\"); }\n";
    std::fs::write(&file_path, content).unwrap();
    let expected_hash = blake3::hash(content).to_hex().to_string();

    let env = Envelope::now(
        "PostToolUse",
        json!({
            "tool_name": "Write",
            "tool_input": {"file_path": file_path.display().to_string(), "content": "..."},
            "tool_response": {"filePath": file_path.display().to_string(), "type": "create"}
        }),
    );
    let evs = envelope_to_events(&env, 5);
    assert_eq!(evs.len(), 2);

    match &evs[0] {
        Event::FileWrite {
            seq,
            path,
            content_hash,
            size,
            inline_payload,
            truncated,
            ..
        } => {
            assert_eq!(*seq, 5);
            assert_eq!(path, &file_path.display().to_string());
            assert_eq!(content_hash, &expected_hash);
            assert_eq!(*size as usize, content.len());
            assert!(!*truncated);
            // Inline bytes round-trip via base64.
            use base64::engine::general_purpose::STANDARD as B64;
            use base64::Engine;
            let bytes = inline_payload
                .as_ref()
                .map(|s| B64.decode(s).unwrap())
                .expect("inline payload present");
            assert_eq!(bytes.as_slice(), content);
        }
        other => panic!("expected FileWrite, got {other:?}"),
    }

    if let Event::Marker { seq, label, .. } = &evs[1] {
        assert_eq!(*seq, 6);
        assert_eq!(label, "hook:PostToolUse");
    } else {
        panic!("expected Marker after FileWrite");
    }

    // Canonical: `file_write <path> <size>B (<hash[..8]>)`
    let line = canonicalize(&evs[0]);
    assert_eq!(line.kind, "file_write");
    let short_hash: String = expected_hash.chars().take(8).collect();
    let want = format!("{} {}B ({short_hash})", file_path.display(), content.len());
    assert_eq!(line.summary, want);
}

#[test]
fn file_write_truncates_oversized_files() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("big.bin");
    let big = vec![0xABu8; MAX_INLINE_PAYLOAD + 1024];
    std::fs::write(&file_path, &big).unwrap();

    let env = Envelope::now(
        "PostToolUse",
        json!({
            "tool_name": "Write",
            "tool_input": {"file_path": file_path.display().to_string()},
            "tool_response": {"filePath": file_path.display().to_string()}
        }),
    );
    let evs = envelope_to_events(&env, 0);
    if let Event::FileWrite {
        truncated,
        truncated_original_size,
        inline_payload,
        size,
        ..
    } = &evs[0]
    {
        assert!(*truncated);
        assert_eq!(truncated_original_size.unwrap(), big.len() as u64);
        assert!(inline_payload.is_none());
        assert_eq!(*size, big.len() as u64);
    } else {
        panic!("expected FileWrite first");
    }
}

#[test]
fn file_write_missing_file_emits_marker_only() {
    let env = Envelope::now(
        "PostToolUse",
        json!({
            "tool_name": "Write",
            "tool_input": {"file_path": "/this/path/definitely/does/not/exist/xyz"},
            "tool_response": {"filePath": "/this/path/definitely/does/not/exist/xyz"}
        }),
    );
    let evs = envelope_to_events(&env, 0);
    assert_eq!(evs.len(), 1, "no FileWrite when re-read fails");
    assert!(matches!(evs[0], Event::Marker { .. }));
}

#[test]
fn schema_drift_increments_counter_in_meta() {
    // Strip required `tool_response` from a PostToolUse and confirm that
    // the recorder still ingests it but bumps schema_drift_events.
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

    // Missing tool_response on PostToolUse → drift.
    drop_envelope(
        &dropbox,
        "PostToolUse",
        json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}),
    );
    wait_for_drain(&dropbox);

    stop.store(true, Ordering::Relaxed);
    handle.join().unwrap();

    let meta = store.read_meta().expect("meta");
    assert!(
        meta.schema_drift_events >= 1,
        "schema_drift_events should have fired, got {}",
        meta.schema_drift_events
    );
}

#[test]
fn well_formed_payload_does_not_increment_drift_counter() {
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

    drop_envelope(
        &dropbox,
        "PostToolUse",
        json!({
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_response": {"stdout": "ok\n", "stderr": ""}
        }),
    );
    wait_for_drain(&dropbox);

    stop.store(true, Ordering::Relaxed);
    handle.join().unwrap();

    let meta = store.read_meta().expect("meta");
    assert_eq!(
        meta.schema_drift_events, 0,
        "well-formed payload should not trip drift"
    );

    // Sanity: the log carries Output + Marker + session_end.
    let result = SessionReader::read(store.log_path()).unwrap();
    let kinds: Vec<&str> = result
        .events
        .iter()
        .map(|e| match e {
            Event::Output { .. } => "Output",
            Event::Marker { .. } => "Marker",
            Event::Input { .. } => "Input",
            Event::Resize { .. } => "Resize",
            Event::FileWrite { .. } => "FileWrite",
        })
        .collect();
    assert_eq!(kinds, vec!["Output", "Marker", "Marker"]);
}

#[test]
fn fixture_file_is_documented_and_loadable() {
    // Anchor: the fixture is the source of truth for the hook payload shapes
    // we depend on. Test fails loudly if it disappears or stops being valid
    // JSON, so a refactor that breaks the schema check has a cheap signal.
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hooks/claude_code_v1.json");
    let bytes = std::fs::read(&p).expect("fixture exists");
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("fixture is valid JSON");
    let obj = v.as_object().expect("fixture is a JSON object");
    for required in [
        "PreToolUse_Bash",
        "PostToolUse_Bash",
        "PostToolUse_Edit",
        "PostToolUse_Write",
        "PostToolUse_MultiEdit",
    ] {
        assert!(obj.contains_key(required), "fixture missing {required}");
    }
}

#[test]
fn fileformat_version_is_two() {
    assert_eq!(redo::format::FORMAT_VERSION, 2);
}
