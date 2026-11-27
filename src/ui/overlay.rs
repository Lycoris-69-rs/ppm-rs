use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, Operation};
use crate::model::format_bytes;
use crate::ui;

fn overlay_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4)).max(20);
    let height = height.min(area.height.saturating_sub(2)).max(5);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn modal_block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default())
        .title(Line::from(Span::styled(
            format!(" {title} "),
            Style::default().add_modifier(Modifier::BOLD),
        )))
        .title_alignment(Alignment::Left)
}

pub fn draw_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let Some(dialog) = &app.confirm else {
        return;
    };
    let width = area.width.saturating_sub(8).min(94);
    let mut body: Vec<Line> = Vec::new();
    body.push(Line::from(Span::styled(
        dialog.title.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    body.push(Line::from(""));
    for item in &dialog.summary {
        body.push(Line::from(vec![
            Span::styled("  • ", Style::default()),
            Span::styled(item.clone(), Style::default()),
        ]));
    }
    body.push(Line::from(""));
    body.push(Line::from(Span::styled(
        "COMMANDS",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    for command in &dialog.commands {
        body.push(Line::from(Span::styled(
            format!("  $ {command}"),
            Style::default(),
        )));
    }
    body.push(Line::from(""));
    body.push(Line::from(vec![
        Span::styled("[Y/Enter]", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(" run   ", Style::default()),
        Span::styled("[N/Esc]", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(" cancel", Style::default()),
    ]));

    let text_width = width.saturating_sub(4) as usize;
    let wrapped_height: usize = body
        .iter()
        .map(|line| {
            let length = line.width();
            (length / text_width.max(1)) + 1
        })
        .sum();
    let height = (wrapped_height as u16 + 2).clamp(6, area.height.saturating_sub(2));
    let rect = overlay_rect(area, width, height);

    frame.render_widget(Clear, rect);
    frame.render_widget(modal_block("CONFIRM"), rect);
    let inner = Rect {
        x: rect.x + 2,
        y: rect.y + 1,
        width: rect.width.saturating_sub(4),
        height: rect.height.saturating_sub(2),
    };
    frame.render_widget(
        Paragraph::new(body)
            .style(Style::default())
            .wrap(Wrap { trim: false }),
        inner,
    );
}

pub fn draw_operation(frame: &mut Frame, app: &App, area: Rect) {
    let Some(operation) = &app.overlay else {
        return;
    };
    let rect = overlay_rect(
        area,
        area.width.saturating_sub(10).min(100),
        (area.height * 72 / 100).max(10),
    );
    frame.render_widget(Clear, rect);
    let title = truncate(&operation.title, rect.width.saturating_sub(14) as usize);
    frame.render_widget(modal_block(&format!("OPERATION · {title}")), rect);

    let inner = Rect {
        x: rect.x + 2,
        y: rect.y + 1,
        width: rect.width.saturating_sub(4),
        height: rect.height.saturating_sub(2),
    };
    if inner.height < 3 {
        return;
    }

    let has_progress = operation.progress.is_some();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(if has_progress { 1 } else { 0 }),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    draw_operation_status(frame, operation, chunks[0]);
    if let Some(progress) = &operation.progress {
        draw_progress(frame, progress, chunks[1]);
    }
    draw_operation_log(frame, operation, chunks[2]);
    draw_operation_hints(frame, operation, chunks[3]);
}

fn draw_operation_status(frame: &mut Frame, operation: &Operation, area: Rect) {
    let (icon, text) = if operation.running {
        (spinner_from_state(operation), operation.status.clone())
    } else {
        match operation.result {
            Some(true) => ("✓", format!("done — {}", operation.status)),
            Some(false) => ("×", operation.status.clone()),
            None => ("·", operation.status.clone()),
        }
    };
    let line = Line::from(vec![
        Span::styled(format!(" {icon} "), Style::default()),
        Span::styled(text, Style::default().add_modifier(Modifier::BOLD)),
    ]);
    frame.render_widget(Paragraph::new(line).style(Style::default()), area);
}

fn spinner_from_state(operation: &Operation) -> &'static str {
    let index = operation.lines.len() % ui::SPINNER.len();
    ui::SPINNER[index]
}

fn progress_label(progress: &crate::event::Progress) -> String {
    let percent = (progress.fraction() * 100.0).round() as u64;
    let speed = progress
        .speed_bps
        .map(|bps| format!("  ·  {}/s", format_bytes(bps.max(0.0) as u64)))
        .unwrap_or_default();
    let total = progress
        .total
        .map(|total| format!(" / {}", format_bytes(total)))
        .unwrap_or_default();
    format!(
        " {percent:>3}%  {}{total}{speed}  ·  {}  ({}/{}) ",
        format_bytes(progress.done),
        truncate(&progress.file, 36),
        progress.files_done + 1,
        progress.files_total
    )
}

fn draw_progress(frame: &mut Frame, progress: &crate::event::Progress, area: Rect) {
    if area.width == 0 {
        return;
    }
    let width = area.width as usize;
    let bar_width = 20.min(width.saturating_sub(10)).max(1);
    let filled = ((progress.fraction() * bar_width as f64).round() as usize).min(bar_width);
    let mut cells = vec![' '; bar_width];
    for cell in cells.iter_mut().take(filled) {
        *cell = '=';
    }
    if filled > 0 && filled < bar_width {
        cells[filled] = '>';
    }
    let bar: String = cells.into_iter().collect();
    let mut text = format!(" [{bar}] {}", progress_label(progress).trim());
    if text.chars().count() > width {
        text = text.chars().take(width).collect();
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().add_modifier(Modifier::BOLD),
        ))),
        area,
    );
}

fn draw_operation_log(frame: &mut Frame, operation: &Operation, area: Rect) {
    let visible = area.height as usize;
    let start = operation.lines.len().saturating_sub(visible);
    let width = area.width as usize;
    let lines: Vec<Line> = operation.lines[start..]
        .iter()
        .map(|(text, error)| {
            let style = if *error {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(Span::styled(
                format!(
                    " {:<width$}",
                    truncate(text, width.saturating_sub(1)),
                    width = width.saturating_sub(1)
                ),
                style,
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).style(Style::default()), area);
}

fn draw_operation_hints(frame: &mut Frame, operation: &Operation, area: Rect) {
    let hints: Vec<Span> = if operation.running {
        vec![
            Span::styled(" [Esc]", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(" cancel", Style::default()),
        ]
    } else {
        vec![
            Span::styled(" [Enter]", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(" close", Style::default()),
        ]
    };
    frame.render_widget(
        Paragraph::new(Line::from(hints))
            .style(Style::default())
            .alignment(Alignment::Right),
        area,
    );
}

pub fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let section = |title: &'static str, lines: &mut Vec<Line<'static>>| {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        )));
    };

    lines.push(Line::from(Span::styled(
        "PPM · Powerful Package Manager",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "Every pacman repository and the AUR in one flat searchable list.",
        Style::default(),
    )));

    section("GLOBAL", &mut lines);
    for (key, description) in [
        ("↑ ↓ / j k", "move the selection"),
        ("Enter", "select / confirm"),
        ("Esc", "step back (clears the search first)"),
        ("F1 or ?", "show this help"),
        ("Ctrl+C", "quit PPM"),
    ] {
        lines.push(help_line(key, description));
    }

    section("PACKAGE BROWSER", &mut lines);
    for (key, description) in [
        ("type", "filter the flat package list"),
        ("Backspace", "delete a character"),
        ("Ctrl+U", "clear the search field"),
        ("Ctrl+D", "download the selected/marked packages only"),
        ("Space", "mark a package for a batch action"),
        ("Enter", "install / remove / upgrade / reinstall"),
        ("Ctrl+R", "reload package data"),
    ] {
        lines.push(help_line(key, description));
    }

    section("NOTES", &mut lines);
    for (label, description) in [
        (
            "downloads",
            "package tarballs and AUR snapshots are saved to the downloads directory",
        ),
        (
            "sudo",
            "system changes run through `sudo -n`; run `sudo -v` in a terminal first (or start PPM as root) to cache credentials",
        ),
        (
            "AUR",
            "AUR packages are built and installed with yay or paru, which need sudo credentials as well",
        ),
        (
            "flat list",
            "repositories are merged on purpose: only per-package metadata mentions the AUR",
        ),
    ] {
        lines.push(help_line(label, description));
    }

    let width = area.width.saturating_sub(10).min(104);
    let height = (area.height * 90 / 100)
        .max(lines.len() as u16 + 2)
        .min(area.height.saturating_sub(2));
    let rect = overlay_rect(area, width, height);
    frame.render_widget(Clear, rect);
    frame.render_widget(modal_block("HELP"), rect);
    let inner = Rect {
        x: rect.x + 2,
        y: rect.y + 1,
        width: rect.width.saturating_sub(4),
        height: rect.height.saturating_sub(2),
    };
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default())
            .wrap(Wrap { trim: false }),
        inner,
    );
    let _ = app;
}

fn help_line(key: &str, description: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {key:<12}"), Style::default()),
        Span::styled(description.to_string(), Style::default()),
    ])
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
