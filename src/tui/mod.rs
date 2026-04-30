//! TUI replay viewer.
//!
//! Three panes:
//! * left: timeline (list of events).
//! * middle: detail of the event under the cursor.
//! * right: filesystem placeholder — CoW snapshots ship later.
//!
//! Keys: `j`/`k` step one frame, `J`/`K` jump 10, `g`/`G` first/last,
//! `q` quit, `/` enter filter mode (substring match on event kind/label).
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
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Terminal;

use crate::format::Event;
use crate::store::{SessionReader, SessionStore};

type Tui = Terminal<CrosstermBackend<Stdout>>;

struct App {
    events: Vec<Event>,
    /// Indexes into `events`, after the filter is applied.
    visible: Vec<usize>,
    list_state: ListState,
    cursor: usize,
    filter: String,
    filter_editing: bool,
    quit: bool,
}

impl App {
    fn new(events: Vec<Event>) -> Self {
        let visible: Vec<usize> = (0..events.len()).collect();
        let mut list_state = ListState::default();
        if !visible.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            events,
            visible,
            list_state,
            cursor: 0,
            filter: String::new(),
            filter_editing: false,
            quit: false,
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

    fn current_event(&self) -> Option<&Event> {
        self.visible.get(self.cursor).map(|&i| &self.events[i])
    }
}

/// Public entry point — load the session, run the TUI, restore the terminal.
pub fn run(root: &Path, session_id: uuid::Uuid) -> Result<()> {
    let store = SessionStore::new(root, session_id);
    let result = SessionReader::read(store.log_path()).context("read session log")?;

    let mut terminal = setup_terminal().context("setup terminal")?;
    let app_result = run_app(&mut terminal, App::new(result.events));
    if let Err(e) = restore_terminal(&mut terminal) {
        tracing::warn!(error = %e, "terminal restore failed");
    }
    app_result
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

fn run_app(terminal: &mut Tui, mut app: App) -> Result<()> {
    while !app.quit {
        terminal.draw(|f| draw(f, &mut app))?;
        if event::poll(Duration::from_millis(200))? {
            if let event::Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(&mut app, key);
                }
            }
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
    match key.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit = true,
        KeyCode::Char('j') | KeyCode::Down => app.step(1),
        KeyCode::Char('k') | KeyCode::Up => app.step(-1),
        KeyCode::Char('J') => app.step(10),
        KeyCode::Char('K') => app.step(-10),
        KeyCode::Char('g') => app.jump_first(),
        KeyCode::Char('G') => app.jump_last(),
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
        .constraints([Constraint::Min(1), Constraint::Length(1)])
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
        Line::from(Span::raw("filesystem snapshot")),
        Line::from(Span::raw("(available in v0.2)")),
    ])
    .block(Block::default().borders(Borders::ALL).title("fs"));
    f.render_widget(fs, cols[2]);

    // Status bar.
    let status = if app.filter_editing {
        format!("filter: {}_  (Esc to cancel, Enter to apply)", app.filter)
    } else if !app.filter.is_empty() {
        format!(
            "j/k step  J/K x10  g/G first/last  / filter (active: '{}')  q quit",
            app.filter
        )
    } else {
        "j/k step  J/K x10  g/G first/last  / filter  q quit".to_string()
    };
    let status_p = Paragraph::new(status);
    f.render_widget(status_p, outer[1]);
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
