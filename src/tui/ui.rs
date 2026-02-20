//! UI rendering for the TUI

use super::app::{App, ClickAction, EditableField, Panel};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs},
    Frame,
};

/// Render the entire UI
pub fn render(app: &mut App, frame: &mut Frame) {
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

    // Render tabs (with clickable regions)
    render_tabs(app, frame, chunks[1]);

    // Render current panel content
    match app.current_panel {
        Panel::Configuration => render_config_panel(app, frame, chunks[2]),
        Panel::Logs => render_logs_panel(app, frame, chunks[2]),
        Panel::LiveTranscription => render_live_transcription_panel(app, frame, chunks[2]),
    }

    // Render footer
    render_footer(app, frame, chunks[3]);
}

/// Render the header with title and status
fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let (status_char, status_color, status_text) = if app.vad_active {
        let ch = if app.is_speaking { '●' } else { '◉' };
        let color = if app.is_speaking {
            Color::Yellow
        } else {
            Color::Green
        };
        (ch, color, "VAD Active".to_string())
    } else {
        ('○', Color::Gray, "Idle".to_string())
    };

    let title = Line::from(vec![
        Span::styled(
            "ears ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            concat!("v", env!("CARGO_PKG_VERSION")),
            Style::default().fg(Color::DarkGray),
        ),
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
fn render_tabs(app: &mut App, frame: &mut Frame, area: Rect) {
    let titles = vec!["▸ Configuration", "▸ Logs", "▸ Live"];
    let panels = [
        Panel::Configuration,
        Panel::Logs,
        Panel::LiveTranscription,
    ];
    let index = match app.current_panel {
        Panel::Configuration => 0,
        Panel::Logs => 1,
        Panel::LiveTranscription => 2,
    };

    let tabs = Tabs::new(titles.clone())
        .block(Block::default().borders(Borders::ALL))
        .select(index)
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(tabs, area);

    // Register clickable regions for each tab
    // Tabs are rendered inside the border, so offset by 1
    let inner_x = area.x + 1;
    let inner_y = area.y + 1;
    let mut x_offset = inner_x;

    for (i, title) in titles.iter().enumerate() {
        let tab_width = title.len() as u16 + 1; // +1 for spacing between tabs
        let tab_rect = Rect::new(x_offset, inner_y, tab_width, 1);
        app.add_clickable_region(tab_rect, ClickAction::SwitchPanel(panels[i]));
        x_offset += tab_width;
    }
}


/// Render the configuration panel
fn render_config_panel(app: &mut App, frame: &mut Frame, area: Rect) {
    let is_editing_server = app.editing_field == Some(EditableField::ServerUrl);

    // Server URL line - show edit buffer if editing, otherwise show current value
    let server_line = if is_editing_server {
        Line::from(vec![
            Span::styled(
                "Server URL: ",
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Yellow),
            ),
            Span::styled(&app.edit_buffer, Style::default().fg(Color::White)),
            Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "Server URL: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(&app.server),
            Span::styled(" [e]", Style::default().fg(Color::DarkGray)),
        ])
    };

    let profile_display = app.profile.as_deref().unwrap_or("default");

    let mut text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Profile: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(profile_display, Style::default().fg(Color::Cyan)),
            Span::styled(" [Shift+P]", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
        server_line,
        Line::from(""),
        Line::from(vec![
            Span::styled("Model: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&app.model),
        ]),
        Line::from(""),
    ];

    // Device picker: show list when open, single line when closed
    let mut device_list_start: usize = 0;
    if app.device_picker_open {
        text.push(Line::from(vec![
            Span::styled(
                "Audio Device: ",
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Yellow),
            ),
            Span::styled(
                "(j/k navigate, Enter select, Esc cancel)",
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        if let Some(ref error) = app.device_picker_error {
            text.push(Line::from(vec![Span::styled(
                format!("  {}", error),
                Style::default().fg(Color::Red),
            )]));
        } else {
            device_list_start = text.len();
            for (i, device) in app.device_picker_devices.iter().enumerate() {
                let is_current = device.name == app.device;
                let is_selected = i == app.device_picker_selected;

                let marker = if is_selected { ">" } else { " " };

                let style = if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else if is_current {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                };

                let mut spans = vec![
                    Span::styled(format!("  {} ", marker), style),
                    Span::styled(device.description.as_str(), style),
                ];
                if is_current {
                    spans.push(Span::styled(
                        " (current)",
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                text.push(Line::from(spans));
            }
        }
    } else {
        text.push(Line::from(vec![
            Span::styled(
                "Audio Device: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(&app.device),
            Span::styled(" [d]", Style::default().fg(Color::DarkGray)),
        ]));
    }

    text.push(Line::from(""));
    text.push(Line::from(vec![
        Span::styled("Language: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(app.language.as_deref().unwrap_or("auto")),
        Span::styled(" (Shift+L to cycle)", Style::default().fg(Color::DarkGray)),
    ]));

    // Text Filters section
    text.push(Line::from(""));
    text.push(Line::from(vec![Span::styled(
        "Text Filters:",
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    let lowercase_line = text.len();
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.text_filters.lowercase {
                "[x]"
            } else {
                "[ ]"
            },
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" Lowercase [f]"),
    ]));
    let punctuation_line = text.len();
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.text_filters.remove_punctuation {
                "[x]"
            } else {
                "[ ]"
            },
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" Remove Punctuation [p]"),
    ]));

    // Typing Settings section
    text.push(Line::from(""));
    text.push(Line::from(vec![Span::styled(
        "Typing Settings:",
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    let progressive_typing_line = text.len();
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.progressive_typing { "[x]" } else { "[ ]" },
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" Progressive Typing [t]"),
    ]));
    let auto_correction_line = text.len();
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.auto_correction { "[x]" } else { "[ ]" },
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" Auto-correction [a]"),
    ]));

    text.push(Line::from(""));

    // Show appropriate help text
    if app.device_picker_open {
        text.push(Line::from(vec![
            Span::styled("[j/k] ", Style::default().fg(Color::Cyan)),
            Span::raw("Navigate  "),
            Span::styled("[Enter] ", Style::default().fg(Color::Cyan)),
            Span::raw("Select  "),
            Span::styled("[Esc] ", Style::default().fg(Color::Cyan)),
            Span::raw("Cancel"),
        ]));
    } else if is_editing_server {
        text.push(Line::from(vec![
            Span::styled("[Enter] ", Style::default().fg(Color::Cyan)),
            Span::raw("Save  "),
            Span::styled("[Esc] ", Style::default().fg(Color::Cyan)),
            Span::raw("Cancel"),
        ]));
    } else {
        text.push(Line::from(Span::styled(
            "[P] Profile  [e] URL  [d] Device  [L] Lang  [f] Lower  [p] Punct  [t] Typing  [a] Auto-corr",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let border_color = if app.device_picker_open || is_editing_server {
        Color::Yellow
    } else {
        Color::Blue
    };

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Configuration ")
                .border_style(Style::default().fg(border_color)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);

    // Register clickable regions for text filters and typing settings
    let inner_y = area.y + 1;
    let inner_x = area.x + 1;
    let inner_width = area.width.saturating_sub(2);

    // Lowercase filter toggle
    app.add_clickable_region(
        Rect::new(inner_x, inner_y + lowercase_line as u16, inner_width, 1),
        ClickAction::ToggleLowercaseFilter,
    );

    // Punctuation filter toggle
    app.add_clickable_region(
        Rect::new(inner_x, inner_y + punctuation_line as u16, inner_width, 1),
        ClickAction::TogglePunctuationFilter,
    );

    // Progressive Typing toggle
    app.add_clickable_region(
        Rect::new(
            inner_x,
            inner_y + progressive_typing_line as u16,
            inner_width,
            1,
        ),
        ClickAction::ToggleProgressiveTyping,
    );

    // Auto-correction toggle
    app.add_clickable_region(
        Rect::new(
            inner_x,
            inner_y + auto_correction_line as u16,
            inner_width,
            1,
        ),
        ClickAction::ToggleAutoCorrection,
    );

    // Device picker clickable regions
    if app.device_picker_open && app.device_picker_error.is_none() {
        let device_count = app.device_picker_devices.len();
        for i in 0..device_count {
            app.add_clickable_region(
                Rect::new(
                    inner_x,
                    inner_y + (device_list_start + i) as u16,
                    inner_width,
                    1,
                ),
                ClickAction::SelectDevice(i),
            );
        }
    }
}

/// Render the logs panel
fn render_logs_panel(app: &App, frame: &mut Frame, area: Rect) {
    let items: Vec<ListItem> = app
        .logs
        .iter()
        .map(|log| ListItem::new(log.as_str()))
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

    let mut list_state = ListState::default();
    list_state.select(Some(app.selected_log));

    frame.render_stateful_widget(list, area, &mut list_state);
}

/// Render the footer with key bindings and command mode
fn render_footer(app: &App, frame: &mut Frame, area: Rect) {
    let footer_text = if app.device_picker_open {
        // Device picker mode
        Line::from(vec![
            Span::styled(
                "DEVICE: ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("[j/k] ", Style::default().fg(Color::Cyan)),
            Span::raw("Navigate  "),
            Span::styled("[Enter] ", Style::default().fg(Color::Cyan)),
            Span::raw("Select  "),
            Span::styled("[Esc] ", Style::default().fg(Color::Cyan)),
            Span::raw("Cancel"),
        ])
    } else if app.editing_field.is_some() {
        // Edit mode
        Line::from(vec![
            Span::styled(
                "EDIT: ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("[Enter] ", Style::default().fg(Color::Cyan)),
            Span::raw("Save  "),
            Span::styled("[Esc] ", Style::default().fg(Color::Cyan)),
            Span::raw("Cancel"),
        ])
    } else if app.command_mode {
        Line::from(vec![
            Span::styled(":", Style::default().fg(Color::Yellow)),
            Span::raw(&app.command_buffer),
            Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
        ])
    } else if app.current_panel == Panel::LiveTranscription {
        // Live panel shortcuts
        Line::from(vec![
            Span::styled("[Space/v] ", Style::default().fg(Color::Cyan)),
            Span::raw("VAD  "),
            Span::styled("[c] ", Style::default().fg(Color::Cyan)),
            Span::raw("Config  "),
            Span::styled("[Tab] ", Style::default().fg(Color::Cyan)),
            Span::raw("Panels  "),
            Span::styled("[q] ", Style::default().fg(Color::Cyan)),
            Span::raw("Quit"),
        ])
    } else if app.current_panel == Panel::Configuration {
        // Configuration panel shortcuts
        Line::from(vec![
            Span::styled("[P] ", Style::default().fg(Color::Cyan)),
            Span::raw("Profile  "),
            Span::styled("[e] ", Style::default().fg(Color::Cyan)),
            Span::raw("URL  "),
            Span::styled("[d] ", Style::default().fg(Color::Cyan)),
            Span::raw("Device  "),
            Span::styled("[L] ", Style::default().fg(Color::Cyan)),
            Span::raw("Lang  "),
            Span::styled("[f] ", Style::default().fg(Color::Cyan)),
            Span::raw("Lower  "),
            Span::styled("[p] ", Style::default().fg(Color::Cyan)),
            Span::raw("Punct  "),
            Span::styled("[t] ", Style::default().fg(Color::Cyan)),
            Span::raw("Typing  "),
            Span::styled("[a] ", Style::default().fg(Color::Cyan)),
            Span::raw("Auto-corr  "),
            Span::styled("[q] ", Style::default().fg(Color::Cyan)),
            Span::raw("Quit"),
        ])
    } else {
        // Logs panel shortcuts
        Line::from(vec![
            Span::styled("[j/k] ", Style::default().fg(Color::Cyan)),
            Span::raw("Scroll  "),
            Span::styled("[Tab] ", Style::default().fg(Color::Cyan)),
            Span::raw("Panels  "),
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
    let mut text = vec![];

    // Show transcription text (committed + uncommitted)
    if app.vad_active {
        if app.committed_text.is_empty() && app.uncommitted_text.is_empty() {
            text.push(Line::from(vec![Span::styled(
                "Listening...",
                Style::default().fg(Color::DarkGray),
            )]));
        } else {
            // Show committed text (split by newlines to handle multi-line transcriptions)
            if !app.committed_text.is_empty() {
                for line in app.committed_text.lines() {
                    text.push(Line::from(vec![Span::styled(
                        line.to_string(),
                        Style::default().fg(Color::White),
                    )]));
                }
            }

            // Show uncommitted text on same or next line
            if !app.uncommitted_text.is_empty() {
                if app.committed_text.is_empty() {
                    text.push(Line::from(vec![Span::styled(
                        app.uncommitted_text.clone(),
                        Style::default().fg(Color::DarkGray),
                    )]));
                } else {
                    // Append to last line if committed text exists
                    if let Some(last_line) = text.last_mut() {
                        last_line.spans.push(Span::styled(
                            app.uncommitted_text.clone(),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                }
            }

            text.push(Line::from(""));
            text.push(Line::from(vec![
                Span::styled("(", Style::default().fg(Color::DarkGray)),
                Span::styled("gray", Style::default().fg(Color::DarkGray)),
                Span::styled(" = uncommitted)", Style::default().fg(Color::DarkGray)),
            ]));
        }
    } else {
        text.push(Line::from(vec![Span::styled(
            "VAD mode is inactive. Press [Space] or [v] to enable.",
            Style::default().fg(Color::Yellow),
        )]));
    }

    // Stats in bottom-right corner (we'll create a custom block title for this)
    let stats_text = format!("Latency: {}ms  #{}", app.avg_latency_ms, app.segments_processed);
    let title = if app.segments_processed > 0 {
        format!(" Live Transcription {} ", stats_text)
    } else {
        " Live Transcription ".to_string()
    };

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(Color::Green)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}
