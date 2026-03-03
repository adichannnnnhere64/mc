use std::fs;  // Add this import
//

use ratatui::{
    layout::{Alignment, Constraint, Margin,  Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};


use crate::{
    app::{App, AppMode, ConnectionStep},
    server::{ServerStatus, ServerType},
};

pub fn render(app: &App, frame: &mut Frame) {

    let area = frame.area();

    // Vertical: main area + 1-line status bar

    let vertical = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);

    // Horizontal: 30% sidebar + 70% detail
    let horizontal =

        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(vertical[0]);

    render_sidebar(app, frame, horizontal[0]);
    render_detail(app, frame, horizontal[1]);
    render_status_bar(app, frame, vertical[1]);


    match &app.mode {
        AppMode::Installing { input } => {
            render_install_modal(input, frame, area);
        }
        AppMode::AddConnection { input, path_input, step } => {
            render_add_connection_modal(input, path_input, step, frame, area);
        }
        AppMode::ManageConnections => {
            render_manage_connections_modal(app, frame, area);
        }
        AppMode::RemoveConnection { selected } => {
            render_remove_connection_modal(app, *selected, frame, area);

        }
        _ => {}
    }
}

// ─── Sidebar ────────────────────────────────────────────────────────────────

fn render_sidebar(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::bordered()
        .title(format!(" Servers ({}) ", app.servers.len()))
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));

    if app.servers.is_empty() {
        let text = Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No servers found.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press 'a' to add a connection:",

                Style::default().fg(Color::Cyan),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press 'm' to manage connections.",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press r to refresh.",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        frame.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let items: Vec<ListItem> = app
        .servers
        .iter()

        .enumerate()
        .map(|(i, server)| {
            let (icon_color, icon) = match &server.status {
                ServerStatus::Running => (Color::Green, "●"),
                ServerStatus::Stopped => (Color::DarkGray, "○"),
                ServerStatus::Starting => (Color::Yellow, "◐"),
                ServerStatus::Error(_) => (Color::Red, "✗"),
            };
            let line = Line::from(vec![
                Span::styled(format!(" {icon} "), Style::default().fg(icon_color)),
                Span::raw(server.name.clone()),
            ]);

            let style = if i == app.selected {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(line).style(style)
        })
        .collect();

    let list = List::new(items).block(block);
    let mut state = ListState::default();

    state.select(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

// ─── Log Panel ──────────────────────────────────────────────────────────────

fn render_log_panel(app: &App, scroll: usize, frame: &mut Frame, area: Rect) {
    let server = &app.servers[app.selected];
    let container = server
        .container_name
        .as_deref()
        .unwrap_or("unknown");

    let block = Block::bordered()
        .title(format!(" Logs: {} ", container))
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible = inner.height as usize;
    let total = app.log_lines.len();

    // Clamp scroll so the last page is the floor
    let max_scroll = total.saturating_sub(visible);
    let scroll = scroll.min(max_scroll);

    let lines: Vec<Line> = app
        .log_lines
        .iter()
        .skip(scroll)
        .take(visible)
        .map(|l| {
            let color = if l.contains("ERROR") || l.contains("error") {
                Color::Red
            } else if l.contains("WARN") {
                Color::Yellow
            } else if l.contains("INFO") {
                Color::White
            } else {
                Color::DarkGray
            };
            Line::from(Span::styled(l.clone(), Style::default().fg(color)))
        })
        .collect();

    let footer = format!(" {}/{} lines ", scroll + lines.len().min(visible), total);
    let footer_line = Line::from(Span::styled(footer, Style::default().fg(Color::DarkGray)));

    // Split inner area: log lines + 1-line footer
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);
    frame.render_widget(Paragraph::new(Text::from(lines)), chunks[0]);
    frame.render_widget(
        Paragraph::new(footer_line).alignment(Alignment::Right),
        chunks[1],
    );
}

// ─── Manage Packs Panel ─────────────────────────────────────────────────────

fn render_manage_packs(app: &App, selected: usize, frame: &mut Frame, area: Rect) {
    let server = &app.servers[app.selected];

    let block = Block::bordered()
        .title(" Manage Packs ")
        .title_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);

    let hint = Line::from(vec![
        Span::styled(" ↑↓ ", Style::default().fg(Color::Cyan)),
        Span::raw("navigate  "),
        Span::styled("Space/Enter ", Style::default().fg(Color::Cyan)),
        Span::raw("toggle  "),
        Span::styled("Esc ", Style::default().fg(Color::Red)),
        Span::raw("back"),
    ]);
    frame.render_widget(Paragraph::new(hint), chunks[0]);

    let rp = &server.installed_resource_packs;
    let bp = &server.installed_behavior_packs;

    let mut items: Vec<ListItem> = Vec::new();

    // ── Resource Packs ──
    items.push(ListItem::new(Line::from(Span::styled(
        "  Resource Packs",
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ))));
    if rp.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "    (none installed)",
            Style::default().fg(Color::DarkGray),
        ))));
    } else {
        for (i, pack) in rp.iter().enumerate() {
            items.push(pack_manage_item(pack.enabled, &pack.name, &pack.version, i == selected));
        }
    }

    items.push(ListItem::new(Line::from("")));

    // ── Behavior Packs ──
    items.push(ListItem::new(Line::from(Span::styled(
        "  Behavior Packs",
        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
    ))));
    if bp.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "    (none installed)",
            Style::default().fg(Color::DarkGray),
        ))));
    } else {
        let rp_len = rp.len();
        for (i, pack) in bp.iter().enumerate() {
            items.push(pack_manage_item(
                pack.enabled,
                &pack.name,
                &pack.version,
                rp_len + i == selected,
            ));
        }
    }

    frame.render_widget(List::new(items), chunks[1]);
}

