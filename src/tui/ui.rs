//! UI rendering for the TUI

use super::app::{App, ClickAction, EditableField, Panel};
use super::theme::Theme;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs},
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

    // Render help overlay on top of everything
    if app.help_overlay_open {
        render_help_overlay(frame, size, &app.theme);
    }
}

/// Render the header with title and status
fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let (status_char, status_color, status_text) = if app.vad_active {
        let ch = if app.is_speaking { '●' } else { '◉' };
        let color = if app.is_speaking {
            theme.accent
        } else {
            theme.success
        };
        (ch, color, "VAD Active".to_string())
    } else {
        ('○', Color::Gray, "Idle".to_string())
    };

    let title = Line::from(vec![
        Span::styled(
            "ears ",
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            concat!("v", env!("CARGO_PKG_VERSION")),
            Style::default().fg(theme.dim),
        ),
        Span::raw(" │ Status: "),
        Span::styled(status_char.to_string(), Style::default().fg(status_color)),
        Span::raw(" "),
        Span::styled(status_text, Style::default().fg(status_color)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_header));

    let paragraph = Paragraph::new(title)
        .block(block)
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}

/// Render the tab bar
fn render_tabs(app: &mut App, frame: &mut Frame, area: Rect) {
    let titles = vec!["▸ Configuration", "▸ Logs", "▸ Live"];
    let panels = [Panel::Configuration, Panel::Logs, Panel::LiveTranscription];
    let index = match app.current_panel {
        Panel::Configuration => 0,
        Panel::Logs => 1,
        Panel::LiveTranscription => 2,
    };

    let tabs = Tabs::new(titles.clone())
        .block(Block::default().borders(Borders::ALL))
        .select(index)
        .style(Style::default().fg(app.theme.text))
        .highlight_style(
            Style::default()
                .fg(app.theme.accent)
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
    let theme = &app.theme;
    let is_editing_server = app.editing_field == Some(EditableField::ServerUrl);

    // Server URL line - show edit buffer if editing, otherwise show current value
    let server_line = if is_editing_server {
        Line::from(vec![
            Span::styled(
                "Server URL: ",
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(theme.border_active),
            ),
            Span::styled(&app.edit_buffer, Style::default().fg(theme.text)),
            Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
        ])
    } else {
        let mut spans = vec![
            Span::styled(
                "Server URL: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(&app.server),
        ];
        if app.env_server {
            spans.push(Span::styled(
                " [env]",
                Style::default().fg(theme.env_indicator),
            ));
        } else {
            spans.push(Span::styled(" [e]", Style::default().fg(theme.dim)));
        }
        Line::from(spans)
    };

    let profile_display = app.profile.as_deref().unwrap_or("default");

    let mut text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("Profile: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(profile_display, Style::default().fg(theme.title)),
            Span::styled(" [Shift+P]", Style::default().fg(theme.dim)),
        ]),
        Line::from(""),
        server_line,
        Line::from(""),
        Line::from({
            let mut spans = vec![
                Span::styled("Model: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(&app.model),
            ];
            if app.env_model {
                spans.push(Span::styled(
                    " [env]",
                    Style::default().fg(theme.env_indicator),
                ));
            }
            spans
        }),
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
                    .fg(theme.border_active),
            ),
            Span::styled(
                "(j/k navigate, Enter select, Esc cancel)",
                Style::default().fg(theme.dim),
            ),
        ]));

        if let Some(ref error) = app.device_picker_error {
            text.push(Line::from(vec![Span::styled(
                format!("  {}", error),
                Style::default().fg(theme.error),
            )]));
        } else {
            device_list_start = text.len();
            for (i, device) in app.device_picker_devices.iter().enumerate() {
                let is_current = device.name == app.device;
                let is_selected = i == app.device_picker_selected;

                let marker = if is_selected { ">" } else { " " };

                let style = if is_selected {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else if is_current {
                    Style::default().fg(theme.title)
                } else {
                    Style::default()
                };

                let mut spans = vec![
                    Span::styled(format!("  {} ", marker), style),
                    Span::styled(device.description.as_str(), style),
                ];
                if is_current {
                    spans.push(Span::styled(" (current)", Style::default().fg(theme.dim)));
                }
                text.push(Line::from(spans));
            }
        }
    } else {
        let mut spans = vec![
            Span::styled(
                "Audio Device: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(&app.device),
        ];
        if app.env_device {
            spans.push(Span::styled(
                " [env]",
                Style::default().fg(theme.env_indicator),
            ));
        } else {
            spans.push(Span::styled(" [d]", Style::default().fg(theme.dim)));
        }
        text.push(Line::from(spans));
    }

    text.push(Line::from(""));
    text.push(Line::from({
        let mut spans = vec![
            Span::styled("Language: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(app.language.as_deref().unwrap_or("auto")),
        ];
        if app.env_language {
            spans.push(Span::styled(
                " [env]",
                Style::default().fg(theme.env_indicator),
            ));
        } else {
            spans.push(Span::styled(
                " (Shift+L to cycle)",
                Style::default().fg(theme.dim),
            ));
        }
        spans
    }));

    // Text Filters section
    text.push(Line::from(""));
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
            Style::default().fg(theme.accent),
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
            Style::default().fg(theme.accent),
        ),
        Span::raw(" Remove Punctuation [p]"),
    ]));
    let strict_alphabet_line = text.len();
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.text_filters.strict_alphabet {
                "[x]"
            } else {
                "[ ]"
            },
            Style::default().fg(theme.accent),
        ),
        Span::raw(" Strict Alphabet [s]"),
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
    let auto_enter_line = text.len();
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.auto_enter { "[x]" } else { "[ ]" },
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" Auto Enter [n]"),
    ]));
    let save_to_clipboard_line = text.len();
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.save_to_clipboard { "[x]" } else { "[ ]" },
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" Save to Clipboard [b]"),
    ]));
    let typing_mode_line = text.len();
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            app.typing_mode.display_name(),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(" [m]", Style::default().fg(Color::DarkGray)),
    ]));
    text.push(Line::from(vec![
        Span::raw("  Cue Volume: "),
        Span::styled(
            format!("{}%", app.cue_volume),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(" [+/-]", Style::default().fg(Color::DarkGray)),
    ]));
    text.push(Line::from(""));

    // Show appropriate help text
    if app.device_picker_open {
        text.push(Line::from(vec![
            Span::styled("[j/k] ", Style::default().fg(theme.toggle)),
            Span::raw("Navigate  "),
            Span::styled("[Enter] ", Style::default().fg(theme.toggle)),
            Span::raw("Select  "),
            Span::styled("[Esc] ", Style::default().fg(theme.toggle)),
            Span::raw("Cancel"),
        ]));
    } else if is_editing_server {
        text.push(Line::from(vec![
            Span::styled("[Enter] ", Style::default().fg(theme.toggle)),
            Span::raw("Save  "),
            Span::styled("[Esc] ", Style::default().fg(theme.toggle)),
            Span::raw("Cancel"),
        ]));
    } else {
        text.push(Line::from(Span::styled(
            "[P] Profile  [e] URL  [d] Device  [L] Lang  [f] Lower  [p] Punct  [n] Enter  [t] Typing  [a] Auto-corr  [m] Mode  [+/-] Vol",
            Style::default().fg(theme.dim),
        )));
    }

    let border_color = if app.device_picker_open || is_editing_server {
        theme.border_active
    } else {
        theme.border_config
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

    // Register clickable regions for text filters
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

    // Strict alphabet filter toggle
    app.add_clickable_region(
        Rect::new(
            inner_x,
            inner_y + strict_alphabet_line as u16,
            inner_width,
            1,
        ),
        ClickAction::ToggleStrictAlphabetFilter,
    );

    // Auto-enter toggle
    app.add_clickable_region(
        Rect::new(inner_x, inner_y + auto_enter_line as u16, inner_width, 1),
        ClickAction::ToggleAutoEnter,
    );

    // Save to clipboard toggle
    app.add_clickable_region(
        Rect::new(inner_x, inner_y + save_to_clipboard_line as u16, inner_width, 1),
        ClickAction::ToggleSaveToClipboard,
    );

    // Progressive typing toggle
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

    // Typing mode cycle
    app.add_clickable_region(
        Rect::new(inner_x, inner_y + typing_mode_line as u16, inner_width, 1),
        ClickAction::CycleTypingMode,
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
    let theme = &app.theme;
    let search_active = !app.search_buffer.is_empty();
    let query = app.search_buffer.to_lowercase();

    let items: Vec<ListItem> = app
        .logs
        .iter()
        .enumerate()
        .filter(|(_, log)| app.log_filter.matches(log))
        .map(|(i, log)| {
            let is_selected = i == app.selected_log;
            let is_match = search_active && app.search_matches.contains(&i);

            if search_active && is_match {
                let spans = highlight_search(log, &query, is_selected, theme);
                ListItem::new(Line::from(spans))
            } else {
                let style = if is_selected {
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(log.as_str()).style(style)
            }
        })
        .collect();

    let title = if app.log_filter == super::app::LogFilter::All {
        " Logs ".to_string()
    } else {
        format!(" Logs [{}] ", app.log_filter.label())
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(theme.border_logs)),
        )
        .highlight_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_widget(list, area);
}

/// Highlight search matches in a log line
fn highlight_search<'a>(
    text: &'a str,
    query: &str,
    is_selected: bool,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let base_style = if is_selected {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let match_style = Style::default()
        .bg(theme.search_match_bg)
        .fg(theme.search_match_fg);

    let lower = text.to_lowercase();
    let mut spans = Vec::new();
    let mut last = 0;

    for (start, _) in lower.match_indices(query) {
        if start > last {
            spans.push(Span::styled(&text[last..start], base_style));
        }
        spans.push(Span::styled(&text[start..start + query.len()], match_style));
        last = start + query.len();
    }

    if last < text.len() {
        spans.push(Span::styled(&text[last..], base_style));
    }

    spans
}

/// Render the footer with key bindings and command mode
fn render_footer(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let key_style = Style::default().fg(theme.toggle);
    let footer_text = if app.device_picker_open {
        // Device picker mode
        Line::from(vec![
            Span::styled(
                "DEVICE: ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("[j/k] ", key_style),
            Span::raw("Navigate  "),
            Span::styled("[Enter] ", key_style),
            Span::raw("Select  "),
            Span::styled("[Esc] ", key_style),
            Span::raw("Cancel"),
        ])
    } else if app.editing_field.is_some() {
        // Edit mode
        Line::from(vec![
            Span::styled(
                "EDIT: ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("[Enter] ", key_style),
            Span::raw("Save  "),
            Span::styled("[Esc] ", key_style),
            Span::raw("Cancel"),
        ])
    } else if app.command_mode {
        Line::from(vec![
            Span::styled(":", Style::default().fg(theme.accent)),
            Span::raw(&app.command_buffer),
            Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
        ])
    } else if app.search_mode {
        let match_info = if app.search_buffer.is_empty() {
            String::new()
        } else {
            format!(" ({} matches)", app.search_matches.len())
        };
        Line::from(vec![
            Span::styled("/", Style::default().fg(theme.accent)),
            Span::raw(&app.search_buffer),
            Span::styled("_", Style::default().add_modifier(Modifier::SLOW_BLINK)),
            Span::styled(match_info, Style::default().fg(theme.dim)),
        ])
    } else if app.current_panel == Panel::LiveTranscription {
        Line::from(vec![
            Span::styled("[Space/v] ", key_style),
            Span::raw("VAD  "),
            Span::styled("[t] ", key_style),
            Span::raw("Typing  "),
            Span::styled("[a] ", key_style),
            Span::raw("Auto-corr  "),
            Span::styled("[q] ", key_style),
            Span::raw("Quit"),
        ])
    } else if app.current_panel == Panel::Configuration {
        Line::from(vec![
            Span::styled("[P] ", key_style),
            Span::raw("Profile  "),
            Span::styled("[e] ", key_style),
            Span::raw("URL  "),
            Span::styled("[d] ", key_style),
            Span::raw("Device  "),
            Span::styled("[L] ", key_style),
            Span::raw("Lang  "),
            Span::styled("[f] ", key_style),
            Span::raw("Lower  "),
            Span::styled("[p] ", key_style),
            Span::raw("Punct  "),
            Span::styled("[n] ", key_style),
            Span::raw("Enter  "),
            Span::styled("[q] ", key_style),
            Span::raw("Quit"),
        ])
    } else if app.current_panel == Panel::Logs {
        Line::from(vec![
            Span::styled("[/] ", key_style),
            Span::raw("Search  "),
            Span::styled("[F] ", key_style),
            Span::raw("Filter  "),
            Span::styled("[j/k] ", key_style),
            Span::raw("Scroll  "),
            Span::styled("[q] ", key_style),
            Span::raw("Quit"),
        ])
    } else {
        // Default shortcuts
        Line::from(vec![
            Span::styled("[Space/v] ", key_style),
            Span::raw("VAD  "),
            Span::styled("[Tab] ", key_style),
            Span::raw("Panels  "),
            Span::styled("[j/k] ", key_style),
            Span::raw("Scroll  "),
            Span::styled("[q] ", key_style),
            Span::raw("Quit"),
        ])
    };

    let paragraph = Paragraph::new(footer_text)
        .block(Block::default().borders(Borders::ALL))
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);
}

/// Render the live transcription panel
fn render_live_transcription_panel(app: &mut App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    // VAD status indicator
    let vad_status_char = if app.vad_active { '●' } else { '○' };
    let vad_status_color = if app.vad_active {
        theme.success
    } else {
        Color::Gray
    };
    let vad_status_text = if app.vad_active { "Active" } else { "Inactive" };

    let mut text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("VAD Mode: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                vad_status_char.to_string(),
                Style::default().fg(vad_status_color),
            ),
            Span::raw(" "),
            Span::styled(vad_status_text, Style::default().fg(vad_status_color)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Transcription:",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
    ];
    let vad_line = 1; // Line index for VAD Mode (after empty line)

    // Show transcription text (committed + uncommitted)
    if app.vad_active {
        let full_text = format!("{}{}", app.committed_text, app.uncommitted_text);

        if full_text.is_empty() {
            text.push(Line::from(vec![Span::styled(
                "  Listening...",
                Style::default().fg(theme.dim),
            )]));
        } else {
            for line in full_text.lines() {
                if line.len() <= app.committed_text.len() {
                    text.push(Line::from(vec![Span::styled(
                        format!("  {}", line),
                        Style::default().fg(theme.text),
                    )]));
                } else {
                    let committed_part = if line.len() <= app.committed_text.len() {
                        line.to_string()
                    } else {
                        app.committed_text[app.committed_text.len().saturating_sub(line.len())..]
                            .to_string()
                    };

                    text.push(Line::from(vec![
                        Span::styled(
                            format!("  {}", committed_part),
                            Style::default().fg(theme.text),
                        ),
                        Span::styled(&app.uncommitted_text, Style::default().fg(theme.dim)),
                    ]));
                }
            }
        }

        text.push(Line::from(""));
        text.push(Line::from(vec![
            Span::styled("  (", Style::default().fg(theme.dim)),
            Span::styled("gray", Style::default().fg(theme.dim)),
            Span::styled(" = uncommitted)", Style::default().fg(theme.dim)),
        ]));
    } else {
        text.push(Line::from(vec![Span::styled(
            "  VAD mode is inactive. Press [Space] or [v] to enable.",
            Style::default().fg(theme.accent),
        )]));
    }

    let _transcription_lines = text.len();

    // Settings section
    text.push(Line::from(""));
    text.push(Line::from(""));
    text.push(Line::from(vec![Span::styled(
        "Settings:",
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    let progressive_typing_line = text.len();
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.progressive_typing { "[x]" } else { "[ ]" },
            Style::default().fg(theme.toggle),
        ),
        Span::raw(" Progressive Typing [t]"),
    ]));
    let auto_correction_line = text.len();
    let save_to_clipboard_line = text.len();
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.save_to_clipboard { "[x]" } else { "[ ]" },
            Style::default().fg(theme.toggle),
        ),
        Span::raw(" Save to Clipboard [b]"),
    ]));
    text.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            if app.auto_correction { "[x]" } else { "[ ]" },
            Style::default().fg(theme.toggle),
        ),
        Span::raw(" Auto-correction [a]"),
    ]));

    // Stats section
    text.push(Line::from(""));
    text.push(Line::from(""));
    text.push(Line::from(vec![Span::styled(
        "Stats:",
        Style::default().add_modifier(Modifier::BOLD),
    )]));
    text.push(Line::from(vec![
        Span::styled("  Latency: ", Style::default().fg(theme.dim)),
        Span::raw(format!("{}ms", app.avg_latency_ms)),
    ]));
    text.push(Line::from(vec![
        Span::styled("  Segments processed: ", Style::default().fg(theme.dim)),
        Span::raw(format!("{}", app.segments_processed)),
    ]));
    text.push(Line::from(vec![
        Span::styled("  Transcriptions: ", Style::default().fg(theme.dim)),
        Span::raw(format!(
            "{} total, {} ok, {} failed",
            app.total_transcriptions, app.successful_transcriptions, app.failed_transcriptions
        )),
    ]));
    text.push(Line::from(vec![
        Span::styled("  Words transcribed: ", Style::default().fg(theme.dim)),
        Span::raw(format!("{}", app.total_words)),
    ]));

    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Live Transcription ")
                .border_style(Style::default().fg(theme.border_live)),
        )
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, area);

    // Register clickable regions
    // Content starts at area.y + 1 (border) and area.x + 1
    let inner_y = area.y + 1;
    let inner_x = area.x + 1;
    let inner_width = area.width.saturating_sub(2);

    // VAD Mode line (line index 1, after empty line at 0)
    app.add_clickable_region(
        Rect::new(inner_x, inner_y + vad_line as u16, inner_width, 1),
        ClickAction::ToggleVadMode,
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

    // Save to clipboard toggle
    app.add_clickable_region(
        Rect::new(inner_x, inner_y + save_to_clipboard_line as u16, inner_width, 1),
        ClickAction::ToggleSaveToClipboard,
    );
}

