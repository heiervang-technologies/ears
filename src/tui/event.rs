//! Event handling for the TUI

use anyhow::Result;
use crossterm::event::{self, Event as CrosstermEvent, KeyEvent, MouseEvent};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

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

trait EventSource: Send + 'static {
    fn poll(&mut self, timeout: Duration) -> std::io::Result<bool>;
    fn read(&mut self) -> std::io::Result<CrosstermEvent>;
}

struct TerminalEvents;
impl EventSource for TerminalEvents {
    fn poll(&mut self, timeout: Duration) -> std::io::Result<bool> {
        event::poll(timeout)
    }
    fn read(&mut self) -> std::io::Result<CrosstermEvent> {
        event::read()
    }
}

/// Owns the terminal reader. Drop stops and joins it before returning.
pub struct EventHandler {
    receiver: mpsc::UnboundedReceiver<Event>,
    sender: mpsc::UnboundedSender<Event>,
    failure: watch::Receiver<Option<String>>,
    stopped: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl EventHandler {
    /// Create a new event handler; polling checks shutdown at least every 50 ms.
    /// Crossterm's ready poll makes the subsequent read nonblocking.
    pub fn new(tick_rate_ms: u64) -> Self {
        Self::with_source(tick_rate_ms, TerminalEvents)
    }

    fn with_source(tick_rate_ms: u64, source: impl EventSource) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let (failure_tx, failure) = watch::channel(None);
        let stopped = Arc::new(AtomicBool::new(false));
        let thread_stopped = stopped.clone();
        let thread_sender = sender.clone();
        let worker = std::thread::spawn(move || {
            if let Err(error) = read_events(
                source,
                Duration::from_millis(tick_rate_ms.max(1)),
                thread_stopped,
                thread_sender,
            ) {
                tracing::warn!(error = %format!("{error:#}"), "Terminal event worker failed");
                let _ = failure_tx.send(Some(format!("{error:#}")));
            }
            tracing::debug!("Terminal event worker stopped");
        });
        Self {
            receiver,
            sender,
            failure,
            stopped,
            worker: Some(worker),
        }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<Event> {
        self.sender.clone()
    }

    pub async fn next(&mut self) -> Result<Event> {
        if let Some(error) = self.failure.borrow().clone() {
            anyhow::bail!(error);
        }
        tokio::select! {
            biased;
            _ = self.failure.changed() => {
                anyhow::bail!(self.failure.borrow().clone().unwrap_or_else(|| "Terminal event worker stopped".into()));
            }
            event = self.receiver.recv() => event.ok_or_else(|| anyhow::anyhow!("Event channel closed")),
        }
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.receiver.close();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                tracing::warn!("Terminal event worker panicked");
            }
        }
    }
}