fn pack_manage_item(enabled: bool, name: &str, version: &[u32], highlighted: bool) -> ListItem<'static> {
    let (badge, badge_color) = if enabled {
        ("[✓]", Color::Green)
    } else {
        ("[✗]", Color::DarkGray)
    };
    let ver = version
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(".");
    let line = Line::from(vec![
        Span::styled(format!("  {badge} "), Style::default().fg(badge_color)),
        Span::raw(name.to_owned()),
        Span::styled(format!("  v{ver}"), Style::default().fg(Color::DarkGray)),
    ]);
    let style = if highlighted {
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    ListItem::new(line).style(style)
}

// ─── Detail Panel ───────────────────────────────────────────────────────────

fn render_detail(app: &App, frame: &mut Frame, area: Rect) {
    if let AppMode::ViewLogs { scroll } = &app.mode {
        render_log_panel(app, *scroll, frame, area);
        return;
    }
    if let AppMode::ManagePacks { selected } = &app.mode {
        render_manage_packs(app, *selected, frame, area);
        return;
    }

    let block = Block::bordered()
        .title(" Server Details ")

        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));

    if app.servers.is_empty() {
        let text = Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No servers available.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Add a connection with 'a'",
                Style::default().fg(Color::Cyan),
            )),
        ]);
        frame.render_widget(
            Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    let server = &app.servers[app.selected];

    let (status_color, status_label) = match &server.status {
        ServerStatus::Running => (Color::Green, "RUNNING"),
        ServerStatus::Stopped => (Color::DarkGray, "STOPPED"),
        ServerStatus::Starting => (Color::Yellow, "STARTING"),
        ServerStatus::Error(_) => (Color::Red, "ERROR"),
    };

    // Detect if path is a symlink
    let is_symlink = server.path.is_symlink();
    let path_display = if is_symlink {
        format!("{} → {}", server.path.display(), 
                fs::read_link(&server.path).unwrap_or_default().display())
    } else {
        server.path.display().to_string()

    };

    let label_style = Style::default().fg(Color::DarkGray);

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Name:     ", label_style),
            Span::styled(
                server.name.clone(),
                Style::default()
                    .fg(Color::White)

                    .add_modifier(Modifier::BOLD),
            ),
        ]),

        Line::from(vec![
            Span::styled("  Type:     ", label_style),
            Span::styled(
                server.server_type.as_str(),
                Style::default().fg(match server.server_type {
                    ServerType::Bedrock => Color::Blue,
                    ServerType::Java => Color::Red,
                    ServerType::Unknown => Color::DarkGray,
                }),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Port:     ", label_style),
            Span::styled(
                server.port.map_or("unknown".to_string(), |p| p.to_string()),
                Style::default().fg(Color::Cyan),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Path:     ", label_style),

            Span::styled(path_display, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("  Status:   ", label_style),
            Span::styled(
                status_label,

                Style::default()

                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("  Resource Packs ({}):", server.resource_packs.len()),
            Style::default().fg(Color::Yellow),
        )),
    ];

    if server.resource_packs.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (none)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {

        for pack in &server.resource_packs {
            lines.push(pack_line(pack.pack_id.clone(), &pack.version, Color::Yellow));

        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  Behavior Packs ({}):", server.behavior_packs.len()),
        Style::default().fg(Color::Magenta),
    )));


    if server.behavior_packs.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (none)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for pack in &server.behavior_packs {

            lines.push(pack_line(pack.pack_id.clone(), &pack.version, Color::Magenta));
        }
    }


    if let ServerStatus::Error(e) = &server.status {

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  Error: {e}"),
            Style::default().fg(Color::Red),

        )));
    }

    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        area,

    );
}

