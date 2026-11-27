//! Running external programs (pacman, sudo, yay, ...) and streaming their
//! output into the TUI log without ever handing them a TTY.

use std::path::PathBuf;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::event::AppEvent;

/// A command line that PPM is about to run.
#[derive(Debug, Clone)]
pub struct CmdSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CmdSpec {
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// Prefix with `sudo -n` unless PPM already runs as root.
    pub fn privileged(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let program = program.into();
        let args: Vec<String> = args.into_iter().map(Into::into).collect();
        if is_root() {
            Self { program, args }
        } else {
            let mut all = vec!["-n".to_string(), program];
            all.extend(args);
            Self {
                program: "sudo".to_string(),
                args: all,
            }
        }
    }

    /// Shell-like rendering for the confirmation dialog and log header.
    pub fn display(&self) -> String {
        let mut out = self.program.clone();
        for arg in &self.args {
            out.push(' ');
            if arg.is_empty() || arg.contains([' ', '\'', '"', '$']) {
                out.push('\'');
                out.push_str(&arg.replace('\'', "'\\''"));
                out.push('\'');
            } else {
                out.push_str(arg);
            }
        }
        out
    }
}

pub enum RunOutcome {
    Exited(i32),
    Cancelled,
    Failed(String),
}

/// Run a command, streaming merged stdout/stderr lines as [`AppEvent::OpLine`].
pub async fn run_streamed(
    spec: &CmdSpec,
    tx: &UnboundedSender<AppEvent>,
    cancel: &CancellationToken,
) -> RunOutcome {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return RunOutcome::Failed(format!("could not start `{}`: {err}", spec.program));
        }
    };

    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_line_reader(stdout, tx.clone(), false));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_line_reader(stderr, tx.clone(), true));
    }

    let status = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            for reader in &readers {
                reader.abort();
            }
            return RunOutcome::Cancelled;
        }
        status = child.wait() => status,
    };

    for reader in readers {
        let _ = reader.await;
    }

    match status {
        Ok(status) => RunOutcome::Exited(status.code().unwrap_or(-1)),
        Err(err) => RunOutcome::Failed(format!("failed to wait for `{}`: {err}", spec.program)),
    }
}

fn spawn_line_reader<R>(
    stream: R,
    tx: UnboundedSender<AppEvent>,
    is_stderr: bool,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let line = line.trim_end_matches('\r').trim_end();
                    if line.is_empty() {
                        continue;
                    }
                    let mut text = line.to_string();
                    if text.chars().count() > 2000 {
                        text = text.chars().take(2000).collect();
                    }
                    if tx
                        .send(AppEvent::OpLine {
                            text,
                            error: is_stderr,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
    })
}

#[derive(Debug)]
pub struct Captured {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run a command to completion and capture its output.
pub async fn capture(program: &str, args: &[&str]) -> Result<Captured, String> {
    let output = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|err| format!("could not run `{program}`: {err}"))?;
    Ok(Captured {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Like [`capture`], but accepts a non-zero exit status (used for queries
/// such as `pacman -Qtdq` which exits 1 when there is simply nothing to show).
pub async fn capture_query(program: &str, args: &[&str]) -> Result<Captured, String> {
    capture(program, args).await
}

pub fn is_root() -> bool {
    // Reading /proc avoids pulling in libc just for geteuid().
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return false;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next() == Some("0");
        }
    }
    false
}

/// Look up an executable in `$PATH`.
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn have_sudo() -> bool {
    is_root() || find_in_path("sudo").is_some()
}

/// The preferred AUR helper, if one is installed.
pub fn aur_helper() -> Option<String> {
    for helper in ["yay", "paru"] {
        if find_in_path(helper).is_some() {
            return Some(helper.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_display_quotes_arguments() {
        let spec = CmdSpec::new("pacman", ["-S", "--needed", "foo bar"]);
        assert_eq!(spec.display(), "pacman -S --needed 'foo bar'");
    }

    #[test]
    fn streams_lines_and_reports_exit_code() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let result = rt.block_on(async {
            let spec = CmdSpec::new(
                "sh",
                ["-c", "echo first; echo second; echo problem >&2; exit 2"],
            );
            let cancel = CancellationToken::new();
            let outcome = run_streamed(&spec, &tx, &cancel).await;
            drop(tx);
            let mut lines = Vec::new();
            while let Ok(event) = rx.try_recv() {
                if let AppEvent::OpLine { text, error } = event {
                    lines.push((text, error));
                }
            }
            (outcome, lines)
        });
        match result.0 {
            RunOutcome::Exited(code) => assert_eq!(code, 2),
            _ => panic!("expected exit code 2"),
        }
        assert!(result.1.iter().any(|(text, _)| text == "first"));
        assert!(result.1.iter().any(|(text, _)| text == "second"));
        assert!(
            result
                .1
                .iter()
                .any(|(text, error)| text == "problem" && *error)
        );
    }

    #[test]
    fn cancellation_stops_a_long_running_command() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let cancel = CancellationToken::new();
            let spec = CmdSpec::new("sh", ["-c", "sleep 30"]);
            let token = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                token.cancel();
            });
            let started = std::time::Instant::now();
            let outcome = run_streamed(&spec, &tx, &cancel).await;
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            assert!(matches!(outcome, RunOutcome::Cancelled));
        });
    }

    #[test]
    fn captures_output_and_status() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let captured = rt
            .block_on(capture("sh", &["-c", "echo hello; echo oops >&2; exit 3"]))
            .unwrap();
        assert_eq!(captured.status, 3);
        assert!(captured.stdout.contains("hello"));
        assert!(captured.stderr.contains("oops"));
    }
}
