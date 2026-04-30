//! In-TUI side-by-side diff view.
//!
//! Loaded by pressing `d` in the main view: prompts for a peer session id,
//! then opens a two-column layout where each row pairs a canonical line from
//! session A with the aligned line from session B (or a blank when one side
//! has no counterpart). Differing rows are highlighted.
//!
//! Keys: `j`/`k` step, `J`/`K` page, `g`/`G` first/last, `q` return.

use std::path::Path;

use anyhow::{Context, Result};
use crossterm::event::{self, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TextSpan};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use similar::{ChangeTag, TextDiff};
use uuid::Uuid;

use crate::format::canonicalize_all;
use crate::store::{SessionReader, SessionStore};

/// One aligned row of the side-by-side view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlignedRow {
    pub left: Option<String>,
    pub right: Option<String>,
    pub differs: bool,
}

/// Produce aligned rows from two sequences of canonical lines.
///
/// Pure function so we can unit-test alignment without a TTY. Pairs equal
/// regions one-to-one; insertions show up as `(None, Some(_))`, deletions
/// as `(Some(_), None)`.
pub fn align(left: &[String], right: &[String]) -> Vec<AlignedRow> {
    let l = join(left);
    let r = join(right);
    let diff = TextDiff::from_lines(&l, &r);
    let mut rows: Vec<AlignedRow> = Vec::new();
    // Track buffered deletes so we can pair them with subsequent inserts on
    // the same hunk; this gives a more readable side-by-side than strict
    // sequential listing.
    let mut pending_dels: Vec<String> = Vec::new();
    let mut pending_ins: Vec<String> = Vec::new();
    let flush = |dels: &mut Vec<String>, ins: &mut Vec<String>, rows: &mut Vec<AlignedRow>| {
        let n = dels.len().max(ins.len());
        for i in 0..n {
            rows.push(AlignedRow {
                left: dels.get(i).cloned(),
                right: ins.get(i).cloned(),
                differs: true,
            });
        }
        dels.clear();
        ins.clear();
    };
    for change in diff.iter_all_changes() {
        let v = change.value().trim_end_matches('\n').to_string();
        match change.tag() {
            ChangeTag::Equal => {
                flush(&mut pending_dels, &mut pending_ins, &mut rows);
                rows.push(AlignedRow {
                    left: Some(v.clone()),
                    right: Some(v),
                    differs: false,
                });
            }
            ChangeTag::Delete => pending_dels.push(v),
            ChangeTag::Insert => pending_ins.push(v),
        }
    }
    flush(&mut pending_dels, &mut pending_ins, &mut rows);
    rows
}

fn join(v: &[String]) -> String {
    let mut s = String::with_capacity(v.iter().map(|x| x.len() + 1).sum());
    for line in v {
        s.push_str(line);
        s.push('\n');
    }
    s
}

