//! Group consecutive related frames into spans.
//!
//! A span is a contiguous run of frames that share a coarse "topic":
//!
//! * **ToolCall** — opens on a `Marker` whose label starts with `hook:PreToolUse`
//!   (or contains `tool-call`), runs until the matching `hook:PostToolUse` (or
//!   `tool-result`) marker. If no closing marker is found the span ends with
//!   the last frame.
//! * **ModelStream** — coalesces a contiguous run of `Output` frames.
//! * **InputStream** — coalesces a contiguous run of `Input` frames.
//! * **Resize** — singleton.
//! * **Marker** — singleton for any marker not matched as a tool boundary.
//!
//! The grouping is driven only by event order and labels; it never recurses or
//! buffers across the whole session, so it's O(n) and stable under truncation.

use crate::format::Event;

/// What kind of span this is. Drives both the scrub-bar tick colour and the
/// `shift-J` / `shift-K` jump targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    ToolCall,
    ModelStream,
    InputStream,
    Resize,
    Marker,
}

impl SpanKind {
    pub fn label(self) -> &'static str {
        match self {
            SpanKind::ToolCall => "tool",
            SpanKind::ModelStream => "out",
            SpanKind::InputStream => "in",
            SpanKind::Resize => "rsz",
            SpanKind::Marker => "mark",
        }
    }
}

/// Half-open `[start, end)` window over the event slice this span was built
/// from, plus a one-line summary suitable for status bars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub kind: SpanKind,
    pub start: usize,
    pub end: usize,
    pub summary: String,
}

impl Span {
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Does this span cover the given event index?
    pub fn contains(&self, idx: usize) -> bool {
        idx >= self.start && idx < self.end
    }
}

/// Group `events` into spans. Empty input yields an empty list.
pub fn group(events: &[Event]) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut i = 0;
    while i < events.len() {
        match &events[i] {
            Event::Output { .. } => {
                let start = i;
                while i < events.len() && matches!(events[i], Event::Output { .. }) {
                    i += 1;
                }
                spans.push(Span {
                    kind: SpanKind::ModelStream,
                    start,
                    end: i,
                    summary: format!("model output ({} frames)", i - start),
                });
            }
            Event::Input { .. } => {
                let start = i;
                while i < events.len() && matches!(events[i], Event::Input { .. }) {
                    i += 1;
                }
                spans.push(Span {
                    kind: SpanKind::InputStream,
                    start,
                    end: i,
                    summary: format!("user input ({} frames)", i - start),
                });
            }
            Event::Resize { cols, rows, .. } => {
                spans.push(Span {
                    kind: SpanKind::Resize,
                    start: i,
                    end: i + 1,
                    summary: format!("resize {cols}x{rows}"),
                });
                i += 1;
            }
            Event::Marker { label, .. } => {
                if is_tool_open(label) {
                    let start = i;
                    let open_label = label.clone();
                    i += 1;
                    while i < events.len() {
                        if let Event::Marker { label, .. } = &events[i] {
                            if is_tool_close(label) {
                                i += 1;
                                break;
                            }
                            // A second open before a close starts a new span.
                            if is_tool_open(label) {
                                break;
                            }
                        }
                        i += 1;
                    }
                    spans.push(Span {
                        kind: SpanKind::ToolCall,
                        start,
                        end: i,
                        summary: tool_summary(&open_label, &events[start..i]),
                    });
                } else {
                    spans.push(Span {
                        kind: SpanKind::Marker,
                        start: i,
                        end: i + 1,
                        summary: format!("marker {label}"),
                    });
                    i += 1;
                }
            }
        }
    }
    spans
}

fn is_tool_open(label: &str) -> bool {
    label.starts_with("hook:PreToolUse") || label.contains("tool-call")
}

fn is_tool_close(label: &str) -> bool {
    label.starts_with("hook:PostToolUse") || label.contains("tool-result")
}

