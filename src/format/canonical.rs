//! Canonical-tuple summary of an `Event`.
//!
//! Both the CLI `redo diff` subcommand and the in-TUI side-by-side diff view
//! consume sequences of these tuples. A `CanonicalLine` is a stable, line-
//! oriented projection of an event — `(seq, kind, summary)` — that elides
//! payload bytes (which would dominate any reasonable diff) and surfaces the
//! parts a human cares about: tool names, marker labels, sizes.
//!
//! Rules:
//!
//! * `Output` collapses to `"<stream> <N> bytes [<preview>]"` when projected
//!   from a known stream (e.g. `bash <preview>`); legacy streamless captures
//!   keep the bare `"<N> bytes"` shape.
//! * `Input` collapses to `"<N> bytes"` (decoded length when available).
//! * `Resize` shows the new dimensions.
//! * `Marker` shows its label, with the hook `tool_name` from `extras.payload`
//!   appended when present so two runs with different tool calls diff cleanly.
//! * `FileWrite` shows the path, size, and the first 8 hex chars of the hash
//!   so a diff surfaces "same path, different content" at a glance.
//!
//! The summary is intentionally short: a diff of two long sessions still has
//! to fit on a terminal, and lines that are too noisy hide the real signal.

use std::fmt;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::Value;

use crate::format::{Event, OutputStream};

/// A canonical, line-oriented projection of an event used for diffs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalLine {
    pub seq: u64,
    pub kind: &'static str,
    pub summary: String,
}

impl CanonicalLine {
    /// Render as a single line for diff output.
    ///
    /// Format: `#<seq> <kind> <summary>`. Stable across releases; consumers
    /// (including diff golden tests) depend on it.
    pub fn render(&self) -> String {
        format!("#{:>5} {} {}", self.seq, self.kind, self.summary)
    }
}

impl fmt::Display for CanonicalLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

/// Project a single event onto its canonical line.
pub fn canonicalize(e: &Event) -> CanonicalLine {
    match e {
        Event::Output {
            seq,
            bytes,
            stream,
            truncated_original_size,
            extras,
            ..
        } => CanonicalLine {
            seq: *seq,
            kind: "Output",
            summary: output_summary(bytes, *stream, *truncated_original_size, extras),
        },
        Event::Input {
            seq,
            bytes,
            truncated_original_size,
            ..
        } => CanonicalLine {
            seq: *seq,
            kind: "Input",
            summary: bytes_summary(bytes, *truncated_original_size),
        },
        Event::Resize {
            seq, cols, rows, ..
        } => CanonicalLine {
            seq: *seq,
            kind: "Resize",
            summary: format!("{cols}x{rows}"),
        },
        Event::Marker {
            seq, label, extras, ..
        } => CanonicalLine {
            seq: *seq,
            kind: "Marker",
            summary: marker_summary(label, extras),
        },
        Event::FileWrite {
            seq,
            path,
            content_hash,
            size,
            truncated,
            ..
        } => CanonicalLine {
            seq: *seq,
            kind: "file_write",
            summary: file_write_summary(path, *size, content_hash, *truncated),
        },
    }
}

/// Project an entire event sequence to canonical lines.
pub fn canonicalize_all(events: &[Event]) -> Vec<CanonicalLine> {
    events.iter().map(canonicalize).collect()
}

fn bytes_summary(b64: &str, truncated_original: Option<usize>) -> String {
    if let Some(orig) = truncated_original {
        return format!("{orig} bytes (truncated)");
    }
    // Decode-length is the source of truth; fall back to b64 length if the
    // payload is malformed (treating "looks like binary noise" as still a
    // legitimate signal of size).
    let n = match B64.decode(b64) {
        Ok(v) => v.len(),
        Err(_) => b64.len(),
    };
    format!("{n} bytes")
}

