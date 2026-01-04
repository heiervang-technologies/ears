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
        Panel::LiveTranscription => render_live_transcription_panel(app, frame, chunks[2]),
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
        Span::styled(
            "ears ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
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
    let titles = vec!["▸ Status", "▸ Configuration", "▸ Logs", "▸ Live"];
    let index = match app.current_panel {
        Panel::Status => 0,
        Panel::Configuration => 1,
        Panel::Logs => 2,
        Panel::LiveTranscription => 3,
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
            Span::styled(
                "Current State: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(if app.is_recording {
                "Recording"
            } else {
                "Idle"
            }),
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
            Span::styled(
                "Server URL: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(&app.server),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Model: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&app.model),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Audio Device: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
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
    } else if app.current_panel == Panel::LiveTranscription {
        // Live panel shortcuts
        Line::from(vec![
            Span::styled("[Space/v] ", Style::default().fg(Color::Cyan)),
            Span::raw("Toggle VAD  "),
            Span::styled("[t] ", Style::default().fg(Color::Cyan)),
            Span::raw("Typing  "),
            Span::styled("[a] ", Style::default().fg(Color::Cyan)),
            Span::raw("Auto-corr  "),
            Span::styled("[Tab] ", Style::default().fg(Color::Cyan)),
            Span::raw("Panels  "),
            Span::styled("[q] ", Style::default().fg(Color::Cyan)),
            Span::raw("Quit"),
        ])
    } else {
        // Default shortcuts
        Line::from(vec![
            Span::styled("[Space] ", Style::default().fg(Color::Cyan)),
            Span::raw("Start/Stop  "),
            Span::styled("[v] ", Style::default().fg(Color::Cyan)),
            Span::raw("VAD  "),
            Span::styled("[Tab] ", Style::default().fg(Color::Cyan)),
            Span::raw("Panels  "),
            Span::styled("[j/k] ", Style::default().fg(Color::Cyan)),
            Span::raw("Scroll  "),
            Span::styled("[q] ", Style::default().fg(Color::Cyan)),
            Span::raw("Quit"),
        ])
    };

    let paragraph = Paragraph::new(footer_text)
        .block(Block::default().borders(Borders::ALL))
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}

/// Render the live transcription panel
fn render_live_transcription_panel(app: &App, frame: &mut Frame, area: Rect) {
    // VAD status indicator
    let vad_status_char = if app.vad_active { '●' } else { '○' };
    let vad_status_color = if app.vad_active {
        Color::Green
    } else {
        Color::Gray
    };
    let vad_status_text = if app.vad_active {
        "Active"
    } else {
        "Inactive"
    };

    let mut text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "VAD Mode: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(vad_status_char.to_string(), Style::default().fg(vad_status_color)),
            Span::raw(" "),
            Span::styled(vad_status_text, Style::default().fg(vad_status_color)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Transcription:",
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    // Show transcription text (committed + uncommitted)
    if app.vad_active {
        // Combine committed and uncommitted text
        let full_text = format!("{}{}", app.committed_text, app.uncommitted_text);

        if full_text.is_empty() {
            text.push(Line::from(vec![
                Span::styled("  Listening...", Style::default().fg(Color::DarkGray)),
            ]));
        } else {
            // Split into lines for display
            for line in full_text.lines() {
                if line.len() <= app.committed_text.len() {
                    // This line is fully committed
                    text.push(Line::from(vec![
                        Span::styled(format!("  {}", line), Style::default().fg(Color::White)),
                    ]));
                } else {
                    // This line contains uncommitted text
                    let committed_part = if line.len() <= app.committed_text.len() {
                        line.to_string()
                    } else {
                        app.committed_text[app.committed_text.len().saturating_sub(line.len())..].to_string()
                    };

                    text.push(Line::from(vec![
                        Span::styled(format!("  {}", committed_part), Style::default().fg(Color::White)),
                        Span::styled(
                            &app.uncommitted_text,
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
            }
        }

        text.push(Line::from(""));
        text.push(Line::from(vec![
            Span::styled("  (", Style::default().fg(Color::DarkGray)),
            Span::styled("gray", Style::default().fg(Color::DarkGray)),
            Span::styled(" = uncommitted)", Style::default().fg(Color::DarkGray)),
        ]));
    } else {
        text.push(Line::from(vec![
            Span::styled(
                "  VAD mode is inactive. Press [Space] or [v] to enable.",
                Style::default().fg(Color::Yellow),
            ),
        ]));
    }

    // Settings
    text.push(Line::from(""));
    text.push(Line::from(""));
    text.push(Line::from(vec![
        Span::styled("Settings:", Style::default().add_modifier(Modifier::BOLD)),
    ]));
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.progressive_typing { "[x]" } else { "[ ]" },
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" Progressive Typing"),
    ]));
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.auto_correction { "[x]" } else { "[ ]" },
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" Auto-correction"),
    ]));

    // Stats
    text.push(Line::from(""));
    text.push(Line::from(""));
    text.push(Line::from(vec![
        Span::styled("Stats:", Style::default().add_modifier(Modifier::BOLD)),
    ]));
    text.push(Line::from(vec![
        Span::styled("  Latency: ", Style::default().fg(Color::DarkGray)),
        Span::raw(format!("{}ms", app.avg_latency_ms)),
    ]));
    text.push(Line::from(vec![
        Span::styled("  Segments processed: ", Style::default().fg(Color::DarkGray)),
        Span::raw(format!("{}", app.segments_processed)),
    ]));

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Live Transcription ")
                .border_style(Style::default().fg(Color::Green)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}
