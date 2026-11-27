use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, InfoState, Row, RowKind};
use crate::model::{Source, Status, format_bytes};
use crate::ui::{self, panel_block, spinner, thousands};

const KEY_WIDTH: usize = 14;
const VALUE_INDENT: usize = KEY_WIDTH + 2;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel_block(" PACKAGE INFORMATION ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let Some(row) = app.browse.current() else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No package selected",
                Style::default(),
            )))
            .style(Style::default())
            .alignment(Alignment::Center),
            inner,
        );
        return;
    };

    let lines = build_lines(app, row, inner.width as usize);
    frame.render_widget(Paragraph::new(lines).style(Style::default()), inner);
}

fn build_lines(app: &App, row: &Row, width: usize) -> Vec<Line<'static>> {
    let status = app.row_status(row);
    let label = full_status(status);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" ● ", Style::default()),
            Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("  ", Style::default()),
            ui::source_tag(row.source),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Package Information",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];

    if let RowKind::Cached { path, size, arch } = &row.kind {
        lines.push(kv("Name", &row.name, width));
        lines.push(kv("Cached version", &row.version, width));
        lines.push(kv(
            "Installed",
            row.installed.as_deref().unwrap_or("—"),
            width,
        ));
        lines.push(kv("Architecture", arch, width));
        lines.push(kv("File size", &format_bytes(*size), width));
        lines.extend(kv_wrapped(
            "Location",
            &path.display().to_string(),
            width,
            2,
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Reinstalling this tarball downgrades the package.",
            Style::default().add_modifier(Modifier::DIM),
        )));
        return lines;
    }

    if let RowKind::Update { old, new } = &row.kind {
        lines.push(Line::from(vec![
            Span::styled("Update  ", Style::default()),
            Span::styled(
                format!("{old} → {new}"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    match app.data.info.get(&row.name) {
        None | Some(InfoState::Loading) => {
            lines.push(Line::from(Span::styled(
                format!("{} loading package metadata…", spinner(app)),
                Style::default(),
            )));
        }
        Some(InfoState::Failed(error)) => {
            lines.push(Line::from(Span::styled(
                format!("metadata unavailable: {error}"),
                Style::default(),
            )));
        }
        Some(InfoState::Ready(info)) => {
            lines.push(kv("Name", &info.name, width));
            if !info.version.is_empty() {
                lines.push(kv("Version", &info.version, width));
            }
            if let Some(installed) = &row.installed {
                lines.push(kv("Installed", installed, width));
            }
            lines.push(kv(
                "Source",
                match info.source {
                    Source::Pacman => "pacman repositories",
                    Source::Aur => "AUR",
                },
                width,
            ));

            if let Some(description) = &info.description {
                lines.push(Line::from(""));
                for chunk in wrap_words(description, width, 4) {
                    lines.push(Line::from(Span::styled(chunk, Style::default())));
                }
            }
            lines.push(Line::from(""));

            if let Some(size) = &info.download_size {
                lines.push(kv("Download size", size, width));
            }
            if let Some(size) = &info.installed_size {
                lines.push(kv("Installed size", size, width));
            }
            if let Some(url) = &info.url {
                lines.push(kv("Upstream", url, width));
            }
            if !info.licenses.is_empty() {
                lines.extend(kv_wrapped("Licenses", &info.licenses.join(", "), width, 2));
            }
            if !info.depends.is_empty() {
                lines.extend(kv_wrapped(
                    "Depends on",
                    &info.depends.join(" · "),
                    width,
                    2,
                ));
            }
            if !info.optdepends.is_empty() {
                lines.extend(kv_wrapped(
                    "Optional",
                    &info.optdepends.join(" · "),
                    width,
                    2,
                ));
            }
            if !info.conflicts.is_empty() {
                lines.extend(kv_wrapped(
                    "Conflicts",
                    &info.conflicts.join(" · "),
                    width,
                    1,
                ));
            }
            if !info.provides.is_empty() {
                lines.extend(kv_wrapped("Provides", &info.provides.join(" · "), width, 1));
            }
            if info.source == Source::Aur {
                lines.push(Line::from(""));
                if let Some(maintainer) = &info.maintainer {
                    lines.push(kv("Maintainer", maintainer, width));
                }
                if let Some(votes) = info.votes {
                    lines.push(kv("Votes", &thousands(votes as usize), width));
                }
                if let Some(popularity) = info.popularity {
                    lines.push(kv("Popularity", &format!("{popularity:.2}"), width));
                }
                if let Some(date) = &info.out_of_date {
                    lines.push(Line::from(vec![
                        Span::styled("Out of date", Style::default()),
                        Span::styled(format!("  {date}"), Style::default()),
                    ]));
                }
            }
            if info.source == Source::Pacman
                && let Some(packager) = &info.packager
            {
                lines.extend(kv_wrapped("Packager", packager, width, 1));
            }
            if let Some(build_date) = &info.build_date {
                lines.push(kv("Build date", build_date, width));
            }
        }
    }
    lines
}

fn kv(key: &str, value: &str, width: usize) -> Line<'static> {
    let available = width.saturating_sub(VALUE_INDENT).max(8);
    Line::from(vec![
        Span::styled(
            format!("{key:<KEY_WIDTH$}"),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(": ", Style::default().add_modifier(Modifier::DIM)),
        Span::styled(truncate(value, available), Style::default()),
    ])
}

fn kv_wrapped(key: &str, value: &str, width: usize, max_lines: usize) -> Vec<Line<'static>> {
    let available = width.saturating_sub(VALUE_INDENT).max(10);
    let chunks = wrap_words(value, available, max_lines);
    let mut lines = Vec::new();
    for (index, chunk) in chunks.into_iter().enumerate() {
        if index == 0 {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{key:<KEY_WIDTH$}"),
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Span::styled(": ", Style::default().add_modifier(Modifier::DIM)),
                Span::styled(chunk, Style::default()),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(" ".repeat(VALUE_INDENT), Style::default()),
                Span::styled(chunk, Style::default()),
            ]));
        }
    }
    lines
}

fn wrap_words(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();

        while word.chars().count() > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let chunk: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            lines.push(chunk);
        }
        if current.is_empty() {
            current.push_str(&word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(&word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(&word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            if last.chars().count() >= width {
                *last = truncate(last, width.saturating_sub(1));
            }
            last.push('…');
        }
    }
    lines
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

fn full_status(status: Status) -> &'static str {
    status.label()
}
