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
//! * `Output` / `Input` collapse to `"<N> bytes"` (decoded length when
//!   available, base64-encoded length otherwise — never the raw bytes).
//! * `Resize` shows the new dimensions.
//! * `Marker` shows its label, with the hook `tool_name` from `extras.payload`
//!   appended when present so two runs with different tool calls diff cleanly.
//!
//! The summary is intentionally short: a diff of two long sessions still has
//! to fit on a terminal, and lines that are too noisy hide the real signal.

use std::fmt;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::Value;

use crate::format::Event;

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
            truncated_original_size,
            ..
        } => CanonicalLine {
            seq: *seq,
            kind: "Output",
            summary: bytes_summary(bytes, *truncated_original_size),
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
            truncated: None,
            truncated_original_size: None,
            extras: Map::new(),
        };
        let line = canonicalize(&e);
        assert_eq!(line.seq, 7);
        assert_eq!(line.kind, "Output");
        assert_eq!(line.summary, "11 bytes");
    }

    #[test]
    fn truncated_summary_uses_original_size() {
        let e = Event::Output {
            seq: 1,
            t_ns: 0,
            bytes: b64(b"first 4 bytes only"),
            truncated: Some(true),
            truncated_original_size: Some(1_048_576),
            extras: Map::new(),
        };
        assert_eq!(canonicalize(&e).summary, "1048576 bytes (truncated)");
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