fn read_events(
    mut source: impl EventSource,
    tick_rate: Duration,
    stopped: Arc<AtomicBool>,
    sender: mpsc::UnboundedSender<Event>,
) -> Result<()> {
    use anyhow::Context;
    let mut last_tick = Instant::now();
    while !stopped.load(Ordering::Acquire) && !sender.is_closed() {
        let timeout = tick_rate
            .saturating_sub(last_tick.elapsed())
            .min(Duration::from_millis(50));
        let ready = match source.poll(timeout) {
            Ok(ready) => ready,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("Terminal event poll failed"),
        };
        if stopped.load(Ordering::Acquire) {
            break;
        }
        if ready {
            let input = match source.read() {
                Ok(input) => input,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("Terminal event read failed"),
            };
            let event = match input {
                CrosstermEvent::Key(key) => Some(Event::Key(key)),
                CrosstermEvent::Mouse(mouse) => Some(Event::Mouse(mouse)),
                CrosstermEvent::Resize(w, h) => Some(Event::Resize(w, h)),
                _ => None,
            };
            if let Some(event) = event {
                if sender.send(event).is_err() {
                    break;
                }
            }
        }
        // Input traffic must not postpone periodic UI updates indefinitely.
        if last_tick.elapsed() >= tick_rate {
            if sender.send(Event::Tick).is_err() {
                break;
            }
            last_tick = Instant::now();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers, MouseEventKind};
    use std::{collections::VecDeque, io, sync::atomic::AtomicUsize};

    struct Source {
        events: VecDeque<io::Result<CrosstermEvent>>,
        poll_error: bool,
        polls: Arc<AtomicUsize>,
        dropped: Arc<AtomicBool>,
    }
    impl Source {
        fn new(events: Vec<io::Result<CrosstermEvent>>) -> Self {
            Self {
                events: events.into(),
                poll_error: false,
                polls: Arc::new(AtomicUsize::new(0)),
                dropped: Arc::new(AtomicBool::new(false)),
            }
        }
    }
    impl EventSource for Source {
        fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            if self.poll_error {
                return Err(io::Error::other("poll broke"));
            }
            if !self.events.is_empty() {
                return Ok(true);
            }
            std::thread::sleep(timeout);
            Ok(false)
        }
        fn read(&mut self) -> io::Result<CrosstermEvent> {
            self.events.pop_front().unwrap()
        }
    }
    impl Drop for Source {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn forwards_keys_mouse_resize_and_periodic_ticks() {
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let mouse = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 3,
            row: 4,
            modifiers: KeyModifiers::NONE,
        };
        let source = Source::new(vec![
            Ok(CrosstermEvent::Key(key)),
            Ok(CrosstermEvent::Mouse(mouse)),
            Ok(CrosstermEvent::Resize(80, 24)),
            Ok(CrosstermEvent::FocusGained),
        ]);
        let mut handler = EventHandler::with_source(60_000, source);
        assert!(matches!(handler.next().await.unwrap(), Event::Key(k) if k == key));
        assert!(matches!(handler.next().await.unwrap(), Event::Mouse(m) if m == mouse));
        assert!(matches!(
            handler.next().await.unwrap(),
            Event::Resize(80, 24)
        ));
        handler
            .sender()
            .send(Event::ModelFetched(Some("test".into())))
            .unwrap();
        loop {
            if let Event::ModelFetched(model) = handler.next().await.unwrap() {
                assert_eq!(model.as_deref(), Some("test"));
                break;
            }
        }
    }

    #[tokio::test]
    async fn ticks_arrive_while_idle_and_during_continuous_input() {
        let mut idle = EventHandler::with_source(10, Source::new(vec![]));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), idle.next())
                .await
                .unwrap()
                .unwrap(),
            Event::Tick
        ));
        struct Busy;
        impl EventSource for Busy {
            fn poll(&mut self, _: Duration) -> io::Result<bool> {
                std::thread::sleep(Duration::from_millis(1));
                Ok(true)
            }
            fn read(&mut self) -> io::Result<CrosstermEvent> {
                Ok(CrosstermEvent::Resize(80, 24))
            }
        }
        let mut busy = EventHandler::with_source(10, Busy);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(busy.next().await.unwrap(), Event::Tick) {
                    break;
                }
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn interrupted_read_is_retried_without_a_fake_event() {
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let source = Source::new(vec![
            Err(io::ErrorKind::Interrupted.into()),
            Ok(CrosstermEvent::Key(key)),
        ]);
        let mut handler = EventHandler::with_source(60_000, source);
        assert!(
            matches!(tokio::time::timeout(Duration::from_secs(1), handler.next()).await.unwrap().unwrap(), Event::Key(k) if k == key)
        );
    }

    #[tokio::test]
    async fn terminal_errors_stop_worker_instead_of_generating_fake_ticks() {
        for poll_error in [false, true] {
            let mut source = Source::new(vec![Err(io::Error::other("read broke"))]);
            source.poll_error = poll_error;
            let polls = source.polls.clone();
            let dropped = source.dropped.clone();
            let mut handler = EventHandler::with_source(10, source);
            let error = tokio::time::timeout(Duration::from_secs(1), handler.next())
                .await
                .unwrap()
                .unwrap_err();
            assert!(error.to_string().contains(if poll_error {
                "poll failed"
            } else {
                "read failed"
            }));
            drop(handler);
            assert!(dropped.load(Ordering::SeqCst));
            assert_eq!(polls.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn drop_joins_worker_even_with_a_long_tick_interval() {
        let source = Source::new(vec![]);
        let dropped = source.dropped.clone();
        let polls = source.polls.clone();
        let handler = EventHandler::with_source(60_000, source);
        let deadline = Instant::now() + Duration::from_secs(1);
        while polls.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let started = Instant::now();
        drop(handler);
        assert!(dropped.load(Ordering::SeqCst));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