fn pack_line(pack_id: String, version: &[u32], bullet_color: Color) -> Line<'static> {
    let ver = version
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(".");
    Line::from(vec![
        Span::styled("    • ", Style::default().fg(bullet_color)),
        Span::styled(pack_id, Style::default().fg(Color::Gray)),
        Span::styled(
            format!("  v{ver}"),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

// ─── Status Bar ─────────────────────────────────────────────────────────────

fn render_status_bar(app: &App, frame: &mut Frame, area: Rect) {

    let line = if let Some(msg) = &app.message {

        Line::from(vec![

            Span::raw(" "),
            Span::styled(msg.as_str().to_owned(), Style::default().fg(Color::Yellow)),
        ])
    } else {
        match &app.mode {
            AppMode::Normal => Line::from(vec![
                kb(" q ", "quit"),
                Span::raw("  "),
                kb(" ↑↓ ", "navigate"),
                Span::raw("  "),
                kb(" a ", "add conn"),
                Span::raw("  "),
                kb(" m ", "manage"),
                Span::raw("  "),
                kb(" i ", "install"),
                Span::raw("  "),
                kb(" p ", "packs"),
                Span::raw("  "),
                kb(" l ", "logs"),
                Span::raw("  "),
                kb(" r ", "refresh"),
            ]),
            AppMode::Installing { .. } => Line::from(vec![

                kb(" Enter ", "confirm"),
                Span::raw("  "),
                kb(" Esc ", "cancel"),
            ]),

            AppMode::AddConnection { step, .. } => {
                let step_text = match step {
                    ConnectionStep::Name => "Enter connection name",
                    ConnectionStep::Path => "Enter server path",
                };
                Line::from(vec![
                    Span::styled(format!(" {} ", step_text), Style::default().fg(Color::Cyan)),
                    Span::raw("  "),
                    kb(" Enter ", "next"),

                    Span::raw("  "),
                    kb(" Esc ", "cancel"),
                ])

            }
            AppMode::ManageConnections => Line::from(vec![
                kb(" ↑↓ ", "select"),

                Span::raw("  "),
                kb(" Enter ", "view"),
                Span::raw("  "),
                kb(" d ", "delete"),
                Span::raw("  "),
                kb(" Esc ", "back"),
            ]),
            AppMode::RemoveConnection { .. } => Line::from(vec![
                Span::styled(" Delete connection? ", Style::default().fg(Color::Red)),
                Span::raw("  "),
                kb(" y ", "yes"),
                Span::raw("  "),
                kb(" n ", "no"),
            ]),
            AppMode::ViewLogs { .. } => Line::from(vec![
                kb(" ↑↓ ", "scroll"),
                Span::raw("  "),
                kb(" PgUp/PgDn ", "page"),
                Span::raw("  "),
                kb(" Esc ", "back"),
            ]),
            AppMode::ManagePacks { .. } => Line::from(vec![
                kb(" ↑↓ ", "navigate"),
                Span::raw("  "),
                kb(" Space/Enter ", "toggle"),
                Span::raw("  "),
                kb(" Esc ", "back"),
            ]),
        }
    };

    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Color::DarkGray)),
        area,
    );
}

fn kb(key: &str, label: &str) -> Span<'static> {
    Span::raw(format!("[{}{}]", key.trim(), label))

}


// ─── Install Modal ──────────────────────────────────────────────────────────

fn render_install_modal(input: &str, frame: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 40, area);
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .title(" Install Plugin ")
        .title_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));

    let content = Text::from(vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Path: ", Style::default().fg(Color::DarkGray)),
            Span::styled(input.to_owned(), Style::default().fg(Color::White)),
            Span::styled(
                "█",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::SLOW_BLINK),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Supports: folder, .zip, .mcaddon",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Enter", Style::default().fg(Color::Green)),
            Span::styled(" confirm   ", Style::default().fg(Color::DarkGray)),
            Span::styled("Esc", Style::default().fg(Color::Red)),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ]),
    ]);

    frame.render_widget(Paragraph::new(content).block(block), popup);
}

// ─── Add Connection Modal ───────────────────────────────────────────────────

