use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, SYSTEM_ACTIONS, SystemAction};
use crate::model::format_bytes;
use crate::ui::{self, footer_hints, key_hint, panel_block, spinner, thousands};

fn centered_row(area: Rect, height: u16) -> Rect {
    let height = height.min(area.height);
    Rect {
        x: area.x,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width: area.width,
        height,
    }
}

fn screen_frame(area: Rect) -> [Rect; 3] {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);
    [rows[0], rows[1], rows[2]]
}

fn title_line(title: &str, subtitle: &str) -> Paragraph<'static> {
    Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            title.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {subtitle}"),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ]))
    .style(Style::default())
}

pub fn draw_orphans(frame: &mut Frame, app: &App, area: Rect) {
    let [header, body, footer] = screen_frame(area);
    let count = app.data.orphans.len();
    frame.render_widget(
        title_line("CLEAN ORPHANS", &format!("{count} orphaned package(s)")),
        header,
    );

    let list_height = if app.data.orphans.is_empty() {
        5
    } else {
        (2 + app.data.orphans.len().min(10) + 2) as u16
    };
    let rect = centered_row(body, list_height);
    let block = panel_block(" ORPHANS ");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();
    if app.data.loading_orphans || !app.data.orphan_loaded {
        lines.push(Line::from(Span::styled(
            format!("{} scanning for orphaned packages…", spinner(app)),
            Style::default(),
        )));
    } else if app.data.orphans.is_empty() {
        lines.push(Line::from(Span::styled(
            "No orphaned packages — the system is clean.",
            Style::default(),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "Packages nothing depends on. Removing them frees disk space:",
            Style::default(),
        )));
        lines.push(Line::from(""));
        let visible = inner.height.saturating_sub(4) as usize;
        for name in app.data.orphans.iter().take(visible) {
            lines.push(Line::from(vec![
                Span::styled("  ● ", Style::default()),
                Span::styled(name.clone(), Style::default()),
            ]));
        }
        if app.data.orphans.len() > visible {
            lines.push(Line::from(Span::styled(
                format!("  … and {} more", app.data.orphans.len() - visible),
                Style::default(),
            )));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default())
            .alignment(Alignment::Left),
        inner,
    );

    let mut hints = Vec::new();
    hints.extend(key_hint("Enter", "remove all orphans"));
    hints.extend(key_hint("r", "rescan"));
    hints.extend(key_hint("Esc", "back"));
    ui::draw_footer(frame, app, footer, footer_hints(hints), None);
}

pub fn draw_cache(frame: &mut Frame, app: &App, area: Rect) {
    let [header, body, footer] = screen_frame(area);
    frame.render_widget(
        title_line(
            "CLEAN PACKAGE CACHE",
            &format!(
                "{} in {} tarballs",
                format_bytes(app.data.cache_size),
                thousands(app.data.cache.len())
            ),
        ),
        header,
    );

    let rect = centered_row(body, 11);
    let block = panel_block(" PACMAN CACHE ");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let cache_dirs: Vec<String> = app
        .data
        .system
        .as_ref()
        .map(|info| {
            info.cache_dirs
                .iter()
                .map(|dir| dir.display().to_string())
                .collect()
        })
        .unwrap_or_else(|| vec!["/var/cache/pacman/pkg".to_string()]);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Cache size    ", Style::default()),
            Span::styled(": ", Style::default()),
            Span::styled(
                format_bytes(app.data.cache_size),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Cached files  ", Style::default()),
            Span::styled(": ", Style::default()),
            Span::styled(thousands(app.data.cache.len()), Style::default()),
        ]),
        Line::from(vec![
            Span::styled("Location      ", Style::default()),
            Span::styled(": ", Style::default()),
            Span::styled(cache_dirs.join(", "), Style::default()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Cleaning removes cached tarballs of packages that are no longer installed.",
            Style::default(),
        )),
        Line::from(Span::styled(
            "Installed packages are not touched, but downgrades may stop being possible.",
            Style::default(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Command  ", Style::default()),
            Span::styled("sudo pacman -Sc --noconfirm", Style::default()),
        ]),
    ];
    if app.data.cache.is_empty() && !app.data.cache_loaded {
        lines.insert(
            0,
            Line::from(Span::styled(
                format!("{} scanning cache…", spinner(app)),
                Style::default(),
            )),
        );
    }
    frame.render_widget(Paragraph::new(lines).style(Style::default()), inner);

    let mut hints = Vec::new();
    hints.extend(key_hint("Enter", "clean cache"));
    hints.extend(key_hint("r", "rescan"));
    hints.extend(key_hint("Esc", "back"));
    ui::draw_footer(frame, app, footer, footer_hints(hints), None);
}

