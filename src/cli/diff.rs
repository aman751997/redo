//! `redo diff` — text-level diff of two sessions' canonical-line projections.
//!
//! Myers diff via the `similar` crate over canonical-line projections.
//!
//! Each session is rendered as a sequence of lines via [`crate::format::canonicalize_all`].
//! That projection is line-oriented, payload-free, and stable, so the diff
//! shows what *changed structurally* between two runs (different tool calls,
//! a marker that appeared on one run but not the other, ...) rather than the
//! ASCII noise of two compressed streams.
//!
//! Output is unified-diff style with a configurable context window. Colour
//! is enabled when stdout is a TTY and neither `--no-color` nor `NO_COLOR=1`
//! is set; the colour decision is made by the caller and passed in.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use similar::{ChangeTag, TextDiff};
use uuid::Uuid;

use crate::format::{canonicalize_all, CanonicalLine};
use crate::store::{SessionReader, SessionStore};

/// Run `redo diff` against the configured root. `use_color` is the resolved
/// colour decision (caller already merged `--no-color` and `NO_COLOR`).
pub fn run(root: &Path, a: Uuid, b: Uuid, context: usize, use_color: bool) -> Result<()> {
    let lines_a = read_canonical(root, a).context("read session a")?;
    let lines_b = read_canonical(root, b).context("read session b")?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    render_unified(&mut out, &lines_a, &lines_b, &a, &b, context, use_color)?;
    Ok(())
}

fn read_canonical(root: &Path, id: Uuid) -> Result<Vec<String>> {
    let store = SessionStore::new(root, id);
    let res = SessionReader::read(store.log_path())
        .with_context(|| format!("read log for session {id}"))?;
    Ok(canonicalize_all(&res.events)
        .into_iter()
        .map(|c: CanonicalLine| c.render())
        .collect())
}

/// Pure renderer: produce unified-diff output for two lists of canonical lines.
/// Public so tests can drive it without spinning up a session on disk.
pub fn render_unified(
    out: &mut dyn Write,
    a: &[String],
    b: &[String],
    label_a: &impl std::fmt::Display,
    label_b: &impl std::fmt::Display,
    context: usize,
    use_color: bool,
) -> Result<()> {
    let a_text = join_lines(a);
    let b_text = join_lines(b);
    let diff = TextDiff::from_lines(&a_text, &b_text);

    writeln!(
        out,
        "{}",
        with_color(use_color, "1", &format!("--- {label_a}"))
    )?;
    writeln!(
        out,
        "{}",
        with_color(use_color, "1", &format!("+++ {label_b}"))
    )?;

    for hunk in diff.unified_diff().context_radius(context).iter_hunks() {
        // similar's UnifiedHunkHeader implements Display.
        writeln!(
            out,
            "{}",
            with_color(use_color, "36", &format!("{}", hunk.header()))
        )?;
        for change in hunk.iter_changes() {
            let tag = change.tag();
            let glyph = match tag {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            // similar's value for line-based diffs already includes the
            // trailing newline; trim it so we own the line terminator.
            let value = change.value();
            let value = value.trim_end_matches('\n');
            let color_code = match tag {
                ChangeTag::Delete => "31",
                ChangeTag::Insert => "32",
                ChangeTag::Equal => "",
            };
            let line = format!("{glyph}{value}");
            if use_color && !color_code.is_empty() {
                writeln!(out, "{}", with_color(true, color_code, &line))?;
            } else {
                writeln!(out, "{line}")?;
            }
        }
    }
    Ok(())
}

fn join_lines(v: &[String]) -> String {
    let mut out = String::with_capacity(v.iter().map(|s| s.len() + 1).sum());
    for line in v {
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn with_color(use_color: bool, code: &str, body: &str) -> String {
    if !use_color || code.is_empty() {
        body.to_string()
    } else {
        format!("\x1b[{code}m{body}\x1b[0m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_difference_emits_only_headers() {
        let lines = vec!["#    0 Marker hello".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        render_unified(&mut buf, &lines, &lines, &"a", &"b", 3, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("--- a\n+++ b\n"));
        // No hunks means no `@@` lines.
        assert!(!s.contains("@@"));
    }

    #[test]
    fn detects_inserted_line_with_no_color() {
        let a = vec!["#    0 Marker hello".to_string()];
        let b = vec![
            "#    0 Marker hello".to_string(),
            "#    1 Marker world".to_string(),
        ];
        let mut buf: Vec<u8> = Vec::new();
        render_unified(&mut buf, &a, &b, &"a", &"b", 3, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("--- a"));
        assert!(s.contains("+++ b"));
        assert!(s.contains("@@"));
        assert!(s.contains("+#    1 Marker world"));
        // No ANSI escapes when colour disabled.
        assert!(
            !s.contains("\x1b["),
            "no_color must suppress escapes: {s:?}"
        );
    }

    #[test]
    fn detects_deletion_and_modification() {
        let a = vec![
            "#    0 Marker a".to_string(),
            "#    1 Marker b".to_string(),
            "#    2 Marker c".to_string(),
        ];
        let b = vec!["#    0 Marker a".to_string(), "#    1 Marker B".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        render_unified(&mut buf, &a, &b, &"a", &"b", 1, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("-#    1 Marker b"), "missing delete: {s}");
        assert!(s.contains("+#    1 Marker B"), "missing insert: {s}");
        assert!(
            s.contains("-#    2 Marker c"),
            "missing trailing delete: {s}"
        );
    }

    #[test]
    fn color_emits_ansi_escapes_when_enabled() {
        let a = vec!["#    0 Marker a".to_string()];
        let b = vec!["#    0 Marker b".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        render_unified(&mut buf, &a, &b, &"a", &"b", 1, true).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // The minus line should be wrapped in red (31).
        assert!(
            s.contains("\x1b[31m-#    0 Marker a\x1b[0m"),
            "missing red: {s:?}"
        );
        assert!(
            s.contains("\x1b[32m+#    0 Marker b\x1b[0m"),
            "missing green: {s:?}"
        );
    }
}
