//! Global panic hook.
//!
//! On panic from any thread we best-effort restore the terminal (leave the
//! alternate screen, disable raw mode, show the cursor) before deferring to
//! the default panic printer. Without this, a TUI crash leaves the user's
//! shell in raw mode and the alt-screen buffer still active.

use std::io;
use std::panic;

pub fn install() {
    let default = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default(info);
    }));
}

fn restore_terminal() {
    use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
    let _ = disable_raw_mode();
    let mut out = io::stdout();
    let _ = crossterm::execute!(out, LeaveAlternateScreen, crossterm::cursor::Show);
}
