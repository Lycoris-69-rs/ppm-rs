//! Headless rendering used to preview/verify the TUI without a terminal.
//!
//! `ppm --snapshot <dir>` loads real data, drives the app through its screens
//! with scripted events, and writes one `.txt` and one `.html` file per screen.
//! The HTML files can be turned into PNG screenshots with a headless browser
//! (see `scripts/render-screenshots.sh`).

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

use crate::app::{App, BrowseMode, ConfirmDialog, Operation, PendingAction};
use crate::event::{AppEvent, Progress};
use crate::model::Source;
use crate::repo::{Paths, aur, pacman};

const WIDTH: u16 = 132;
const HEIGHT: u16 = 40;

pub async fn run(out_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("could not create {}", out_dir.display()))?;

    let client = aur::client().map_err(anyhow::Error::msg)?;
    let paths = Paths::discover();

    eprintln!("ppm snapshot: reading pacman databases…");
    let packages = pacman::list_available().await.unwrap_or_default();
    let installed = pacman::list_installed().await.unwrap_or_default();
    let updates = pacman::list_updates().await.unwrap_or_default();
    let orphans = pacman::list_orphans().await.unwrap_or_default();
    let cache_dirs = pacman::cache_dirs().await;
    let cache_scan = pacman::scan_cache(cache_dirs, installed.clone()).await;
    let cache = cache_scan.entries;
    let cache_size = cache_scan.total_size;
    let system = pacman::system_info().await;

    eprintln!("ppm snapshot: loading AUR package list…");
    let aur_names = match tokio::time::timeout(
        Duration::from_secs(45),
        aur::load_names(&client, &paths.cache_dir),
    )
    .await
    {
        Ok(Ok(loaded)) => loaded.names,
        _ => Vec::new(),
    };

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(tx, paths, client.clone());

    // Inject the data we just loaded instead of starting the async loaders.
    app.handle_event(AppEvent::Installed(installed));
    app.handle_event(AppEvent::PacmanPackages(packages));
    app.handle_event(AppEvent::AurNames(aur_names));
    app.handle_event(AppEvent::PacmanUpdates(updates));
    app.handle_event(AppEvent::CacheEntries(cache));
    app.handle_event(AppEvent::CacheSize(cache_size));
    app.handle_event(AppEvent::Orphans(orphans));
    app.handle_event(AppEvent::SystemInfo(system));
    app.handle_event(AppEvent::Tick);

    // 1. Main menu.
    render(&mut app, out_dir, "01-main-menu")?;

    // 2. Install browser, searching for "linux".
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "linux");
    fetch_info(&mut app, &client).await;
    render(&mut app, out_dir, "02-install-search")?;

    // 3. Confirmation dialog for installing the selected package.
    if let Some(row) = app.browse.current() {
        let name = row.name.clone();
        let source = row.source;
        let command = if source == Source::Aur {
            format!("yay -S --needed --noconfirm {name}")
        } else {
            format!("sudo -n pacman -S --needed --noconfirm {name}")
        };
        app.confirm = Some(ConfirmDialog {
            title: "Install 1 package(s)".to_string(),
            summary: vec![name.clone()],
            commands: vec![command],
            action: PendingAction::Install(vec![(name, source)]),
            danger: false,
        });
    }
    render(&mut app, out_dir, "03-confirm")?;
    app.confirm = None;

    // 4. Download progress overlay.
    let mut operation = Operation::new("Downloading 2 package(s)");
    operation.status = "downloading packages".to_string();
    app.overlay = Some(operation);
    app.handle_event(AppEvent::OpLine {
        text: "$ pacman -Spdd --noconfirm linux".to_string(),
        error: false,
    });
    app.handle_event(AppEvent::OpLine {
        text: "saved /home/user/ppm-rs/downloads/linux-7.3.6.arch1-1-x86_64.pkg.tar.zst"
            .to_string(),
        error: false,
    });
    app.handle_event(AppEvent::OpProgress(Progress {
        file: "linux-7.3.6.arch1-1-x86_64.pkg.tar.zst".to_string(),
        done: 96_468_992,
        total: Some(158_544_000),
        files_done: 0,
        files_total: 2,
        speed_bps: Some(11_534_336.0),
    }));
    render(&mut app, out_dir, "04-download-progress")?;

    // 5. Command console after a finished install.
    let operation = Operation::new("Installing 1 package(s)");
    app.overlay = Some(operation);
    for (text, error) in [
        ("$ sudo -n pacman -S --needed --noconfirm linux", false),
        ("resolving dependencies...", false),
        ("looking for conflicting packages...", false),
        ("", false),
        (
            "Packages (2) linux-7.3.6.arch1-1  linux-headers-7.3.6.arch1-1",
            false,
        ),
        ("", false),
        ("Total Download Size:   151.24 MiB", false),
        ("Total Installed Size:  151.61 MiB", false),
        ("", false),
        (":: Proceed with installation? [Y/n] ", false),
        ("checking keyring...", false),
        ("installing linux...", false),
        ("installing linux-headers...", false),
        (":: Running post-transaction hooks...", false),
        ("(1/2) Updating module dependencies...", false),
        ("(2/2) Updating the desktop file MIME type cache...", false),
    ] {
        app.handle_event(AppEvent::OpLine {
            text: text.to_string(),
            error,
        });
    }
    app.handle_event(AppEvent::OpDone {
        ok: true,
        message: "install: done".to_string(),
        reload: crate::event::ReloadKind::None,
    });
    render(&mut app, out_dir, "05-install-console")?;
    app.overlay = None;

    // 6. Updates screen.
    back_to_menu(&mut app);
    app.menu_index = 2;
    press(&mut app, KeyCode::Enter);
    fetch_info(&mut app, &client).await;
    render(&mut app, out_dir, "06-updates")?;

    // 7. Downgrade screen.
    back_to_menu(&mut app);
    app.menu_index = 3;
    press(&mut app, KeyCode::Enter);
    render(&mut app, out_dir, "07-downgrade")?;

    // 8. System tools.
    back_to_menu(&mut app);
    app.menu_index = 6;
    press(&mut app, KeyCode::Enter);
    render(&mut app, out_dir, "08-system-tools")?;

    // 9. Orphans.
    back_to_menu(&mut app);
    app.menu_index = 4;
    press(&mut app, KeyCode::Enter);
    render(&mut app, out_dir, "09-orphans")?;

    // 10. Cache cleanup.
    back_to_menu(&mut app);
    app.menu_index = 5;
    press(&mut app, KeyCode::Enter);
    render(&mut app, out_dir, "10-cache")?;

    // 11. Help overlay on top of the browser.
    back_to_menu(&mut app);
    app.menu_index = 0;
    press(&mut app, KeyCode::Enter);
    type_text(&mut app, "git");
    fetch_info(&mut app, &client).await;
    app.help_visible = true;
    render(&mut app, out_dir, "11-help")?;

    println!("snapshots written to {}", out_dir.display());
    Ok(())
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_event(AppEvent::Input(Event::Key(KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::NONE,
    })));
    app.handle_event(AppEvent::Tick);
}