fn output_summary(
    b64: &str,
    stream: Option<OutputStream>,
    truncated_original: Option<usize>,
    extras: &serde_json::Map<String, Value>,
) -> String {
    // Source tag (e.g. `bash`) comes from the projection that produced this
    // event; stream tag (`stdout`/`stderr`) from the variant field. When
    // neither is set we keep the legacy `"<N> bytes"` shape so older fixtures
    // and external consumers diff cleanly across the v1 → v2 boundary.
    let source_tag: Option<&str> = extras.get("source").and_then(|v| v.as_str());
    let stream_tag = match stream {
        Some(OutputStream::Stdout) => Some("stdout"),
        Some(OutputStream::Stderr) => Some("stderr"),
        None => None,
    };

    if source_tag.is_none() && stream_tag.is_none() {
        return bytes_summary(b64, truncated_original);
    }

    let mut out = String::new();
    if let Some(s) = source_tag {
        out.push_str(s);
        out.push(' ');
    }
    if let Some(s) = stream_tag {
        out.push_str(s);
        out.push(' ');
    }

    if let Some(orig) = truncated_original {
        out.push_str(&format!("{orig} bytes (truncated)"));
        return out;
    }

    let decoded: Vec<u8> = B64.decode(b64).unwrap_or_default();
    let n = if decoded.is_empty() && !b64.is_empty() {
        b64.len()
    } else {
        decoded.len()
    };
    let preview = preview_from_bytes(&decoded);
    if !preview.is_empty() {
        out.push_str(&format!("{preview} ({n} bytes)"));
    } else {
        out.push_str(&format!("{n} bytes"));
    }
    out
}
fn preview_from_bytes(bytes: &[u8]) -> String {
    // Take the first line of UTF-8; cap to ~48 chars; replace non-printable
    // bytes with `.` so binary output is still legible.
    if bytes.is_empty() {
        return String::new();
    }
    let s = String::from_utf8_lossy(bytes);
    let first_line = s.split(['\n', '\r']).next().unwrap_or("");
    let cleaned: String = first_line
        .chars()
        .map(|c| if c.is_control() { '.' } else { c })
        .collect();
    let mut iter = cleaned.chars();
    let mut out: String = iter.by_ref().take(48).collect();
    if iter.next().is_some() {
        out.push('…');
    }
    out
}

