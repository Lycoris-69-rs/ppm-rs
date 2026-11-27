//! The main menu, following the layout sketch: ASCII brand on top, a flat list
//! of actions, and a system overview column on the right.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{App, MENU_ITEMS, MenuItem};
use crate::ui::{self, footer_hints, key_hint, panel_block, spinner, thousands};

const BRAND: [&str; 5] = [
    "██████  ██████  ██    ██",
    "██  ██  ██  ██  ███  ███",
    "██████  ██████  ██ ██ ██",
    "██      ██      ██    ██",
    "██      ██      ██    ██",
];

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(area);

    draw_brand(frame, app, rows[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(rows[1]);

    draw_menu(frame, app, body[0]);
    draw_overview(frame, app, body[1]);

    ui::draw_footer(
        frame,
        app,
        rows[2],
        footer_hints(hints()),
        Some(format!(
            "{} packages in one list",
            thousands(app.data.package_count())
        )),
    );
}

fn hints() -> Vec<Span<'static>> {
    let mut hints = Vec::new();
    hints.extend(key_hint("↑↓", "move"));
    hints.extend(key_hint("Enter", "select"));
    hints.extend(key_hint("R", "reload"));
    hints.extend(key_hint("?", "help"));
    hints.extend(key_hint("q", "quit"));
    hints
}

fn draw_brand(frame: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let art_area = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.min(26),
        height: 5,
    };
    let art_lines: Vec<Line> = BRAND
        .iter()
        .map(|line| Line::from(Span::styled(*line, Style::default())))
        .collect();
    frame.render_widget(Paragraph::new(art_lines).style(Style::default()), art_area);

    let text_area = Rect {
        x: area.x + 28,
        y: area.y + 1,
        width: area.width.saturating_sub(30),
        height: 5,
    };
    let lines = vec![
        Line::from(Span::styled(
            "Powerful Package Manager",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("pacman + AUR", Style::default()),
            Span::styled(
                " · every repository merged into one flat list",
                Style::default(),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                thousands(app.data.package_count()),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(" packages available", Style::default()),
        ]),
        Line::from(vec![
            Span::styled(thousands(app.data.installed_count()), Style::default()),
            Span::styled(" installed", Style::default()),
            Span::styled("  ·  ", Style::default()),
            Span::styled(thousands(app.data.updates.len()), Style::default()),
            Span::styled(" updates", Style::default()),
        ]),
        Line::from(vec![Span::styled(
            format!("downloads: {}", app.paths.downloads_dir.display()),
            Style::default(),
        )]),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default())
            .wrap(Wrap { trim: true }),
        text_area,
    );

    // Thin separator under the brand.
    let separator_y = area.y + 7;
    if separator_y < area.bottom() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "─".repeat(area.width as usize),
                Style::default(),
            ))),
            Rect {
                x: area.x,
                y: separator_y,
                width: area.width,
                height: 1,
            },
        );
    }
}

fn draw_menu(frame: &mut Frame, app: &mut App, area: Rect) {
    let header = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "MAIN MENU",
            Style::default().add_modifier(Modifier::BOLD),
        )))
        .style(Style::default()),
        header,
    );

    let list_area = Rect {
        x: area.x + 1,
        y: area.y + 2,
        width: area.width.saturating_sub(2),
        height: MENU_ITEMS.len() as u16,
    };

    let items: Vec<ListItem> = MENU_ITEMS
        .iter()
        .map(|item| {
            let marker = match item {
                MenuItem::Install => "+",
                MenuItem::Remove => "-",
                MenuItem::Update => "▴",
                MenuItem::Downgrade => "▾",
                MenuItem::CleanOrphans => "x",
                MenuItem::CleanCache => "◫",
                MenuItem::SystemTools => "⌂",
                MenuItem::Exit => "⏻",
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {marker}  "),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{:<22}", item.title()), Style::default()),
                Span::styled(item.description(), Style::default()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol("▸");

    let mut state = ListState::default();
    state.select(Some(app.menu_index));
    frame.render_stateful_widget(list, list_area, &mut state);
}

fn draw_overview(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel_block(" SYSTEM OVERVIEW ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let system = app.data.system.as_ref();
    let mut lines = Vec::new();

    let pacman_version = system
        .map(|info| info.pacman_version.clone())
        .unwrap_or_else(|| format!("{} reading…", spinner(app)));
    lines.push(kv("pacman", &pacman_version));
    let helper = system
        .and_then(|info| info.aur_helper.clone())
        .unwrap_or_else(|| "—".to_string());
    lines.push(kv("AUR helper", &helper));
    let root = system
        .map(|info| if info.is_root { "yes" } else { "no" })
        .unwrap_or("—");
    lines.push(kv("running as root", root));
    lines.push(Line::from(""));
    lines.push(kv(
        "package cache",
        &crate::model::format_bytes(app.data.cache_size),
    ));
    if app.data.cache_loaded {
        lines.push(kv(
            "cached versions",
            &thousands(app.data.cache.len()).to_string(),
        ));
    }
    if app.data.loading_aur {
        lines.push(kv("AUR list", &format!("{} fetching…", spinner(app))));
    } else if app.data.aur_loaded {
        lines.push(kv(
            "AUR list",
            &format!("{} packages", thousands(app.data.aur_names.len())),
        ));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "TIP",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "Install opens the flat package browser: type to search, Enter to install, Ctrl+D to only download.",
        Style::default().add_modifier(Modifier::DIM),
    )));
    lines.push(Line::from(""));
    let missing = if app.data.loading_pacman || app.data.loading_aur {
        format!("{} loading package lists…", spinner(app))
    } else {
        "ready".to_string()
    };
    lines.push(Line::from(vec![
        Span::styled("state  ", Style::default().add_modifier(Modifier::DIM)),
        Span::styled(missing, Style::default()),
    ]));

    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default())
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn kv(key: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key:<18}"),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(value.to_string(), Style::default()),
    ])
}