fn type_text(app: &mut App, text: &str) {
    for character in text.chars() {
        press(app, KeyCode::Char(character));
    }
}

fn back_to_menu(app: &mut App) {
    // Leave any overlay/dialog and return to the main menu regardless of the
    // current filter or screen depth.
    app.overlay = None;
    app.confirm = None;
    app.help_visible = false;
    while app.screen != crate::app::Screen::MainMenu {
        press(app, KeyCode::Esc);
    }
}

async fn fetch_info(app: &mut App, client: &reqwest::Client) {
    let Some(row) = app.browse.current() else {
        return;
    };
    let name = row.name.clone();
    let source = row.source;
    let result = match source {
        Source::Pacman => pacman::info(&name).await,
        Source::Aur => aur::info(client, &name).await,
    };
    match result {
        Ok(info) => app.handle_event(AppEvent::Info {
            name,
            info: Box::new(info),
        }),
        Err(error) => app.handle_event(AppEvent::InfoFailed { name, error }),
    }
    app.handle_event(AppEvent::Tick);
}

fn render(app: &mut App, out_dir: &Path, name: &str) -> Result<()> {
    app.toast = None;
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut terminal = Terminal::new(backend).context("could not create test terminal")?;
    terminal.draw(|frame| crate::ui::draw(frame, app))?;
    let buffer = terminal.backend().buffer().clone();
    std::fs::write(out_dir.join(format!("{name}.txt")), buffer_to_text(&buffer))?;
    std::fs::write(
        out_dir.join(format!("{name}.html")),
        buffer_to_html(&buffer),
    )?;
    Ok(())
}

