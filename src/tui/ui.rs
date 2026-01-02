//! UI rendering for the TUI

use super::app::{App, Panel};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Tabs},
    Frame,
};

/// Render the entire UI
pub fn render(app: &App, frame: &mut Frame) {
    let size = frame.area();

    // Main layout: header, tabs, content, footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(3), // Tabs
            Constraint::Min(10),   // Content
            Constraint::Length(3), // Footer
        ])
        .split(size);

    // Render header
    render_header(app, frame, chunks[0]);

    // Render tabs
    render_tabs(app, frame, chunks[1]);

    // Render current panel content
    match app.current_panel {
        Panel::Status => render_status_panel(app, frame, chunks[2]),
        Panel::Configuration => render_config_panel(app, frame, chunks[2]),
        Panel::Logs => render_logs_panel(app, frame, chunks[2]),
    }

    // Render footer
    render_footer(app, frame, chunks[3]);
}

/// Render the header with title and status
fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let status_char = if app.is_recording { '●' } else { '○' };
    let status_color = if app.is_recording {
        Color::Red
    } else {
        Color::Gray
    };
    let status_text = if app.is_recording {
        format!("Recording ({}s)", app.recording_duration)
    } else {
        "Idle".to_string()
    };

    let title = Line::from(vec![
        Span::styled("ears ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled("v1.0.0", Style::default().fg(Color::DarkGray)),
        Span::raw(" │ Status: "),
        Span::styled(status_char.to_string(), Style::default().fg(status_color)),
        Span::raw(" "),
        Span::styled(status_text, Style::default().fg(status_color)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));

    let paragraph = Paragraph::new(title)
        .block(block)
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}

/// Render the tab bar
fn render_tabs(app: &App, frame: &mut Frame, area: Rect) {
    let titles = vec!["▸ Status", "▸ Configuration", "▸ Logs"];
    let index = match app.current_panel {
        Panel::Status => 0,
        Panel::Configuration => 1,
        Panel::Logs => 2,
    };

    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL))
        .select(index)
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(tabs, area);
}

/// Render the status panel
fn render_status_panel(app: &App, frame: &mut Frame, area: Rect) {
    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Current State: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(if app.is_recording { "Recording" } else { "Idle" }),
        ]),
        Line::from(vec![
            Span::styled("Model: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&app.model),
        ]),
        Line::from(vec![
            Span::styled("Server: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&app.server),
        ]),
        Line::from(vec![
            Span::styled("Device: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&app.device),
        ]),
    ];

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Status ")
                .border_style(Style::default().fg(Color::Green)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}

/// Render the configuration panel
fn render_config_panel(app: &App, frame: &mut Frame, area: Rect) {
    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Server URL: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&app.server),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Model: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&app.model),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Audio Device: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&app.device),
        ]),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "Configuration editing not yet implemented",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Configuration ")
                .border_style(Style::default().fg(Color::Blue)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}

/// Render the logs panel
fn render_logs_panel(app: &App, frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = app
        .logs
        .iter()
        .enumerate()
        .map(|(i, log)| {
            let style = if i == app.selected_log {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(log.as_str()).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Logs ")
                .border_style(Style::default().fg(Color::Magenta)),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(list, area);
}

/// Render the footer with key bindings and command mode
fn render_footer(app: &App, frame: &mut Frame, area: Rect) {
    let footer_text = if app.command_mode {
        Line::from(vec![
            Span::styled(":", Style::default().fg(Color::Yellow)),
            Span::raw(&app.command_buffer),
            Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
        ])
    } else {
        Line::from(vec![
            Span::styled("[Space] ", Style::default().fg(Color::Cyan)),
            Span::raw("Start/Stop  "),
            Span::styled("[h/l] ", Style::default().fg(Color::Cyan)),
            Span::raw("Tabs  "),
            Span::styled("[j/k] ", Style::default().fg(Color::Cyan)),
            Span::raw("Scroll  "),
            Span::styled("[:] ", Style::default().fg(Color::Cyan)),
            Span::raw("Command  "),
            Span::styled("[q] ", Style::default().fg(Color::Cyan)),
            Span::raw("Quit"),
        ])
    };

    let paragraph = Paragraph::new(footer_text)
        .block(Block::default().borders(Borders::ALL))
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}
