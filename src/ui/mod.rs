//! Rendering: a small design system shared by every screen.

pub mod browse;
pub mod info;
pub mod menu;
pub mod overlay;
pub mod simple;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::{App, BrowseMode, Screen};

pub const BG: Color = Color::Rgb(13, 15, 22);
pub const PANEL: Color = Color::Rgb(20, 24, 34);
pub const PANEL_HIGHLIGHT: Color = Color::Rgb(32, 40, 58);
pub const ACCENT: Color = Color::Rgb(72, 224, 206);
pub const ACCENT2: Color = Color::Rgb(190, 146, 255);
pub const FG: Color = Color::Rgb(219, 226, 240);
pub const DIM: Color = Color::Rgb(118, 130, 156);
pub const FAINT: Color = Color::Rgb(74, 84, 105);
pub const GREEN: Color = Color::Rgb(116, 226, 148);
pub const YELLOW: Color = Color::Rgb(242, 201, 108);
pub const RED: Color = Color::Rgb(244, 116, 116);
pub const BLUE: Color = Color::Rgb(120, 176, 255);

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(app: &App) -> &'static str {
    SPINNER[(app.spinner as usize) % SPINNER.len()]
}

pub fn base_style() -> Style {
    Style::default().bg(BG).fg(FG)
}

pub fn panel_block(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(FAINT))
        .style(Style::default().bg(PANEL))
        .title(title)
        .title_style(Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD))
}

pub fn dim_style() -> Style {
    Style::default().fg(DIM)
}

pub fn key_hint(key: &'static str, description: &'static str) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            key,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {description}"), Style::default().fg(DIM)),
        Span::styled("   ", Style::default()),
    ]
}

pub fn footer_hints(hints: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(" ", Style::default())];
    spans.extend(hints);
    Line::from(spans)
}

/// Draw the global background and dispatch to the active screen.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(base_style()), area);

    if area.width < 60 || area.height < 16 {
        let message = Paragraph::new(vec![
            Line::from(Span::styled(
                "Terminal too small",
                Style::default().fg(RED).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "PPM needs at least 60x16 characters.",
                dim_style(),
            )),
            Line::from(Span::styled(
                "Resize the window or press Ctrl+C to quit.",
                dim_style(),
            )),
        ])
        .style(base_style());
        frame.render_widget(message, area);
        return;
    }

    match app.screen {
        Screen::MainMenu => menu::draw(frame, app, area),
        Screen::Browse => browse::draw(frame, app, area),
        Screen::Orphans => simple::draw_orphans(frame, app, area),
        Screen::CacheClean => simple::draw_cache(frame, app, area),
        Screen::SystemTools => simple::draw_system(frame, app, area),
    }

    if app.help_visible {
        overlay::draw_help(frame, app, area);
    }
    if app.confirm.is_some() {
        overlay::draw_confirm(frame, app, area);
    }
    if app.overlay.is_some() {
        overlay::draw_operation(frame, app, area);
    }
}

/// Shared two-line footer with key hints, loading state and toasts.
pub fn draw_footer(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    hints: Line<'static>,
    extra_right: Option<String>,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    let hints_area = Rect {
        x: area.x + 1,
        y: rows[0].y,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(hints)
            .style(Style::default().bg(PANEL))
            .alignment(ratatui::layout::Alignment::Left),
        Rect {
            y: rows[0].y,
            height: 1,
            ..area
        },
    );
    let _ = hints_area;

    let left = status_summary(app);
    let right = app
        .toast
        .as_ref()
        .map(|toast| (toast.text.clone(), if toast.error { RED } else { ACCENT }))
        .or_else(|| extra_right.map(|text| (text, DIM)));

    let status_line = Line::from(vec![Span::styled(
        format!(" {left}"),
        Style::default().fg(DIM),
    )]);
    frame.render_widget(
        Paragraph::new(status_line)
            .style(Style::default().bg(PANEL))
            .alignment(ratatui::layout::Alignment::Left),
        Rect {
            y: rows[1].y,
            height: 1,
            ..area
        },
    );
    if let Some((text, color)) = right {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("{text} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )))
            .style(Style::default().bg(PANEL))
            .alignment(ratatui::layout::Alignment::Right),
            Rect {
                y: rows[1].y,
                height: 1,
                ..area
            },
        );
    }
}

pub fn status_summary(app: &App) -> String {
    let data = &app.data;
    let mut parts = Vec::new();
    if data.aur_updates_pending {
        parts.push(format!("{} checking AUR updates", spinner(app)));
    }
    if data.loading_pacman {
        parts.push(format!("{} pacman packages", spinner(app)));
    } else if data.pacman_loaded {
        parts.push(format!("pacman {}", thousands(data.pacman_packages.len())));
    }
    if data.loading_aur {
        parts.push(format!("{} AUR list", spinner(app)));
    } else if data.aur_loaded {
        parts.push(format!("AUR {}", thousands(data.aur_names.len())));
    }
    if data.installed_loaded {
        parts.push(format!("installed {}", thousands(data.installed.len())));
    }
    if data.updates_loaded {
        parts.push(format!("updates {}", thousands(data.updates.len())));
    }
    if data.cache_size > 0 {
        parts.push(format!(
            "cache {}",
            crate::model::format_bytes(data.cache_size)
        ));
    }
    if parts.is_empty() {
        format!("{} loading package data", spinner(app))
    } else {
        parts.join("  ·  ")
    }
}

pub fn thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(character);
    }
    out
}

pub fn centered_rect(width_percent: u16, height_percent: u16, area: Rect) -> Rect {
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_percent) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100 - width_percent) / 2),
        ])
        .split(area);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_percent) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100 - height_percent) / 2),
        ])
        .split(horizontal[1]);
    vertical[1]
}

/// Small colored tag used in lists and the info pane.
pub fn source_tag(source: crate::model::Source) -> Span<'static> {
    match source {
        crate::model::Source::Aur => Span::styled("aur", Style::default().fg(ACCENT2)),
        crate::model::Source::Pacman => Span::styled("   ", Style::default()),
    }
}

pub fn mode_hints(mode: BrowseMode) -> Vec<Span<'static>> {
    let mut hints = Vec::new();
    match mode {
        BrowseMode::Install => {
            hints.extend(key_hint("Enter", "install"));
            hints.extend(key_hint("^D", "download"));
            hints.extend(key_hint("Space", "mark"));
        }
        BrowseMode::Remove => {
            hints.extend(key_hint("Enter", "remove"));
            hints.extend(key_hint("Space", "mark"));
        }
        BrowseMode::Update => {
            hints.extend(key_hint("Enter", "upgrade"));
            hints.extend(key_hint("^D", "download"));
            hints.extend(key_hint("Space", "mark"));
        }
        BrowseMode::Downgrade => {
            hints.extend(key_hint("Enter", "reinstall cached"));
            hints.extend(key_hint("Space", "mark"));
        }
    }
    hints.extend(key_hint("↑↓", "move"));
    hints.extend(key_hint("Type", "search"));
    hints.extend(key_hint("Esc", "back"));
    hints.extend(key_hint("F1", "help"));
    hints
}
