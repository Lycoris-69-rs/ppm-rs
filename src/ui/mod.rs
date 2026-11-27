//! Rendering: a small design system shared by every screen.
//!
//! PPM is deliberately monochrome: the palette below maps every colour role to
//! the terminal's *default* foreground/background, and no widget paints a
//! background. Hierarchy comes from bold/dim modifiers and from glyphs only.

pub mod browse;
pub mod info;
pub mod menu;
pub mod overlay;
pub mod simple;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::{App, BrowseMode, Screen};

// PPM uses no palette: text is drawn with the terminal's default foreground on
// its default background.  Emphasis is expressed with the bold/dim modifiers.

/// Bold text — the only emphasis used by the interface.
pub fn emphasis() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// De-emphasised (dim) text for hints and secondary metadata.
pub fn secondary() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(app: &App) -> &'static str {
    SPINNER[(app.spinner as usize) % SPINNER.len()]
}

pub fn base_style() -> Style {
    Style::default()
}

pub fn panel_block(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default())
        .style(Style::default())
        .title(title)
        .title_style(Style::default().add_modifier(Modifier::BOLD))
}

pub fn dim_style() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

pub fn key_hint(key: &'static str, description: &'static str) -> Vec<Span<'static>> {
    vec![
        Span::styled(key, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {description}"), Style::default()),
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
                Style::default().add_modifier(Modifier::BOLD),
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

    frame.render_widget(
        Paragraph::new(hints).alignment(ratatui::layout::Alignment::Left),
        Rect {
            y: rows[0].y,
            height: 1,
            ..area
        },
    );

    let left = status_summary(app);
    let right = app
        .toast
        .as_ref()
        .map(|toast| toast.text.clone())
        .or(extra_right);

    let status_line = Line::from(vec![Span::styled(format!(" {left}"), secondary())]);
    frame.render_widget(
        Paragraph::new(status_line).alignment(ratatui::layout::Alignment::Left),
        Rect {
            y: rows[1].y,
            height: 1,
            ..area
        },
    );
    if let Some(text) = right {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!("{text} "), emphasis())))
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
        crate::model::Source::Aur => {
            Span::styled("aur", Style::default().add_modifier(Modifier::DIM))
        }
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
