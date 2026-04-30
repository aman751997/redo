//! TUI replay viewer.
//!
//! Three panes:
//! * left: timeline (list of events).
//! * middle: detail of the event under the cursor.
//! * right: filesystem placeholder — CoW snapshots ship later.
//!
//! A scrub bar at the bottom shows the current frame's position in the
//! session and the boundaries of the spans the events have been grouped into
//! (see [`spans::group`]).
//!
//! Keys: `j`/`k` step one frame, `J`/`K` jump to next/prev span boundary,
//! `g`/`G` (and `Home`/`End`) jump to first/last frame, `0`-`9` jump to that
//! decile, `q` quit, `/` enter filter mode (substring match on event
//! kind/label).
//!
//! Crossterm raw mode + alt screen. The panic hook restores the terminal on
//! abort so a crashing TUI does not leave the user's shell wedged.

use std::io::{self, Stdout};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span as TextSpan};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;

use crate::format::Event;
use crate::store::{SessionReader, SessionStore};

pub mod diff_view;
pub mod spans;

use spans::{Span, SpanKind};

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Action requested by the user when the TUI exits. The caller turns these
/// into CLI side-effects (printing a new session id, opening a diff, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitAction {
    /// User pressed `q` / Ctrl-C; nothing further to do.
    None,
    /// User pressed `f` at frame `at` of session `parent`; the caller should
    /// run `redo fork`.
    ForkAt { parent: uuid::Uuid, at: u64 },
    /// User pressed `d` and entered a peer session id; the caller should
    /// run a diff.
    Diff { peer: String },
}

struct App {
    events: Vec<Event>,
    spans: Vec<Span>,
    /// Indexes into `events`, after the filter is applied.
    visible: Vec<usize>,
    list_state: ListState,
    cursor: usize,
    filter: String,
    filter_editing: bool,
    /// Single-line input buffer for the `d` peer-session prompt.
    peer_prompt: Option<String>,
    quit: bool,
    parent_session: uuid::Uuid,
    action: ExitAction,
}

impl App {
    fn new(events: Vec<Event>) -> Self {
        let spans = spans::group(&events);
        let visible: Vec<usize> = (0..events.len()).collect();
        let mut list_state = ListState::default();
        if !visible.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            events,
            spans,
            visible,
            list_state,
            cursor: 0,
            filter: String::new(),
            filter_editing: false,
            peer_prompt: None,
            quit: false,
            parent_session: uuid::Uuid::nil(),
            action: ExitAction::None,
        }
    }

    fn recompute_filter(&mut self) {
        if self.filter.is_empty() {
            self.visible = (0..self.events.len()).collect();
        } else {
            let needle = self.filter.to_lowercase();
            self.visible = self
                .events
                .iter()
                .enumerate()
                .filter(|(_, e)| event_filter_string(e).to_lowercase().contains(&needle))
                .map(|(i, _)| i)
                .collect();
        }
        if self.visible.is_empty() {
            self.cursor = 0;
            self.list_state.select(None);
        } else {
            self.cursor = self.cursor.min(self.visible.len().saturating_sub(1));
            self.list_state.select(Some(self.cursor));
        }
    }

    fn step(&mut self, delta: i64) {
        if self.visible.is_empty() {
            return;
        }
        let max = self.visible.len() - 1;
        let next = (self.cursor as i64 + delta).clamp(0, max as i64) as usize;
        self.cursor = next;
        self.list_state.select(Some(next));
    }

    fn jump_first(&mut self) {
        if !self.visible.is_empty() {
            self.cursor = 0;
            self.list_state.select(Some(0));
        }
    }

    fn jump_last(&mut self) {
        if !self.visible.is_empty() {
            self.cursor = self.visible.len() - 1;
            self.list_state.select(Some(self.cursor));
        }
    }

    /// Move to the start of the next span boundary at or after the cursor.
    /// Operates on the *underlying* event index so it works correctly with an
    /// active filter as well: we map the visible cursor → event index, find
    /// the next span boundary, then map back to the closest visible row.
    fn jump_next_span(&mut self) {
        if self.visible.is_empty() || self.spans.is_empty() {
            return;
        }
        let here = self.visible[self.cursor];
        // First boundary strictly after `here`.
        let target = self
            .spans
            .iter()
            .map(|s| s.start)
            .find(|&start| start > here)
            .unwrap_or(self.events.len().saturating_sub(1));
        self.move_cursor_to_event(target);
    }

    fn jump_prev_span(&mut self) {
        if self.visible.is_empty() || self.spans.is_empty() {
            return;
        }
        let here = self.visible[self.cursor];
        let target = self
            .spans
            .iter()
            .map(|s| s.start)
            .rev()
            .find(|&start| start < here)
            .unwrap_or(0);
        self.move_cursor_to_event(target);
    }

    /// Jump to the decile `d` (0..=9) of the *visible* sequence.
    fn jump_decile(&mut self, d: u8) {
        if self.visible.is_empty() {
            return;
        }
        let n = self.visible.len();
        // 0 -> 0, 9 -> last. d/10 of the way through.
        let target = ((d as usize) * n / 10).min(n - 1);
        self.cursor = target;
        self.list_state.select(Some(target));
    }

    /// Set the cursor to the visible row whose underlying event index is the
    /// closest match to `event_idx` (clamped, exact preferred).
    fn move_cursor_to_event(&mut self, event_idx: usize) {
        if self.visible.is_empty() {
            return;
        }
        let vis = match self.visible.binary_search(&event_idx) {
            Ok(i) => i,
            Err(i) => i.min(self.visible.len() - 1),
        };
        self.cursor = vis;
        self.list_state.select(Some(vis));
    }

    fn current_event(&self) -> Option<&Event> {
        self.visible.get(self.cursor).map(|&i| &self.events[i])
    }
}