fn buffer_to_text(buffer: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buffer.area.height {
        let mut line = String::new();
        for x in 0..buffer.area.width {
            line.push_str(buffer[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn buffer_to_html(buffer: &Buffer) -> String {
    let mut html = String::new();
    html.push_str(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\n\
html,body{margin:0;padding:0;background:#ffffff;}\n\
pre{margin:0;padding:20px;white-space:pre;font-family:\"CaskaydiaMono Nerd Font\",\"Caskaydia Mono Nerd Font\",monospace;font-size:16px;line-height:18px;color:#111111;background:#ffffff;}\n\
</style></head><body><pre>",
    );
    for y in 0..buffer.area.height {
        let mut x = 0;
        while x < buffer.area.width {
            let cell = &buffer[(x, y)];
            let fg = cell.fg;
            let bg = cell.bg;
            let modifier = cell.modifier;
            let mut symbol = String::new();
            let mut next = x;
            while next < buffer.area.width {
                let candidate = &buffer[(next, y)];
                if candidate.fg != fg || candidate.bg != bg || candidate.modifier != modifier {
                    break;
                }
                symbol.push_str(candidate.symbol());
                next += 1;
            }
            html.push_str(&span_html(&symbol, fg, bg, modifier));
            x = next;
        }
        html.push('\n');
    }
    html.push_str("</pre></body></html>\n");
    html
}

fn span_html(symbol: &str, fg: Color, bg: Color, modifier: Modifier) -> String {
    let mut css = String::new();
    if let Some(color) = color_css(fg) {
        let _ = write!(css, "color:{color};");
    }
    if let Some(color) = color_css(bg) {
        let _ = write!(css, "background:{color};");
    }
    if modifier.contains(Modifier::BOLD) {
        let _ = write!(css, "font-weight:bold;");
    }
    if modifier.contains(Modifier::ITALIC) {
        let _ = write!(css, "font-style:italic;");
    }
    if modifier.contains(Modifier::UNDERLINED) {
        let _ = write!(css, "text-decoration:underline;");
    }
    if modifier.contains(Modifier::DIM) {
        let _ = write!(css, "opacity:0.55;");
    }
    format!("<span style=\"{css}\">{}</span>", escape_html(symbol))
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(character),
        }
    }
    out
}

fn color_css(color: Color) -> Option<String> {
    match color {
        Color::Reset => None,
        Color::Black => Some("#000000".into()),
        Color::Red => Some("#cd3131".into()),
        Color::Green => Some("#0dbc79".into()),
        Color::Yellow => Some("#e5e510".into()),
        Color::Blue => Some("#2472c8".into()),
        Color::Magenta => Some("#bc3fbc".into()),
        Color::Cyan => Some("#11a8cd".into()),
        Color::Gray => Some("#e5e5e5".into()),
        Color::DarkGray => Some("#666666".into()),
        Color::LightRed => Some("#f14c4c".into()),
        Color::LightGreen => Some("#23d18b".into()),
        Color::LightYellow => Some("#f5f543".into()),
        Color::LightBlue => Some("#3b8eea".into()),
        Color::LightMagenta => Some("#d670d6".into()),
        Color::LightCyan => Some("#29b8db".into()),
        Color::White => Some("#e5e5e5".into()),
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        Color::Indexed(index) => Some(xterm_css(index)),
    }
}

fn xterm_css(index: u8) -> String {
    const BASIC: [&str; 16] = [
        "#000000", "#800000", "#008000", "#808000", "#000080", "#800080", "#008080", "#c0c0c0",
        "#808080", "#ff0000", "#00ff00", "#ffff00", "#0000ff", "#ff00ff", "#00ffff", "#ffffff",
    ];
    if index < 16 {
        return BASIC[index as usize].to_string();
    }
    if index < 232 {
        let index = index - 16;
        let r = index / 36;
        let g = (index % 36) / 6;
        let b = index % 6;
        let scale = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
        return format!("#{:02x}{:02x}{:02x}", scale(r), scale(g), scale(b));
    }
    let grey = 8 + (index - 232) * 10;
    format!("#{grey:02x}{grey:02x}{grey:02x}")
}

#[allow(dead_code)]
fn unused_browse_mode() -> BrowseMode {
    BrowseMode::Install
}
