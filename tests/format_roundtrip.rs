//! Table-driven roundtrip tests for the on-disk log format.
//!
//! Each case asserts: parse a JSON line into the typed value, re-serialise it,
//! parse again, and compare the two typed values for equality. This catches
//! both lossy serialisation and asymmetric default handling.

use redo::format::{Event, SessionHeader, TermSize};
use serde_json::{json, Map, Value};

/// Roundtrip a typed value through JSON and assert it survives unchanged.
fn assert_roundtrip<T>(value: &T)
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de> + std::fmt::Debug + PartialEq,
{
    let s = serde_json::to_string(value).expect("serialize");
    let back: T = serde_json::from_str(&s).expect("deserialize");
    assert_eq!(value, &back, "roundtrip mismatch via {s}");
}

#[test]
fn header_roundtrip() {
    let h = SessionHeader {
        version: 1,
        format: "redo".into(),
        session_id: uuid::Uuid::now_v7(),
        created_at: "2026-04-29T12:34:56.789Z".into(),
        cmd: vec!["claude".into(), "--resume".into()],
        env_term: TermSize {
            cols: 120,
            rows: 40,
        },
        claude_version: Some("1.0.123".into()),
        redo_version: "0.0.0".into(),
        cwd: "/home/user/project".into(),
    };
    assert_roundtrip(&h);
}

#[test]
fn header_omits_optional_when_absent() {
    let h = SessionHeader {
        version: 1,
        format: "redo".into(),
        session_id: uuid::Uuid::nil(),
        created_at: "2026-04-29T00:00:00Z".into(),
        cmd: vec![],
        env_term: TermSize { cols: 80, rows: 24 },
        claude_version: None,
        redo_version: "0.0.0".into(),
        cwd: "/".into(),
    };
    let s = serde_json::to_string(&h).unwrap();
    assert!(
        !s.contains("claude_version"),
        "absent optional must be omitted, got: {s}"
    );
}

#[test]
fn output_event_roundtrip() {
    let e = Event::Output {
        seq: 0,
        t_ns: 0,
        bytes: "aGVsbG8=".into(),
        stream: None,
        truncated: None,
        truncated_original_size: None,
        extras: Map::new(),
    };
    assert_roundtrip(&e);
}

#[test]
fn output_truncated_roundtrip() {
    let e = Event::Output {
        seq: 42,
        t_ns: 1_700_000_000_000_000_000,
        bytes: "x".repeat(256 * 1024),
        stream: None,
        truncated: Some(true),
        truncated_original_size: Some(1_048_576),
        extras: Map::new(),
    };
    assert_roundtrip(&e);
}

#[test]
fn output_omits_truncation_fields_when_absent() {
    let e = Event::Output {
        seq: 0,
        t_ns: 0,
        bytes: "aGVsbG8=".into(),
        stream: None,
        truncated: None,
        truncated_original_size: None,
        extras: Map::new(),
    };
    let s = serde_json::to_string(&e).unwrap();
    assert!(!s.contains("truncated"), "got: {s}");
    assert!(!s.contains("truncated_original_size"), "got: {s}");
}

#[test]
fn input_event_roundtrip() {
    let e = Event::Input {
        seq: 7,
        t_ns: 100,
        bytes: "Y29tbWFuZA==".into(),
        truncated: None,
        truncated_original_size: None,
        extras: Map::new(),
    };
    assert_roundtrip(&e);
}

#[test]
fn resize_event_roundtrip() {
    let e = Event::Resize {
        seq: 3,
        t_ns: 999,
        cols: 100,
        rows: 30,
        extras: Map::new(),
    };
    assert_roundtrip(&e);
}

#[test]
fn marker_event_roundtrip() {
    for label in ["interrupt", "sigterm", "force_kill"] {
        let e = Event::Marker {
            seq: 1,
            t_ns: 500,
            label: label.into(),
            extras: Map::new(),
        };
        assert_roundtrip(&e);
    }
}

#[test]
fn event_serializes_flat_with_kind_tag() {
    let e = Event::Resize {
        seq: 0,
        t_ns: 0,
        cols: 100,
        rows: 30,
        extras: Map::new(),
    };
    let v: Value = serde_json::to_value(&e).unwrap();
    let obj = v
        .as_object()
        .expect("event must serialize as a JSON object");

    assert_eq!(obj.get("kind"), Some(&json!("Resize")));
    assert_eq!(obj.get("seq"), Some(&json!(0)));
    assert_eq!(obj.get("t_ns"), Some(&json!(0)));
    assert_eq!(obj.get("cols"), Some(&json!(100)));
    assert_eq!(obj.get("rows"), Some(&json!(30)));
    assert!(obj.get("body").is_none(), "body must be flattened");
}

#[test]
fn extras_preserves_unknown_fields_through_roundtrip() {
    let raw = json!({
        "kind": "Resize",
        "seq": 5,
        "t_ns": 12345,
        "cols": 80,
        "rows": 24,
        "future_field": "hello",
        "another_unknown": { "nested": [1, 2, 3] }
    });

    let e: Event = serde_json::from_value(raw.clone()).expect("parse");
    let back = serde_json::to_value(&e).unwrap();

    assert_eq!(back.get("future_field"), raw.get("future_field"));
    assert_eq!(back.get("another_unknown"), raw.get("another_unknown"));
}

#[test]
fn event_parses_without_extras_field() {
    let raw = json!({
        "kind": "Resize",
        "seq": 1,
        "t_ns": 1000,
        "cols": 80,
        "rows": 24,
    });
    let e: Event = serde_json::from_value(raw).unwrap();
    if let Event::Resize { extras, .. } = e {
        assert!(extras.is_empty());
    } else {
        panic!("expected Resize variant");
    }
}

#[test]
fn event_accessors_expose_seq_and_t_ns() {
    let e = Event::Marker {
        seq: 99,
        t_ns: 1234,
        label: "interrupt".into(),
        extras: Map::new(),
    };
    assert_eq!(e.seq(), 99);
    assert_eq!(e.t_ns(), 1234);
}
