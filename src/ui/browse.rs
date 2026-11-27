//! The flat package browser: search field on top, package list on the left,
//! package information on the right.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::app::{App, BrowseMode, Row};
use crate::model::Status;
use crate::ui::{
    self, ACCENT, ACCENT2, BG, BLUE, DIM, FAINT, FG, GREEN, PANEL_HIGHLIGHT, YELLOW, footer_hints,
    mode_hints, panel_block, spinner, thousands,
};

const VERSION_WIDTH: usize = 14;
const STATE_WIDTH: usize = 10;
const TAG_WIDTH: usize = 4;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);

    draw_header(frame, app, rows[0]);
    draw_search(frame, app, rows[1]);
    draw_separator(frame, rows[2]);

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(rows[3]);

    draw_list(frame, app, main[0]);
    crate::ui::info::draw(frame, app, main[1]);

    let hints = footer_hints(mode_hints(app.browse.mode));
    let extra = if app.browse.marked.is_empty() {
        None
    } else {
        Some(format!("{} marked", app.browse.marked.len()))
    };
    ui::draw_footer(frame, app, rows[4], hints, extra);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let title = format!(" {} ", app.browse.mode.title());
    let left = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{} shown / {} total",
                thousands(app.browse.filtered.len()),
                thousands(app.browse.rows.len())
            ),
            Style::default().fg(DIM),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(left).style(Style::default().bg(BG)),
        Rect { height: 1, ..area },
    );

    let mut right = Vec::new();
    if app.browse.loading {
        right.push(Span::styled(
            format!("{} loading  ", spinner(app)),
            Style::default().fg(YELLOW),
        ));
    }
    if app.data.loading_aur {
        right.push(Span::styled(
            format!("{} AUR  ", spinner(app)),
            Style::default().fg(DIM),
        ));
    }
    if app.data.aur_updates_pending {
        right.push(Span::styled(
            format!("{} AUR updates  ", spinner(app)),
            Style::default().fg(DIM),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(right))
            .style(Style::default().bg(BG))
            .alignment(Alignment::Right),
        Rect { height: 1, ..area },
    );
}

fn draw_search(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::styled(
        "  > ",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )];
    if app.browse.filter.is_empty() {
        spans.push(Span::styled(
            app.browse.mode.search_hint().to_string(),
            Style::default().fg(FAINT),
        ));
    } else {
        spans.push(Span::styled(
            app.browse.filter.clone(),
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ));
    }
    let cursor = if (app.spinner / 3).is_multiple_of(2) {
        "▏"
    } else {
        " "
    };
    spans.push(Span::styled(
        cursor,
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(BG)),
        Rect { height: 1, ..area },
    );

    let hint = if app.browse.filter.is_empty() {
        String::new()
    } else {
        format!("{} matches ", thousands(app.browse.filtered.len()))
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(DIM))))
            .style(Style::default().bg(BG))
            .alignment(Alignment::Right),
        Rect { height: 1, ..area },
    );
}

fn draw_separator(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(FAINT),
        )))
        .style(Style::default().bg(BG)),
        area,
    );
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = format!(" PACKAGES · {} ", thousands(app.browse.filtered.len()));
    let block = panel_block(Line::from(Span::styled(
        title,
        Style::default().fg(ACCENT2).add_modifier(Modifier::BOLD),
    )));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 || inner.width < 20 {
        return;
    }

    let header = column_line(
        "PACKAGE",
        "VERSION",
        "STATE",
        None,
        inner.width,
        Style::default().fg(FAINT),
    );
    frame.render_widget(
        Paragraph::new(header).style(Style::default().bg(ui::PANEL)),
        Rect {
            y: inner.y,
            height: 1,
            ..inner
        },
    );

    let body = Rect {
        y: inner.y + 1,
        height: inner.height - 1,
        ..inner
    };
    let visible = body.height as usize;

    if app.browse.filtered.is_empty() {
        let message = if app.browse.loading {
            format!("{} loading packages…", spinner(app))
        } else if app.browse.filter.is_empty() {
            match app.browse.mode {
                BrowseMode::Update => {
                    "No updates available — everything is up to date.".to_string()
                }
                BrowseMode::Downgrade => {
                    "No cached packages found (the cache is empty or unreadable).".to_string()
                }
                BrowseMode::Remove => "No installed packages matched.".to_string(),
                BrowseMode::Install => "Package list is empty.".to_string(),
            }
        } else {
            format!("No package matches “{}”.", app.browse.filter)
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(message, Style::default().fg(DIM))))
                .style(Style::default().bg(ui::PANEL))
                .alignment(Alignment::Center),
            Rect {
                y: body.y + body.height / 3,
                height: 1,
                ..body
            },
        );
        return;
    }

    if app.browse.selected < app.browse.offset {
        app.browse.offset = app.browse.selected;
    } else if app.browse.selected >= app.browse.offset + visible {
        app.browse.offset = app.browse.selected + 1 - visible;
    }

    for slot in 0..visible {
        let index = app.browse.offset + slot;
        let Some(row_index) = app.browse.filtered.get(index).copied() else {
            break;
        };
        let Some(row) = app.browse.rows.get(row_index).cloned() else {
            continue;
        };
        let selected = index == app.browse.selected;
        let marked = app.browse.marked.contains(&row.name);
        let line = row_line(app, &row, selected, marked, inner.width);
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(ui::PANEL)),
            Rect {
                x: body.x,
                y: body.y + slot as u16,
                width: body.width,
                height: 1,
            },
        );
    }

    let total = app.browse.filtered.len();
    let mut scrollbar_state =
        ScrollbarState::new(total.saturating_sub(visible)).position(app.browse.offset);
    let scroll_area = Rect {
        x: area.x + area.width.saturating_sub(1),
        y: area.y + 1,
        width: 1,
        height: area.height.saturating_sub(2),
    };
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .track_style(Style::default().fg(FAINT))
            .thumb_symbol("┃")
            .thumb_style(Style::default().fg(ACCENT2)),
        scroll_area,
        &mut scrollbar_state,
    );
}

