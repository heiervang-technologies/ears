//! TUI (Terminal User Interface) module
//!
//! Provides an interactive terminal interface for ears with vim-style navigation.

mod app;
mod event;
mod ui;

pub use app::{App, Panel};
pub use event::{Event, EventHandler};

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;

/// Initialize the terminal for TUI mode
pub fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its original state
pub fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

/// Run the TUI application
pub fn run() -> Result<()> {
    let mut terminal = init_terminal()?;
    let mut app = App::new();
    let event_handler = EventHandler::new(250);

    loop {
        terminal.draw(|f| ui::render(&app, f))?;

        if let Event::Key(key) = event_handler.next()? {
            if !app.handle_key(key)? {
                break;
            }
        }
    }

    restore_terminal(&mut terminal)?;
    Ok(())
}