/// Render a centered help overlay
fn render_help_overlay(frame: &mut Frame, area: Rect, theme: &Theme) {
    let help_text = vec![
        Line::from(Span::styled(
            "Help (press ? or Esc to close)",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Global:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  q/Esc       Quit"),
        Line::from("  Tab/h/l     Switch panels"),
        Line::from("  j/k         Scroll"),
        Line::from("  v/Space     Toggle VAD"),
        Line::from("  :           Command mode"),
        Line::from("  ?           This help"),
        Line::from(""),
        Line::from(Span::styled(
            "Configuration Panel:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  e           Edit server URL"),
        Line::from("  d           Device picker"),
        Line::from("  Shift+P     Cycle profile"),
        Line::from("  Shift+L     Cycle language"),
        Line::from("  f           Toggle lowercase filter"),
        Line::from("  p           Toggle punctuation filter"),
        Line::from("  s           Toggle strict alphabet filter"),
        Line::from("  n           Toggle auto-enter"),
        Line::from(""),
        Line::from(Span::styled(
            "Live Panel:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  t           Toggle progressive typing"),
        Line::from("  a           Toggle auto-correction"),
        Line::from(""),
        Line::from(Span::styled(
            "Logs Panel:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  /           Search logs"),
        Line::from("  n/N         Next/prev match"),
        Line::from("  Shift+F     Cycle log filter"),
        Line::from(""),
        Line::from(Span::styled(
            "Commands:",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("  :q          Quit"),
        Line::from("  :export     Export logs to file"),
        Line::from("  :theme      Toggle dark/light theme"),
    ];

    let overlay_height = help_text.len() as u16 + 2; // +2 for borders
    let overlay_width = 44;

    let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
    let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;
    let overlay_area = Rect::new(
        x,
        y,
        overlay_width.min(area.width),
        overlay_height.min(area.height),
    );

    frame.render_widget(Clear, overlay_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .title(" Help ");

    let paragraph = Paragraph::new(help_text)
        .block(block)
        .alignment(Alignment::Left);

    frame.render_widget(paragraph, overlay_area);
}
