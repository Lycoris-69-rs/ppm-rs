//! Streaming downloads (pacman package URLs, AUR snapshot tarballs and local
//! cache copies) with progress reporting and cooperative cancellation.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::event::{AppEvent, Progress};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

pub struct Downloader {
    client: reqwest::Client,
}

pub enum DlOutcome {
    Done(PathBuf),
    Skipped(PathBuf),
    Cancelled,
    Failed(String),
}

pub struct DlCtx<'a> {
    pub name: &'a str,
    pub index: usize,
    pub count: usize,
}

impl DlCtx<'_> {
    fn progress(
        &self,
        file: &str,
        done: u64,
        total: Option<u64>,
        speed: Option<f64>,
        files_done: usize,
    ) -> Progress {
        Progress {
            file: file.to_string(),
            done,
            total,
            files_done,
            files_total: self.count,
            speed_bps: speed,
        }
    }
}

impl Downloader {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// Download a single URL into `dest_dir`.
    ///
    /// `size_hint` is used when the server does not announce a content length.
    pub async fn fetch(
        &self,
        url: &str,
        dest_dir: &Path,
        size_hint: u64,
        ctx: &DlCtx<'_>,
        tx: &UnboundedSender<AppEvent>,
        cancel: &CancellationToken,
    ) -> DlOutcome {
        if let Some(path) = crate::repo::pacman::file_url_path(url) {
            return copy_local(&path, dest_dir, size_hint, ctx, tx, cancel).await;
        }
        self.fetch_http(url, dest_dir, size_hint, ctx, tx, cancel)
            .await
    }

    async fn fetch_http(
        &self,
        url: &str,
        dest_dir: &Path,
        size_hint: u64,
        ctx: &DlCtx<'_>,
        tx: &UnboundedSender<AppEvent>,
        cancel: &CancellationToken,
    ) -> DlOutcome {
        let file_name = file_name_from_url(url);
        let dest = dest_dir.join(&file_name);
        if let Ok(existing) = tokio::fs::metadata(&dest).await {
            let hint = if size_hint > 0 { Some(size_hint) } else { None };
            if hint.is_some_and(|hint| existing.len() == hint) {
                let _ = tx.send(AppEvent::OpProgress(ctx.progress(
                    &file_name,
                    existing.len(),
                    hint,
                    None,
                    ctx.index,
                )));
                return DlOutcome::Skipped(dest);
            }
        }
        if let Err(err) = tokio::fs::create_dir_all(dest_dir).await {
            return DlOutcome::Failed(format!("could not create {}: {err}", dest_dir.display()));
        }

        let response = tokio::select! {
            _ = cancel.cancelled() => return DlOutcome::Cancelled,
            response = self.client.get(url).send() => response,
        };
        let response = match response {
            Ok(response) => response,
            Err(err) => return DlOutcome::Failed(format!("download failed: {err}")),
        };
        if !response.status().is_success() {
            return DlOutcome::Failed(format!(
                "download failed: HTTP {} from {url}",
                response.status()
            ));
        }
        let total =
            response
                .content_length()
                .or(if size_hint > 0 { Some(size_hint) } else { None });

        let part = dest.with_extension(format!(
            "{}part",
            dest.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| format!("{ext}."))
                .unwrap_or_default()
        ));
        let mut file = match tokio::fs::File::create(&part).await {
            Ok(file) => file,
            Err(err) => {
                return DlOutcome::Failed(format!("could not write {}: {err}", part.display()));
            }
        };

        let mut stream = response.bytes_stream();
        let mut done = 0u64;
        let mut last_emit = Instant::now();
        let mut window_bytes = 0u64;
        let mut window_start = Instant::now();
        let mut speed = None;

