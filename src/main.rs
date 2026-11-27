use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use tokio::sync::mpsc;

use ppm::app::App;
use ppm::event::AppEvent;
use ppm::repo::{Paths, aur};
use ppm::ui;

const TICK: Duration = Duration::from_millis(120);
const PROGRAM: &str = env!("CARGO_PKG_NAME");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("{PROGRAM} {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--snapshot") => {
            let dir = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("screenshots"));
            match run_snapshot(dir) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{PROGRAM}: snapshot failed: {error:#}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(unknown) if unknown.starts_with('-') => {
            eprintln!("{PROGRAM}: unknown option `{unknown}` (try `{PROGRAM} --help`)");
            ExitCode::FAILURE
        }
        _ => run_tui(),
    }
}

fn print_help() {
    println!(
        "\
{program} {version} — Powerful Package Manager

A ratatui TUI for pacman repositories and the AUR, merged into one flat list.

USAGE:
    {program}                 start the interactive TUI
    {program} --snapshot DIR  render every screen to DIR as .txt/.html (no TTY needed)
    {program} --help          show this help
    {program} --version       show the version

ENVIRONMENT:
    PPM_CACHE_DIR       where the AUR package list is cached
    PPM_DOWNLOADS_DIR   where package tarballs are saved (default: ./downloads)

KEYS:
    ↑/↓ or j/k     move          Enter    select / run
    type           search        Esc      back / clear search
    Ctrl+D         download      Space    mark package
    Ctrl+R         reload data   F1 or ?  help
    Ctrl+C         quit

NOTES:
    System changes run through `sudo -n`.  Run `sudo -v` in a terminal first
    (or start {program} as root) so the password is cached for the session.
    AUR packages are built and installed with yay or paru.
",
        program = PROGRAM,
        version = env!("CARGO_PKG_VERSION")
    );
}

fn run_snapshot(dir: PathBuf) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(ppm::snapshot::run(&dir))
}

fn run_tui() -> ExitCode {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        eprintln!(
            "{PROGRAM} is an interactive TUI and needs a terminal.\n\
             Try `{PROGRAM} --help`, or `{PROGRAM} --snapshot DIR` for headless previews."
        );
        return ExitCode::FAILURE;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{PROGRAM}: could not start async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    let status = runtime.block_on(async {
        let client = match aur::client() {
            Ok(client) => client,
            Err(error) => {
                eprintln!("{PROGRAM}: {error}");
                return 1;
            }
        };

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx.clone(), Paths::discover(), client);
        app.start_loading();

        std::thread::spawn(move || {
            while let Ok(event) = crossterm::event::read() {
                if tx.send(AppEvent::Input(event)).is_err() {
                    break;
                }
            }
        });

        let mut terminal = ratatui::init();
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(event) => app.handle_event(event),
                        None => break,
                    }
                }
                _ = ticker.tick() => app.handle_event(AppEvent::Tick),
            }

            if app.should_quit {
                break;
            }
            if let Err(error) = terminal.draw(|frame| ui::draw(frame, &mut app)) {
                ratatui::restore();
                eprintln!("{PROGRAM}: rendering failed: {error}");
                return 1;
            }
        }

        ratatui::restore();
        0
    });

    if status == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