fn row_line(app: &App, row: &Row, selected: bool, marked: bool, width: u16) -> Line<'static> {
    let version = app.row_display_version(row);
    let status = app.row_status(row);
    let (state_label, state_color) = short_status(status);
    let name_style = match status {
        Status::Installed => Style::default().fg(GREEN),
        Status::Upgrade => Style::default().fg(YELLOW),
        Status::Newer => Style::default().fg(BLUE),
        Status::NotInstalled => Style::default().fg(FG),
        Status::Unknown => Style::default().fg(DIM),
    };
    let marker = if selected {
        Span::styled("▸", Style::default().fg(ACCENT))
    } else if marked {
        Span::styled("●", Style::default().fg(ACCENT2))
    } else {
        Span::styled(" ", Style::default())
    };
    let name_width = width as usize - 1 - TAG_WIDTH - VERSION_WIDTH - STATE_WIDTH - 2;
    let name = fit_left(&row.name, name_width.saturating_sub(1).max(4));
    let mut line = Line::from(vec![
        marker,
        Span::styled(
            format!(
                " {name:<width$}",
                width = name_width.saturating_sub(1).max(4)
            ),
            if selected {
                name_style.add_modifier(Modifier::BOLD)
            } else {
                name_style
            },
        ),
        ui::source_tag(row.source),
        Span::styled(
            format!("{:>VERSION_WIDTH$}", fit_right(&version, VERSION_WIDTH)),
            Style::default().fg(DIM),
        ),
        Span::styled(
            format!(" {:>STATE_WIDTH$}", state_label),
            Style::default().fg(state_color),
        ),
    ]);
    if selected {
        line = line.style(Style::default().bg(PANEL_HIGHLIGHT));
    }
    line
}

fn column_line(
    name: &str,
    version: &str,
    state: &str,
    tag: Option<&str>,
    width: u16,
    style: Style,
) -> Line<'static> {
    let name_width = width as usize - 1 - TAG_WIDTH - VERSION_WIDTH - STATE_WIDTH - 2;
    let tag = tag.unwrap_or("   ");
    Line::from(vec![
        Span::styled(" ", style),
        Span::styled(
            format!(
                "{name:<width$}",
                width = name_width.saturating_sub(1).max(4)
            ),
            style.add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{tag:<TAG_WIDTH$}"), style),
        Span::styled(
            format!("{:>VERSION_WIDTH$}", version),
            style.add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {:>STATE_WIDTH$}", state),
            style.add_modifier(Modifier::BOLD),
        ),
    ])
}

fn short_status(status: Status) -> (&'static str, ratatui::style::Color) {
    match status {
        Status::Installed => ("installed", GREEN),
        Status::Upgrade => ("upgrade", YELLOW),
        Status::Newer => ("newer", BLUE),
        Status::NotInstalled => ("", DIM),
        Status::Unknown => ("…", DIM),
    }
}

fn fit_left(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

fn fit_right(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        text.to_string()
    } else {
        text.chars().skip(count - width).collect()
    }
}