fn tool_summary(open_label: &str, slice: &[Event]) -> String {
    // Prefer the tool name from the opening marker's payload extras.
    let tool = slice.first().and_then(|e| match e {
        Event::Marker { extras, .. } => extras
            .get("payload")
            .and_then(|p| p.get("tool_name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    });
    match tool {
        Some(t) => format!("tool-call [{t}] ({} frames)", slice.len()),
        None => format!("tool-call ({} frames) [{open_label}]", slice.len()),
    }
}

/// Index of the span that contains `frame_idx`, or `None` if out of range.
pub fn span_index_for_frame(spans: &[Span], frame_idx: usize) -> Option<usize> {
    spans.iter().position(|s| s.contains(frame_idx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};

    fn marker(seq: u64, label: &str) -> Event {
        Event::Marker {
            seq,
            t_ns: seq * 1_000_000,
            label: label.into(),
            extras: Map::new(),
        }
    }

    fn marker_with_tool(seq: u64, label: &str, tool: &str) -> Event {
        let mut extras = Map::new();
        extras.insert("payload".into(), json!({"tool_name": tool}));
        Event::Marker {
            seq,
            t_ns: seq * 1_000_000,
            label: label.into(),
            extras,
        }
    }

    fn output(seq: u64) -> Event {
        Event::Output {
            seq,
            t_ns: seq * 1_000_000,
            bytes: "aGk=".into(),
            truncated: None,
            truncated_original_size: None,
            extras: Map::new(),
        }
    }

    fn input(seq: u64) -> Event {
        Event::Input {
            seq,
            t_ns: seq * 1_000_000,
            bytes: "aGk=".into(),
            truncated: None,
            truncated_original_size: None,
            extras: Map::new(),
        }
    }

    fn resize(seq: u64) -> Event {
        Event::Resize {
            seq,
            t_ns: 0,
            cols: 80,
            rows: 24,
            extras: Map::new(),
        }
    }

    #[test]
    fn empty_input_yields_no_spans() {
        let s = group(&[]);
        assert!(s.is_empty());
    }

    #[test]
    fn coalesces_consecutive_output_into_one_span() {
        let evs = vec![output(0), output(1), output(2)];
        let s = group(&evs);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, SpanKind::ModelStream);
        assert_eq!(s[0].start, 0);
        assert_eq!(s[0].end, 3);
        assert_eq!(s[0].len(), 3);
    }

    #[test]
    fn separates_output_from_input() {
        let evs = vec![output(0), output(1), input(2), output(3)];
        let s = group(&evs);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].kind, SpanKind::ModelStream);
        assert_eq!(s[1].kind, SpanKind::InputStream);
        assert_eq!(s[2].kind, SpanKind::ModelStream);
    }

    #[test]
    fn tool_call_span_runs_from_pre_to_post() {
        let evs = vec![
            marker_with_tool(0, "hook:PreToolUse", "Bash"),
            output(1),
            output(2),
            marker(3, "hook:PostToolUse"),
            marker(4, "session_end"),
        ];
        let s = group(&evs);
        assert_eq!(s.len(), 2, "tool-call + closing session_end marker");
        assert_eq!(s[0].kind, SpanKind::ToolCall);
        assert_eq!(s[0].start, 0);
        assert_eq!(s[0].end, 4);
        assert!(s[0].summary.contains("Bash"));
        assert_eq!(s[1].kind, SpanKind::Marker);
    }

    #[test]
    fn tool_call_without_close_runs_to_end() {
        let evs = vec![marker_with_tool(0, "hook:PreToolUse", "Read"), output(1)];
        let s = group(&evs);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, SpanKind::ToolCall);
        assert_eq!(s[0].end, 2);
    }

    #[test]
    fn standalone_marker_is_singleton_span() {
        let evs = vec![marker(0, "session_end")];
        let s = group(&evs);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, SpanKind::Marker);
        assert_eq!(s[0].len(), 1);
    }

    #[test]
    fn resize_is_singleton_span() {
        let evs = vec![output(0), resize(1), output(2)];
        let s = group(&evs);
        assert_eq!(s.len(), 3);
        assert_eq!(s[1].kind, SpanKind::Resize);
    }

    #[test]
    fn span_index_for_frame_finds_containing_span() {
        let evs = vec![output(0), output(1), input(2)];
        let s = group(&evs);
        assert_eq!(span_index_for_frame(&s, 0), Some(0));
        assert_eq!(span_index_for_frame(&s, 1), Some(0));
        assert_eq!(span_index_for_frame(&s, 2), Some(1));
        assert_eq!(span_index_for_frame(&s, 99), None);
    }

    #[test]
    fn second_open_without_close_starts_a_new_span() {
        let evs = vec![
            marker_with_tool(0, "hook:PreToolUse", "Bash"),
            output(1),
            marker_with_tool(2, "hook:PreToolUse", "Read"),
            output(3),
        ];
        let s = group(&evs);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].kind, SpanKind::ToolCall);
        assert_eq!(s[1].kind, SpanKind::ToolCall);
        assert_eq!(s[0].start, 0);
        assert_eq!(s[0].end, 2);
        assert_eq!(s[1].start, 2);
        assert_eq!(s[1].end, 4);
    }
}