/// Public entry point — load the session, run the TUI, restore the terminal,
/// then act on the user's exit choice (e.g. fork at a frame).
pub fn run(root: &Path, session_id: uuid::Uuid) -> Result<()> {
    let store = SessionStore::new(root, session_id);
    let result = SessionReader::read(store.log_path()).context("read session log")?;

    let mut app = App::new(result.events);
    app.parent_session = session_id;

    let mut terminal = setup_terminal().context("setup terminal")?;
    let run_result = run_app(&mut terminal, &mut app, root);
    if let Err(e) = restore_terminal(&mut terminal) {
        tracing::warn!(error = %e, "terminal restore failed");
    }
    run_result?;

    // Act on the requested exit action. We're back in cooked-mode here so
    // any prints land on the user's normal terminal.
    match app.action {
        ExitAction::None => Ok(()),
        ExitAction::ForkAt { parent, at } => {
            let new_id =
                crate::cli::fork::run(root, parent, at, None).context("fork at cursor frame")?;
            // Print to stderr so it can't be confused with regular replay
            // output and is easy to capture in shell pipelines.
            eprintln!("forked session {new_id} (parent {parent} at frame {at})");
            println!("{new_id}");
            Ok(())
        }
        ExitAction::Diff { .. } => {
            // Should never reach here: Diff is handled inline by run_app.
            Ok(())
        }
    }
}

fn setup_terminal() -> Result<Tui> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen).context("enter alt screen")?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("new terminal")
}

fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode().ok();
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    Ok(())
}