/// Open the side-by-side diff view inside the active terminal. Caller is
/// responsible for setting up / tearing down raw mode + alt screen.
pub fn run<B>(terminal: &mut ratatui::Terminal<B>, root: &Path, a: Uuid, b: Uuid) -> Result<()>
where
    B: ratatui::backend::Backend,
{
    let rows = load_aligned(root, a, b)?;
    let mut cursor: usize = 0;
    let mut list_state = ListState::default();
    if !rows.is_empty() {
        list_state.select(Some(0));
    }
    loop {
        terminal.draw(|f| draw(f, &rows, &mut list_state, &a, &b))?;
        if event::poll(std::time::Duration::from_millis(200))? {
            if let event::Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Char('j') | KeyCode::Down if cursor + 1 < rows.len() => {
                        cursor += 1;
                        list_state.select(Some(cursor));
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        cursor = cursor.saturating_sub(1);
                        list_state.select(Some(cursor));
                    }
                    KeyCode::Char('J') => {
                        cursor = (cursor + 10).min(rows.len().saturating_sub(1));
                        list_state.select(Some(cursor));
                    }
                    KeyCode::Char('K') => {
                        cursor = cursor.saturating_sub(10);
                        list_state.select(Some(cursor));
                    }
                    KeyCode::Char('g') | KeyCode::Home => {
                        cursor = 0;
                        list_state.select(Some(0));
                    }
                    KeyCode::Char('G') | KeyCode::End => {
                        cursor = rows.len().saturating_sub(1);
                        list_state.select(Some(cursor));
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn load_aligned(root: &Path, a: Uuid, b: Uuid) -> Result<Vec<AlignedRow>> {
    let lines_a = read_canonical(root, a).context("read session a")?;
    let lines_b = read_canonical(root, b).context("read session b")?;
    Ok(align(&lines_a, &lines_b))
}

fn read_canonical(root: &Path, id: Uuid) -> Result<Vec<String>> {
    let store = SessionStore::new(root, id);
    let res = SessionReader::read(store.log_path())
        .with_context(|| format!("read log for session {id}"))?;
    Ok(canonicalize_all(&res.events)
        .into_iter()
        .map(|c| c.render())
        .collect())
}

fn draw(
    f: &mut ratatui::Frame,
    rows: &[AlignedRow],
    list_state: &mut ListState,
    a: &Uuid,
    b: &Uuid,
) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(f.area());

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(outer[0]);

    // Build per-column ListItems. We keep the cursor in a single ListState so
    // both columns scroll in lockstep.
    let left_items: Vec<ListItem> = rows.iter().map(|r| row_item(r, true)).collect();
    let right_items: Vec<ListItem> = rows.iter().map(|r| row_item(r, false)).collect();

    let title_left = format!("a  {a}");
    let title_right = format!("b  {b}");

    let left = List::new(left_items)
        .block(Block::default().borders(Borders::ALL).title(title_left))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    let right = List::new(right_items)
        .block(Block::default().borders(Borders::ALL).title(title_right))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    f.render_stateful_widget(left, cols[0], list_state);
    // Mirror selection state so both columns stay aligned.
    let mut right_state = list_state.clone();
    f.render_stateful_widget(right, cols[1], &mut right_state);

    let status = Paragraph::new("j/k step  J/K x10  g/G first/last  q return".to_string());
    f.render_widget(status, outer[1]);
}

fn row_item(row: &AlignedRow, left_side: bool) -> ListItem<'static> {
    let value = if left_side { &row.left } else { &row.right };
    let glyph_color = if row.differs {
        Color::Red
    } else {
        Color::Reset
    };
    let glyph = if !row.differs {
        " "
    } else if value.is_some() && (left_side ^ row.left.is_none()) {
        if left_side {
            "-"
        } else {
            "+"
        }
    } else {
        "~"
    };
    let body = value.clone().unwrap_or_default();
    let style = if row.differs {
        Style::default().fg(glyph_color)
    } else {
        Style::default()
    };
    ListItem::new(Line::from(vec![
        TextSpan::styled(format!("{glyph} "), style),
        TextSpan::raw(body),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_inputs_produce_equal_aligned_rows() {
        let a = vec!["x".to_string(), "y".into(), "z".into()];
        let rows = align(&a, &a);
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert!(!r.differs);
            assert_eq!(r.left, r.right);
        }
    }

    #[test]
    fn insertion_pairs_blank_left_with_inserted_right() {
        let a = vec!["x".to_string()];
        let b = vec!["x".to_string(), "y".into()];
        let rows = align(&a, &b);
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].differs);
        assert_eq!(rows[1].left, None);
        assert_eq!(rows[1].right, Some("y".into()));
        assert!(rows[1].differs);
    }

    #[test]
    fn modification_pairs_left_and_right() {
        let a = vec!["a".to_string()];
        let b = vec!["b".to_string()];
        let rows = align(&a, &b);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].left, Some("a".into()));
        assert_eq!(rows[0].right, Some("b".into()));
        assert!(rows[0].differs);
    }
}