fn render_add_connection_modal(
    input: &str,
    path_input: &str,
    step: &ConnectionStep,
    frame: &mut Frame,

    area: Rect,
) {
    let popup = centered_rect(70, 50, area);
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .title(" Add Server Connection ")
        .title_style(
            Style::default()

                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));

    let (current_input, prompt) = match step {
        ConnectionStep::Name => (input, "Connection Name:"),
        ConnectionStep::Path => (path_input, "Server Path:"),
    };

    let content = Text::from(vec![
        Line::from(""),

        Line::from(vec![

            Span::styled(format!("  {} ", prompt), Style::default().fg(Color::DarkGray)),
            Span::styled(current_input.to_owned(), Style::default().fg(Color::White)),
            Span::styled(
                "█",
                Style::default()
                    .fg(Color::White)

                    .add_modifier(Modifier::SLOW_BLINK),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(

            "  Examples:",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "    • /home/user/minecraft/servers/my-server",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "    • ./servers/survival-world",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Enter", Style::default().fg(Color::Green)),
            Span::styled(" next   ", Style::default().fg(Color::DarkGray)),

            Span::styled("Esc", Style::default().fg(Color::Red)),
            Span::styled(" cancel", Style::default().fg(Color::DarkGray)),
        ]),
    ]);

    frame.render_widget(Paragraph::new(content).block(block), popup);
}

// ─── Manage Connections Modal ───────────────────────────────────────────────

fn render_manage_connections_modal(app: &App, frame: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 70, area);
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .title(format!(" Manage Connections ({}) ", app.connections.connections.len()))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan));


    if app.connections.connections.is_empty() {
        let content = Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No connections saved.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press 'a' to add a connection.",
                Style::default().fg(Color::Cyan),
            )),
        ]);
        frame.render_widget(Paragraph::new(content).block(block), popup);
        return;
    }

    let items: Vec<ListItem> = app
        .connections
        .connections
        .iter()
        .enumerate()
        .map(|(i, conn)| {
            let exists = if conn.path.exists() {
                Span::styled(" ✓", Style::default().fg(Color::Green))
            } else {

                Span::styled(" ✗", Style::default().fg(Color::Red))

            };
            
            let symlink = if conn.is_symlink {
                Span::styled(" (symlink)", Style::default().fg(Color::Yellow))
            } else {
                Span::styled("", Style::default())
            };


            let line = Line::from(vec![
                Span::styled(format!("  {} ", conn.name), Style::default().fg(Color::White)),
                exists,
                symlink,
            ]);
            
            let style = if i == app.selected_connection {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(line).style(style)
        })
        .collect();


    let list = List::new(items).block(Block::default());
    let mut state = ListState::default();
    state.select(Some(app.selected_connection));
    
    // Fix: Remove the & before Margin
    let inner_area = popup.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(inner_area);

    // Instructions
    let instructions = Paragraph::new(Line::from(vec![
        Span::styled("↑↓", Style::default().fg(Color::Cyan)),
        Span::raw(" select  "),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::raw(" view  "),

        Span::styled("d", Style::default().fg(Color::Red)),

        Span::raw(" delete  "),
        Span::styled("Esc", Style::default().fg(Color::Red)),
        Span::raw(" back"),
    ])).alignment(Alignment::Center);
    

    frame.render_widget(instructions, chunks[0]);
    frame.render_stateful_widget(list, chunks[1], &mut state);
}

// ─── Remove Connection Modal ────────────────────────────────────────────────

fn render_remove_connection_modal(app: &App, selected: usize, frame: &mut Frame, area: Rect) {
    let popup = centered_rect(50, 30, area);
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .title(" Confirm Delete ")

        .title_style(
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        )
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Red));

    let conn = &app.connections.connections[selected];
    let content = Text::from(vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  Remove connection '{}'?", conn.name),
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            format!("  Path: {}", conn.path.display()),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled("  y", Style::default().fg(Color::Green)),
            Span::raw("es   "),
            Span::styled("n", Style::default().fg(Color::Red)),
            Span::raw("o"),
        ]),
    ]);

    frame.render_widget(Paragraph::new(content).block(block), popup);
}

// ─── Helpers ────────────────────────────────────────────────────────────────


fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let pad_v = (100 - percent_y) / 2;
    let pad_h = (100 - percent_x) / 2;


    let vertical = Layout::vertical([
        Constraint::Percentage(pad_v),
        Constraint::Percentage(percent_y),
        Constraint::Percentage(pad_v),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage(pad_h),
        Constraint::Percentage(percent_x),
        Constraint::Percentage(pad_h),
    ])
    .split(vertical[1])[1]
}
