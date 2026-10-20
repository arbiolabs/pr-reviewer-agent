//! Raw mode and the alternate screen, restored on drop and from the panic hook, so a crash never leaves the user's
//! shell unusable.

use std::io::{self, Stdout};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub struct TerminalGuard {
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
}

pub fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen, Show);
}

impl TerminalGuard {
    pub fn enter(mouse: bool) -> io::Result<TerminalGuard> {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
        enable_raw_mode()?;
        let guard = TerminalGuard { terminal: Terminal::new(CrosstermBackend::new(io::stdout()))? };
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        if mouse {
            execute!(io::stdout(), EnableMouseCapture)?;
        }
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}