pub fn draw_system(frame: &mut Frame, app: &App, area: Rect) {
    let [header, body, footer] = screen_frame(area);
    frame.render_widget(
        title_line("SYSTEM TOOLS", "pacman diagnostics and maintenance"),
        header,
    );

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
        .split(body);

    draw_system_info(frame, app, columns[0]);
    draw_system_actions(frame, app, columns[1]);

    let mut hints = Vec::new();
    hints.extend(key_hint("↑↓", "choose"));
    hints.extend(key_hint("Enter", "run action"));
    hints.extend(key_hint("Esc", "back"));
    ui::draw_footer(frame, app, footer, footer_hints(hints), None);
}

fn draw_system_info(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel_block(" SYSTEM ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::new();
    match &app.data.system {
        None => lines.push(Line::from(Span::styled(
            format!("{} reading system information…", spinner(app)),
            Style::default(),
        ))),
        Some(info) => {
            lines.push(kv("pacman", &info.pacman_version));
            lines.push(kv("database", info.db_path.as_deref().unwrap_or("—")));
            lines.push(kv("config", info.config_file.as_deref().unwrap_or("—")));
            lines.push(kv(
                "cache",
                &info
                    .cache_dirs
                    .iter()
                    .map(|dir| dir.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
            lines.push(kv(
                "AUR helper",
                info.aur_helper.as_deref().unwrap_or("not installed"),
            ));
            lines.push(kv("root", if info.is_root { "yes" } else { "no" }));
            lines.push(kv(
                "AUR list cache",
                &crate::repo::aur::cache_path(&app.paths.cache_dir)
                    .display()
                    .to_string(),
            ));
            lines.push(kv(
                "downloads",
                &app.paths.downloads_dir.display().to_string(),
            ));
            lines.push(Line::from(""));
            lines.push(kv("packages", &thousands(app.data.package_count())));
            lines.push(kv("installed", &thousands(app.data.installed.len())));
            lines.push(kv("updates", &thousands(app.data.updates.len())));
            lines.push(kv("cache size", &format_bytes(app.data.cache_size)));
        }
    }
    frame.render_widget(Paragraph::new(lines).style(Style::default()), inner);
}

fn draw_system_actions(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel_block(" MAINTENANCE ACTIONS ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::new();
    for (index, action) in SYSTEM_ACTIONS.iter().enumerate() {
        let selected = index == app.system_action_index;
        lines.push(action_line(*action, selected, inner.width as usize));
    }
    lines.push(Line::from(""));
    if let Some(selected) = SYSTEM_ACTIONS.get(app.system_action_index) {
        lines.push(Line::from(Span::styled(
            selected.description(),
            Style::default().add_modifier(Modifier::DIM),
        )));
    }
    frame.render_widget(Paragraph::new(lines).style(Style::default()), inner);
}

fn action_line(action: SystemAction, selected: bool, width: usize) -> Line<'static> {
    let (title, description) = match action {
        SystemAction::RefreshDatabases => (action.title(), "pacman -Sy"),
        SystemAction::FullUpgrade => (action.title(), "pacman -Syu"),
        SystemAction::UpdateAur => (action.title(), "yay -Sua"),
        SystemAction::ReloadData => (action.title(), "re-read databases"),
        SystemAction::ClearAurCache => (action.title(), "force refresh"),
    };
    let marker = if selected { "▸ " } else { "  " };
    let title_style = if selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let title_width = width.saturating_sub(24).max(16);
    let mut line = Line::from(vec![
        Span::styled(marker, Style::default()),
        Span::styled(format!("{title:<title_width$}"), title_style),
        Span::styled(description, Style::default().add_modifier(Modifier::DIM)),
    ]);
    if selected {
        line = line.style(Style::default().add_modifier(Modifier::BOLD));
    }
    line
}

fn kv(key: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key:<15}"),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(": ", Style::default().add_modifier(Modifier::DIM)),
        Span::styled(value.to_string(), Style::default()),
    ])
}
