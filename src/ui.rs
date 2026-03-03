use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::{
    app::{App, AppMode},
    server::ServerStatus,
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

    if let AppMode::Installing { input } = &app.mode {
        render_install_modal(input, frame, area);
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
                "  Create a directory:",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  servers/<name>/",
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

// ─── Detail Panel ───────────────────────────────────────────────────────────

fn render_detail(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::bordered()
        .title(" Server Details ")
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));

    if app.servers.is_empty() {
        let text = Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Select a server to see details.",
                Style::default().fg(Color::DarkGray),
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

    let label_style = Style::default().fg(Color::DarkGray);
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Name:    ", label_style),
            Span::styled(
                server.name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Path:    ", label_style),
            Span::styled(
                server.path.to_string_lossy().to_string(),
                Style::default().fg(Color::Cyan),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Status:  ", label_style),
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
                kb(" i ", "install"),
                Span::raw("  "),
                kb(" r ", "refresh"),
            ]),
            AppMode::Installing { .. } => Line::from(vec![
                kb(" Enter ", "confirm"),
                Span::raw("  "),
                kb(" Esc ", "cancel"),
            ]),
        }
    };

    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Color::DarkGray)),
        area,
    );
}

fn kb(key: &str, label: &str) -> Span<'static> {
    Span::raw(format!(
        "{}{}",
        key.to_owned(),
        label.to_owned()
    ))
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