fn run_app(terminal: &mut Tui, app: &mut App, root: &Path) -> Result<()> {
    while !app.quit {
        terminal.draw(|f| draw(f, app))?;
        if event::poll(Duration::from_millis(200))? {
            if let event::Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(app, key);
                }
            }
        }
        // Handle Diff inline so the user returns to the main view on `q`.
        if let ExitAction::Diff { peer } = &app.action {
            let peer_id = uuid::Uuid::parse_str(peer)
                .with_context(|| format!("parse peer session id {peer}"))?;
            let our_id = app.parent_session;
            // Reset before opening so a parse failure doesn't loop us.
            app.action = ExitAction::None;
            diff_view::run(terminal, root, our_id, peer_id)
                .context("open side-by-side diff view")?;
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if app.filter_editing {
        match key.code {
            KeyCode::Esc => {
                app.filter_editing = false;
            }
            KeyCode::Enter => {
                app.filter_editing = false;
            }
            KeyCode::Backspace => {
                app.filter.pop();
                app.recompute_filter();
            }
            KeyCode::Char(c) => {
                app.filter.push(c);
                app.recompute_filter();
            }
            _ => {}
        }
        return;
    }
    if app.peer_prompt.is_some() {
        match key.code {
            KeyCode::Esc => {
                app.peer_prompt = None;
            }
            KeyCode::Enter => {
                if let Some(s) = app.peer_prompt.take() {
                    let trimmed = s.trim().to_string();
                    if !trimmed.is_empty() {
                        app.action = ExitAction::Diff { peer: trimmed };
                        app.quit = true;
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(buf) = app.peer_prompt.as_mut() {
                    buf.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(buf) = app.peer_prompt.as_mut() {
                    buf.push(c);
                }
            }
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit = true,
        KeyCode::Char('j') | KeyCode::Down => app.step(1),
        KeyCode::Char('k') | KeyCode::Up => app.step(-1),
        KeyCode::Char('J') => app.jump_next_span(),
        KeyCode::Char('K') => app.jump_prev_span(),
        KeyCode::Char('g') | KeyCode::Home => app.jump_first(),
        KeyCode::Char('G') | KeyCode::End => app.jump_last(),
        KeyCode::Char(c @ '0'..='9') => {
            app.jump_decile(c.to_digit(10).unwrap() as u8);
        }
        KeyCode::Char('f') => {
            // Fork at the current frame. Underlying frame index = visible[cursor].
            if let Some(&frame) = app.visible.get(app.cursor) {
                app.action = ExitAction::ForkAt {
                    parent: app.parent_session,
                    at: frame as u64,
                };
                app.quit = true;
            }
        }
        KeyCode::Char('d') => {
            app.peer_prompt = Some(String::new());
        }
        KeyCode::Char('/') => {
            app.filter_editing = true;
            app.filter.clear();
            app.recompute_filter();
        }
        _ => {}
    }
}

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1), // scrub bar
            Constraint::Length(1), // status bar
        ])
        .split(f.area());

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(35),
            Constraint::Percentage(45),
            Constraint::Percentage(20),
        ])
        .split(outer[0]);

    // Left: timeline.
    let items: Vec<ListItem> = app
        .visible
        .iter()
        .map(|&i| {
            let e = &app.events[i];
            ListItem::new(short_line(e))
        })
        .collect();
    let title = format!("timeline ({}/{})", app.visible.len(), app.events.len());
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    f.render_stateful_widget(list, cols[0], &mut app.list_state);

    // Middle: detail.
    let detail_text = match app.current_event() {
        Some(e) => match serde_json::to_string_pretty(e) {
            Ok(s) => s,
            Err(e) => format!("(serialize error: {e})"),
        },
        None => "(no event selected)".into(),
    };
    let detail = Paragraph::new(detail_text)
        .block(Block::default().borders(Borders::ALL).title("frame"))
        .wrap(Wrap { trim: false });
    f.render_widget(detail, cols[1]);

    // Right: placeholder for filesystem panel.
    let fs = Paragraph::new(vec![
        Line::from(""),
        Line::from(TextSpan::raw("filesystem snapshot")),
        Line::from(TextSpan::raw("(available in v0.2)")),
    ])
    .block(Block::default().borders(Borders::ALL).title("fs"));
    f.render_widget(fs, cols[2]);

    // Scrub bar.
    render_scrub_bar(f, outer[1], app);

    // Status bar.
    let status = if app.filter_editing {
        format!("filter: {}_  (Esc to cancel, Enter to apply)", app.filter)
    } else if let Some(buf) = &app.peer_prompt {
        format!("diff peer session: {buf}_  (Esc to cancel, Enter to open diff)")
    } else if !app.filter.is_empty() {
        format!(
            "j/k step  J/K span  g/G first/last  0-9 decile  f fork  d diff  / filter (active: '{}')  q quit",
            app.filter
        )
    } else {
        "j/k step  J/K span  g/G first/last  0-9 decile  f fork  d diff  / filter  q quit"
            .to_string()
    };
    let status_p = Paragraph::new(status);
    f.render_widget(status_p, outer[2]);
}

/// Build the scrub bar as a single-line string with span boundaries marked
/// and the cursor highlighted. Public-ish for unit tests.
fn render_scrub_bar(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let total = app.events.len();
    let width = area.width as usize;
    if total == 0 || width == 0 {
        f.render_widget(Paragraph::new(""), area);
        return;
    }
    let cursor_event = app.visible.get(app.cursor).copied().unwrap_or(0);
    let line = scrub_bar_line(width, total, &app.spans, cursor_event);
    f.render_widget(Paragraph::new(line), area);
}

/// Pure-function scrub-bar renderer. Produces a `Line` whose cells map across
/// the available terminal `width`, with span boundaries marked by `|`, body
/// cells coloured by span kind, and the cursor cell rendered in reverse video.
fn scrub_bar_line(
    width: usize,
    total: usize,
    spans: &[Span],
    cursor_event: usize,
) -> Line<'static> {
    if width == 0 || total == 0 {
        return Line::default();
    }
    // Map an event index to a column in [0, width).
    let cursor_col = if total <= 1 {
        0
    } else {
        (cursor_event * (width - 1)) / (total - 1)
    };
    // For each column, decide what to render.
    let mut cells: Vec<TextSpan<'static>> = Vec::with_capacity(width);
    // Pre-compute which columns are span boundaries (the `start` of each span,
    // mapped through the same scaling as the cursor).
    let mut boundary_cols: Vec<usize> = spans
        .iter()
        .map(|s| {
            if total <= 1 {
                0
            } else {
                (s.start * (width - 1)) / (total - 1)
            }
        })
        .collect();
    boundary_cols.sort_unstable();
    boundary_cols.dedup();

    for col in 0..width {
        // Find the span this column belongs to (the last span whose mapped
        // start is <= col). Cheap linear scan; spans are short for any
        // reasonable session.
        let mut span_kind: Option<SpanKind> = None;
        for s in spans {
            let start_col = if total <= 1 {
                0
            } else {
                (s.start * (width - 1)) / (total - 1)
            };
            if start_col <= col {
                span_kind = Some(s.kind);
            } else {
                break;
            }
        }
        let is_boundary = boundary_cols.binary_search(&col).is_ok();
        let is_cursor = col == cursor_col;
        let glyph = if is_cursor {
            "*"
        } else if is_boundary {
            "|"
        } else {
            "-"
        };
        let mut style = Style::default().fg(span_color(span_kind));
        if is_cursor {
            style = style.add_modifier(Modifier::REVERSED).fg(Color::White);
        }
        cells.push(TextSpan::styled(glyph.to_string(), style));
    }
    Line::from(cells)
}

