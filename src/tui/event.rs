//! Event handling for the TUI

use anyhow::Result;
use crossterm::event::{self, Event as CrosstermEvent, KeyEvent, MouseEvent};
use std::time::Duration;
use tokio::sync::mpsc;

/// Event types that can occur in the TUI
#[derive(Debug, Clone)]
pub enum Event {
    /// A key was pressed
    Key(KeyEvent),
    /// Mouse event (click, scroll, etc.)
    Mouse(MouseEvent),
    /// Terminal was resized
    Resize(u16, u16),
    /// Tick event for periodic updates
    Tick,
    /// Model name fetched
    ModelFetched(Option<String>),
}

/// Handles terminal events with a configurable tick rate
pub struct EventHandler {
    receiver: mpsc::UnboundedReceiver<Event>,
    sender: mpsc::UnboundedSender<Event>,
}

impl EventHandler {
    /// Create a new event handler with the specified tick rate in milliseconds
    pub fn new(tick_rate_ms: u64) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let tick_rate = Duration::from_millis(tick_rate_ms);
        let thread_sender = sender.clone();
        
        std::thread::spawn(move || {
            loop {
                let event = if event::poll(tick_rate).unwrap_or(false) {
                    match event::read().unwrap_or(CrosstermEvent::FocusGained) {
                        CrosstermEvent::Key(key) => Event::Key(key),
                        CrosstermEvent::Mouse(mouse) => Event::Mouse(mouse),
                        CrosstermEvent::Resize(w, h) => Event::Resize(w, h),
                        _ => Event::Tick,
                    }
                } else {
                    Event::Tick
                };
                
                if thread_sender.send(event).is_err() {
                    break;
                }
            }
        });

        Self { receiver, sender }
    }

    /// Get a sender to inject custom events
    pub fn sender(&self) -> mpsc::UnboundedSender<Event> {
        self.sender.clone()
    }

    /// Wait for the next event asynchronously
    pub async fn next(&mut self) -> Result<Event> {
        self.receiver
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("Event channel closed"))
    }
}