        loop {
            let next = tokio::select! {
                _ = cancel.cancelled() => {
                    drop(file);
                    let _ = tokio::fs::remove_file(&part).await;
                    return DlOutcome::Cancelled;
                }
                chunk = stream.next() => chunk,
            };
            match next {
                Some(Ok(chunk)) => {
                    if let Err(err) = file.write_all(&chunk).await {
                        let _ = tokio::fs::remove_file(&part).await;
                        return DlOutcome::Failed(format!(
                            "could not write {}: {err}",
                            part.display()
                        ));
                    }
                    done += chunk.len() as u64;
                    window_bytes += chunk.len() as u64;
                    if last_emit.elapsed() >= PROGRESS_INTERVAL {
                        let elapsed = window_start.elapsed().as_secs_f64();
                        if elapsed > 0.0 {
                            speed = Some(window_bytes as f64 / elapsed);
                        }
                        window_bytes = 0;
                        window_start = Instant::now();
                        last_emit = Instant::now();
                        let _ = tx.send(AppEvent::OpProgress(
                            ctx.progress(&file_name, done, total, speed, ctx.index),
                        ));
                    }
                }
                Some(Err(err)) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(&part).await;
                    return DlOutcome::Failed(format!("download interrupted: {err}"));
                }
                None => break,
            }
        }

        if let Err(err) = file.flush().await {
            let _ = tokio::fs::remove_file(&part).await;
            return DlOutcome::Failed(format!("could not flush {}: {err}", part.display()));
        }
        drop(file);
        if let Err(err) = tokio::fs::rename(&part, &dest).await {
            let _ = tokio::fs::remove_file(&part).await;
            return DlOutcome::Failed(format!("could not finalize {}: {err}", dest.display()));
        }
        let _ = tx.send(AppEvent::OpProgress(ctx.progress(
            &file_name,
            done,
            total.or(Some(done)),
            None,
            ctx.index,
        )));
        DlOutcome::Done(dest)
    }
}

async fn copy_local(
    source: &Path,
    dest_dir: &Path,
    size_hint: u64,
    ctx: &DlCtx<'_>,
    tx: &UnboundedSender<AppEvent>,
    cancel: &CancellationToken,
) -> DlOutcome {
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("package.pkg.tar.zst")
        .to_string();
    let dest = dest_dir.join(&file_name);
    let metadata = match tokio::fs::metadata(source).await {
        Ok(metadata) => metadata,
        Err(err) => {
            return DlOutcome::Failed(format!(
                "cached package {} is unreadable: {err}",
                source.display()
            ));
        }
    };
    let size = if metadata.len() > 0 {
        metadata.len()
    } else {
        size_hint
    };
    if tokio::fs::metadata(&dest)
        .await
        .is_ok_and(|meta| meta.len() == size)
    {
        let _ = tx.send(AppEvent::OpProgress(ctx.progress(
            &file_name,
            size,
            Some(size),
            None,
            ctx.index,
        )));
        return DlOutcome::Skipped(dest);
    }
    if let Err(err) = tokio::fs::create_dir_all(dest_dir).await {
        return DlOutcome::Failed(format!("could not create {}: {err}", dest_dir.display()));
    }
    let _ = tx.send(AppEvent::OpProgress(ctx.progress(
        &file_name,
        0,
        Some(size),
        None,
        ctx.index,
    )));
    let copy = tokio::fs::copy(source, &dest);
    let result = tokio::select! {
        _ = cancel.cancelled() => {
            let _ = tokio::fs::remove_file(&dest).await;
            return DlOutcome::Cancelled;
        }
        result = copy => result,
    };
    match result {
        Ok(copied) => {
            let _ = tx.send(AppEvent::OpProgress(ctx.progress(
                &file_name,
                copied,
                Some(copied),
                None,
                ctx.index,
            )));
            DlOutcome::Done(dest)
        }
        Err(err) => DlOutcome::Failed(format!("could not copy cached package: {err}")),
    }
}

/// Derive a safe local file name from a URL path.
pub fn file_name_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    // Drop the scheme and authority so "https://example.com/" has no filename.
    let path = match path.find("://") {
        Some(index) => {
            let rest = &path[index + 3..];
            match rest.find('/') {
                Some(slash) => &rest[slash..],
                None => "",
            }
        }
        None => path,
    };
    let last = path
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("package");
    let decoded = percent_decode(last);
    let cleaned: String = decoded
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '+' | ':' | '~') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "package.download".to_string()
    } else {
        cleaned
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_file_names() {
        assert_eq!(
            file_name_from_url("https://mirror/extra/os/x86_64/cowsay-3.8.4-1-any.pkg.tar.zst"),
            "cowsay-3.8.4-1-any.pkg.tar.zst"
        );
        assert_eq!(
            file_name_from_url("https://aur.archlinux.org/cgit/aur.git/snapshot/yay.tar.gz?x=1"),
            "yay.tar.gz"
        );
        assert_eq!(file_name_from_url("https://example.com/"), "package");
        assert_eq!(
            file_name_from_url("https://example.com/a%20b.pkg.tar.zst"),
            "a_b.pkg.tar.zst"
        );
    }
}