fn span_color(kind: Option<SpanKind>) -> Color {
    match kind {
        Some(SpanKind::ToolCall) => Color::Yellow,
        Some(SpanKind::ModelStream) => Color::Cyan,
        Some(SpanKind::InputStream) => Color::Green,
        Some(SpanKind::Resize) => Color::Magenta,
        Some(SpanKind::Marker) => Color::Blue,
        None => Color::DarkGray,
    }
}

fn short_line(e: &Event) -> String {
    match e {
        Event::Output {
            seq, t_ns, bytes, ..
        } => {
            format!("#{seq:>5}  {t_ns:>20}  Output  ({} b64)", bytes.len())
        }
        Event::Input {
            seq, t_ns, bytes, ..
        } => {
            format!("#{seq:>5}  {t_ns:>20}  Input   ({} b64)", bytes.len())
        }
        Event::Resize {
            seq,
            t_ns,
            cols,
            rows,
            ..
        } => format!("#{seq:>5}  {t_ns:>20}  Resize  {cols}x{rows}"),
        Event::Marker {
            seq, t_ns, label, ..
        } => format!("#{seq:>5}  {t_ns:>20}  Marker  {label}"),
    }
}

fn event_filter_string(e: &Event) -> String {
    match e {
        Event::Output { .. } => "Output".into(),
        Event::Input { .. } => "Input".into(),
        Event::Resize { .. } => "Resize".into(),
        Event::Marker { label, .. } => format!("Marker {label}"),
    }
}

#[cfg(test)]
mod tui_tests {
    use super::*;
    use serde_json::Map;

    fn ev_marker(seq: u64, label: &str) -> Event {
        Event::Marker {
            seq,
            t_ns: 0,
            label: label.into(),
            extras: Map::new(),
        }
    }

    fn ev_output(seq: u64) -> Event {
        Event::Output {
            seq,
            t_ns: 0,
            bytes: "aGk=".into(),
            truncated: None,
            truncated_original_size: None,
            extras: Map::new(),
        }
    }

    #[test]
    fn jump_next_span_advances_to_next_boundary() {
        let evs = vec![ev_output(0), ev_output(1), ev_marker(2, "session_end")];
        let mut app = App::new(evs);
        app.cursor = 0;
        app.list_state.select(Some(0));
        app.jump_next_span();
        assert_eq!(app.cursor, 2, "should land on the marker span boundary");
    }

    #[test]
    fn jump_prev_span_goes_to_previous_boundary() {
        let evs = vec![ev_output(0), ev_output(1), ev_marker(2, "session_end")];
        let mut app = App::new(evs);
        app.cursor = 2;
        app.list_state.select(Some(2));
        app.jump_prev_span();
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn decile_jumps_proportionally() {
        let evs: Vec<Event> = (0..10).map(ev_output).collect();
        let mut app = App::new(evs);
        app.jump_decile(0);
        assert_eq!(app.cursor, 0);
        app.jump_decile(5);
        assert_eq!(app.cursor, 5);
        app.jump_decile(9);
        assert_eq!(app.cursor, 9);
    }

    #[test]
    fn scrub_line_marks_cursor_cell() {
        let evs: Vec<Event> = (0..20).map(ev_output).collect();
        let spans = spans::group(&evs);
        let line = scrub_bar_line(40, 20, &spans, 10);
        // Render the line glyphs only.
        let glyphs: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(glyphs.len(), 40);
        assert!(glyphs.contains('*'), "cursor glyph must be present");
    }

    #[test]
    fn scrub_line_handles_zero_total() {
        let line = scrub_bar_line(40, 0, &[], 0);
        assert!(line.spans.is_empty());
    }

    #[test]
    fn scrub_line_handles_zero_width() {
        let evs: Vec<Event> = (0..5).map(ev_output).collect();
        let spans = spans::group(&evs);
        let line = scrub_bar_line(0, 5, &spans, 2);
        assert!(line.spans.is_empty());
    }
}