fn marker_summary(label: &str, extras: &serde_json::Map<String, Value>) -> String {
    // Try to surface the tool name from a hook payload, e.g.
    // `extras.payload.tool_name = "Bash"`. Keeps the label terse but
    // informative for diffs across runs.
    let tool = extras
        .get("payload")
        .and_then(|p| p.get("tool_name"))
        .and_then(|v| v.as_str());
    let truncated = extras
        .get("truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let original = extras
        .get("truncated_original_size")
        .and_then(|v| v.as_u64());
    let mut out = match tool {
        Some(t) => format!("{label} [{t}]"),
        None => label.to_string(),
    };
    if truncated {
        if let Some(n) = original {
            out.push_str(&format!(" ({n} bytes truncated)"));
        } else {
            out.push_str(" (truncated)");
        }
    }
    out
}

fn file_write_summary(path: &str, size: u64, content_hash: &str, truncated: bool) -> String {
    let short_hash: String = content_hash.chars().take(8).collect();
    let suffix = if truncated { " (truncated)" } else { "" };
    format!("{path} {size}B ({short_hash}){suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, Value};

    fn b64(bytes: &[u8]) -> String {
        B64.encode(bytes)
    }

    #[test]
    fn output_summary_uses_decoded_length() {
        let e = Event::Output {
            seq: 7,
            t_ns: 0,
            bytes: b64(b"hello world"),
            stream: None,
            truncated: None,
            truncated_original_size: None,
            extras: Map::new(),
        };
        let line = canonicalize(&e);
        assert_eq!(line.seq, 7);
        assert_eq!(line.kind, "Output");
        // Streamless legacy shape (no source/stream): bare byte count.
        assert_eq!(line.summary, "11 bytes");
    }

    #[test]
    fn truncated_summary_uses_original_size() {
        let e = Event::Output {
            seq: 1,
            t_ns: 0,
            bytes: b64(b"first 4 bytes only"),
            stream: None,
            truncated: Some(true),
            truncated_original_size: Some(1_048_576),
            extras: Map::new(),
        };
        assert_eq!(canonicalize(&e).summary, "1048576 bytes (truncated)");
    }

    #[test]
    fn output_summary_includes_source_and_stream_tags() {
        let mut extras = Map::new();
        extras.insert("source".into(), Value::String("bash".into()));
        let e = Event::Output {
            seq: 3,
            t_ns: 0,
            bytes: b64(b"total 0\nfoo\n"),
            stream: Some(OutputStream::Stdout),
            truncated: None,
            truncated_original_size: None,
            extras,
        };
        let s = canonicalize(&e).summary;
        assert!(s.starts_with("bash stdout "), "got: {s}");
        assert!(s.contains("total 0"), "got: {s}");
    }

    #[test]
    fn resize_summary_shows_dims() {
        let e = Event::Resize {
            seq: 0,
            t_ns: 0,
            cols: 120,
            rows: 40,
            extras: Map::new(),
        };
        assert_eq!(canonicalize(&e).summary, "120x40");
    }

    #[test]
    fn marker_summary_includes_tool_name_when_present() {
        let mut extras = Map::new();
        extras.insert(
            "payload".into(),
            serde_json::json!({"tool_name": "Bash", "command": "ls"}),
        );
        let e = Event::Marker {
            seq: 3,
            t_ns: 0,
            label: "hook:PreToolUse".into(),
            extras,
        };
        assert_eq!(canonicalize(&e).summary, "hook:PreToolUse [Bash]");
    }

    #[test]
    fn marker_summary_falls_back_to_label_when_no_tool() {
        let e = Event::Marker {
            seq: 3,
            t_ns: 0,
            label: "session_end".into(),
            extras: Map::new(),
        };
        assert_eq!(canonicalize(&e).summary, "session_end");
    }

    #[test]
    fn file_write_summary_shows_path_size_and_short_hash() {
        let e = Event::FileWrite {
            seq: 9,
            t_ns: 0,
            path: "/tmp/foo.rs".into(),
            content_hash: "abcdef0123456789aaaaaaaa".into(),
            size: 42,
            truncated: false,
            truncated_original_size: None,
            inline_payload: None,
            extras: Map::new(),
        };
        let line = canonicalize(&e);
        assert_eq!(line.kind, "file_write");
        assert_eq!(line.summary, "/tmp/foo.rs 42B (abcdef01)");
    }

    #[test]
    fn file_write_summary_marks_truncation() {
        let e = Event::FileWrite {
            seq: 1,
            t_ns: 0,
            path: "/tmp/big.bin".into(),
            content_hash: "deadbeef00000000".into(),
            size: 1_000_000,
            truncated: true,
            truncated_original_size: Some(1_000_000),
            inline_payload: None,
            extras: Map::new(),
        };
        let s = canonicalize(&e).summary;
        assert!(s.contains("(truncated)"), "got: {s}");
    }

    #[test]
    fn render_format_is_stable() {
        let line = CanonicalLine {
            seq: 42,
            kind: "Marker",
            summary: "session_end".into(),
        };
        assert_eq!(line.render(), "#   42 Marker session_end");
    }

    #[test]
    fn canonicalize_all_preserves_order() {
        let evs = vec![
            Event::Marker {
                seq: 0,
                t_ns: 0,
                label: "a".into(),
                extras: Map::new(),
            },
            Event::Marker {
                seq: 1,
                t_ns: 0,
                label: "b".into(),
                extras: Map::new(),
            },
        ];
        let lines = canonicalize_all(&evs);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].summary, "a");
        assert_eq!(lines[1].summary, "b");
        let _ = Value::Null;
    }
}
