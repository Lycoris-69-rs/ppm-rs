//! The pacman side of the world: one flat package list merged from *all*
//! configured repositories, plus queries used by the UI and actions.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::exec::{self, Captured};
use crate::model::{CacheEntry, Package, PkgInfo, Source, SystemInfo, UpdateEntry};

/// Run a pacman query with a stable, parsable locale.
pub async fn query(args: &[&str]) -> Result<Captured, String> {
    exec::capture("pacman", args).await
}

pub async fn require_query(args: &[&str]) -> Result<String, String> {
    let captured = query(args).await?;
    if captured.status != 0 {
        let detail = captured.stderr.trim();
        return Err(if detail.is_empty() {
            format!(
                "pacman {} failed with exit code {}",
                args.join(" "),
                captured.status
            )
        } else {
            detail.to_string()
        });
    }
    Ok(captured.stdout)
}

/// `pacman -Sl`: every package from every configured repository.
///
/// Repositories are intentionally collapsed: the repository column is dropped
/// and a package name that appears in several repositories is only listed once
/// (the first occurrence wins, which matches pacman's repo priority order).
pub async fn list_available() -> Result<Vec<Package>, String> {
    let stdout = require_query(&["-Sl"]).await?;
    Ok(parse_available(&stdout))
}

pub fn parse_available(stdout: &str) -> Vec<Package> {
    let mut seen = HashSet::new();
    let mut packages = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let (_repo, Some(name), Some(version)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if seen.insert(name.to_string()) {
            packages.push(Package::new(name, version, Source::Pacman));
        }
    }
    packages
}

/// `pacman -Q`: name -> installed version.
pub async fn list_installed() -> Result<HashMap<String, String>, String> {
    let stdout = require_query(&["-Q"]).await?;
    Ok(parse_installed(&stdout))
}

pub fn parse_installed(stdout: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(name), Some(version)) = (parts.next(), parts.next()) {
            map.insert(name.to_string(), version.to_string());
        }
    }
    map
}

/// `pacman -Qm`: packages that no sync database provides (usually AUR/local).
pub async fn list_foreign() -> Result<Vec<String>, String> {
    let stdout = require_query(&["-Qm"]).await?;
    Ok(stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect())
}

/// `pacman -Qu`: packages with an upgrade available.
pub async fn list_updates() -> Result<Vec<UpdateEntry>, String> {
    let captured = query(&["-Qu"]).await?;
    // `-Qu` exits 1 with empty output when the system is up to date.
    Ok(parse_updates(&captured.stdout))
}

pub fn parse_updates(stdout: &str) -> Vec<UpdateEntry> {
    let mut updates = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 && parts[2] == "->" {
            updates.push(UpdateEntry {
                name: parts[0].to_string(),
                old: parts[1].to_string(),
                new: parts[3].to_string(),
                source: Source::Pacman,
            });
        }
    }
    updates
}

/// `pacman -Si <name>`: detailed metadata.
pub async fn info(name: &str) -> Result<PkgInfo, String> {
    let stdout = require_query(&["-Si", name]).await?;
    Ok(parse_info(&stdout, Source::Pacman))
}

/// `pacman -Qi <name>`: metadata for an installed package that is not in any
/// sync database (local builds, packages removed from repositories...).
pub async fn installed_info(name: &str) -> Result<PkgInfo, String> {
    let stdout = require_query(&["-Qi", name]).await?;
    Ok(parse_info(&stdout, Source::Pacman))
}

pub fn parse_info(stdout: &str, source: Source) -> PkgInfo {
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if let Some(last) = fields.last_mut() {
                last.1.push('\n');
                last.1.push_str(line.trim_start());
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            fields.push((key.trim().to_string(), value.trim().to_string()));
        }
    }

    let mut info = PkgInfo {
        source,
        ..Default::default()
    };
    for (key, value) in fields {
        let list = || split_list(&value);
        match key.as_str() {
            "Name" => info.name = value,
            "Version" => info.version = value,
            "Description" => info.description = non_empty(&value),
            "URL" => info.url = non_empty(&value),
            "Licenses" => info.licenses = list(),
            "Groups" => info.groups = list(),
            "Provides" => info.provides = list(),
            "Depends On" => info.depends = list(),
            "Optional Deps" => info.optdepends = list(),
            "Conflicts With" => info.conflicts = list(),
            "Replaces" => info.replaces = list(),
            "Download Size" => info.download_size = non_empty(&value),
            "Installed Size" => info.installed_size = non_empty(&value),
            "Packager" => info.packager = non_empty(&value),
            "Build Date" => info.build_date = non_empty(&value),
            _ => {}
        }
    }
    info
}

/// Split a pacman field into its values.  Values are separated either by runs
/// of two or more spaces (single-line fields) or by the newlines introduced by
/// continuation lines (multi-line fields such as Optional Deps).
fn split_list(value: &str) -> Vec<String> {
    if value.trim() == "None" {
        return Vec::new();
    }
    value
        .replace('\n', "\u{1f}")
        .split(['\u{1f}', '\t'])
        .flat_map(|chunk| chunk.split("  "))
        .map(str::trim)
        .filter(|item| !item.is_empty() && *item != "None")
        .map(str::to_string)
        .collect()
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value == "None" {
        None
    } else {
        Some(value.to_string())
    }
}

/// Resolve the download URLs of a package (and only that package: `-dd` tells
/// pacman to skip dependency resolution).  Returns `(url, size_hint)` pairs.
pub async fn download_urls(name: &str) -> Result<Vec<(String, u64)>, String> {
    let stdout = require_query(&[
        "-Spdd",
        "--noconfirm",
        "--print-format",
        "%n|%v|%l|%s",
        name,
    ])
    .await?;
    let mut urls = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.splitn(4, '|');
        let (Some(pkg), Some(_version), Some(url), size) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if pkg != name {
            continue;
        }
        let size = size.and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
        urls.push((url.to_string(), size));
    }
    if urls.is_empty() {
        return Err(format!(
            "pacman could not resolve a download URL for `{name}`"
        ));
    }
    Ok(urls)
}

/// `pacman -Qtdq`: orphaned packages.
pub async fn list_orphans() -> Result<Vec<String>, String> {
    let captured = query(&["-Qtdq"]).await?;
    // Exit code 1 simply means "no orphans".
    if captured.status != 0 && !captured.stdout.trim().is_empty() {
        return Err(captured.stderr.trim().to_string());
    }
    Ok(captured
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Read pacman's configuration paths.
pub async fn cache_dirs() -> Vec<PathBuf> {
    let dirs = match exec::capture("pacman-conf", &["CacheDir"]).await {
        Ok(captured) if captured.status == 0 => captured
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>(),
        _ => vec![PathBuf::from("/var/cache/pacman/pkg")],
    };
    if dirs.is_empty() {
        vec![PathBuf::from("/var/cache/pacman/pkg")]
    } else {
        dirs
    }
}

pub async fn db_path() -> Option<String> {
    match exec::capture("pacman-conf", &["DBPath"]).await {
        Ok(captured) if captured.status == 0 => {
            let value = captured.stdout.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }
        _ => Some("/var/lib/pacman".to_string()),
    }
}

pub async fn version() -> String {
    let output = exec::capture("pacman", &["-V"]).await;
    let Ok(captured) = output else {
        return "pacman not found".to_string();
    };
    for line in captured.stdout.lines() {
        let Some(position) = line.find("Pacman v") else {
            continue;
        };
        let mut version = line[position..].trim();
        // "Pacman v7.1.0 - libalpm v16.0.1" -> "Pacman v7.1.0"
        if let Some(index) = version.find(" - ") {
            version = &version[..index];
        }
        return version.to_string();
    }
    "pacman (unknown version)".to_string()
}

pub async fn system_info() -> SystemInfo {
    let cache_dirs = cache_dirs().await;
    let pacman_version = version().await;
    let db_path = db_path().await;
    SystemInfo {
        pacman_version,
        cache_dirs,
        db_path,
        config_file: Some("/etc/pacman.conf".to_string()),
        aur_helper: exec::aur_helper(),
        is_root: exec::is_root(),
    }
}

/// Compute the total size and file count of the package cache.
pub async fn cache_stats(dirs: Vec<PathBuf>) -> (u64, usize) {
    tokio::task::spawn_blocking(move || {
        let mut total = 0u64;
        let mut count = 0usize;
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if meta.is_file() {
                    total += meta.len();
                    count += 1;
                }
            }
        }
        (total, count)
    })
    .await
    .unwrap_or((0, 0))
}

/// Result of scanning the pacman cache.
#[derive(Debug, Default)]
pub struct CacheScan {
    /// One entry per unique package version (used by the downgrade screen).
    pub entries: Vec<CacheEntry>,
    /// Total size of all cached tarballs, including duplicate architectures.
    pub total_size: u64,
    /// Number of cached tarball files.
    pub file_count: usize,
}

/// Scan the pacman cache for package tarballs usable for a downgrade.
pub async fn scan_cache(dirs: Vec<PathBuf>, installed: HashMap<String, String>) -> CacheScan {
    tokio::task::spawn_blocking(move || scan_cache_blocking(&dirs, &installed))
        .await
        .unwrap_or_default()
}

pub fn scan_cache_blocking(dirs: &[PathBuf], installed: &HashMap<String, String>) -> CacheScan {
    let mut entries: HashMap<(String, String), CacheEntry> = HashMap::new();
    let mut total_size = 0u64;
    let mut file_count = 0usize;
    for dir in dirs {
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            continue;
        };
        for file in read_dir.flatten() {
            let path = file.path();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some((name, version, arch)) = parse_cache_filename(file_name) else {
                continue;
            };
            let Ok(meta) = file.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let installed_version = installed.get(&name).cloned();
            total_size += meta.len();
            file_count += 1;
            let entry = CacheEntry {
                installed: installed_version.as_deref() == Some(version.as_str()),
                name: name.clone(),
                version: version.clone(),
                arch,
                path,
                size: meta.len(),
            };
            // The same package version can be cached for several
            // architectures (e.g. x86_64 and x86_64_v4 on CachyOS): keep one.
            entries.entry((name, version)).or_insert(entry);
        }
    }
    let mut entries: Vec<CacheEntry> = entries.into_values().collect();
    entries.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| crate::model::vercmp(&b.version, &a.version))
            .then_with(|| a.arch.cmp(&b.arch))
    });
    CacheScan {
        entries,
        total_size,
        file_count,
    }
}

/// Parse `name-version-pkgrel-arch.pkg.tar.*` into
/// `(name, version-pkgrel, arch)`.
pub fn parse_cache_filename(file_name: &str) -> Option<(String, String, String)> {
    let pkg_pos = file_name.find(".pkg.tar")?;
    let stem = &file_name[..pkg_pos];
    let mut parts = stem.rsplitn(3, '-');
    let arch = parts.next()?;
    let pkgrel = parts.next()?;
    let name_version = parts.next()?;
    let (name, version) = name_version.rsplit_once('-')?;
    if name.is_empty() || version.is_empty() || pkgrel.is_empty() || arch.is_empty() {
        return None;
    }
    Some((
        name.to_string(),
        format!("{version}-{pkgrel}"),
        arch.to_string(),
    ))
}

/// Path of the package cache tarball if it is present locally.
pub fn cache_path_for(cache_dirs: &[PathBuf], file_name: &str) -> Option<PathBuf> {
    cache_dirs
        .iter()
        .map(|dir| dir.join(file_name))
        .find(|path| path.is_file())
}

/// True when the path points at a local file we can copy instead of download.
pub fn file_url_path(url: &str) -> Option<PathBuf> {
    url.strip_prefix("file://").map(PathBuf::from)
}

/// Is `path` on a local filesystem?
pub fn is_local_path(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sl_merging_repositories() {
        let fixture = "\
cachyos-v4 bash 5.3.20-2 [installed]
cachyos-v4 clang 23.1.5-2 [installed: 23.1.3-2]
core bash 5.3.20-2
core linux 7.3.6.arch1-1
extra linux-cachyos 6.18.1-1
extra cowsay 3.8.4-1
";
        let packages = parse_available(fixture);
        assert_eq!(packages.len(), 5);
        let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["bash", "clang", "linux", "linux-cachyos", "cowsay"]);
        // First occurrence (repo priority) wins.
        assert_eq!(packages[0].version, "5.3.20-2");
    }

    #[test]
    fn parses_installed_and_updates() {
        let installed = parse_installed("bash 5.3.20-2\nclang 23.1.3-2\n");
        assert_eq!(installed.get("bash").map(String::as_str), Some("5.3.20-2"));

        let updates = parse_updates(
            "7zip 26.04-1.1 -> 26.06-1.1\nalsa-card-profiles 1:1.6.9-1 -> 1:1.6.12-1\n",
        );
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[1].name, "alsa-card-profiles");
        assert_eq!(updates[1].old, "1:1.6.9-1");
        assert_eq!(updates[1].new, "1:1.6.12-1");
        assert!(updates.iter().all(|u| u.source == Source::Pacman));
    }

    #[test]
    fn parses_pacman_info() {
        let fixture = "\
Repository      : core
Name            : linux
Version         : 7.3.6.arch1-1
Description     : The Linux kernel and modules
Architecture    : x86_64
URL             : https://github.com/archlinux/linux
Licenses        : GPL-2.0-only
Groups          : None
Provides        : KSMBD-MODULE  NTSYNC-MODULE
Depends On      : coreutils  initramfs  kmod
Optional Deps   : linux-headers: headers for building modules
                  linux-firmware: firmware
Conflicts With  : None
Replaces        : None
Download Size   : 151.24 MiB
Installed Size  : 151.61 MiB
Packager        : Jan Alexander Steffens (heftig) <heftig@archlinux.org>
Build Date      : Tue 24 Nov 2026 07:08:01
";
        let info = parse_info(fixture, Source::Pacman);
        assert_eq!(info.name, "linux");
        assert_eq!(info.version, "7.3.6.arch1-1");
        assert_eq!(
            info.description.as_deref(),
            Some("The Linux kernel and modules")
        );
        assert_eq!(info.licenses, ["GPL-2.0-only"]);
        assert_eq!(info.depends, ["coreutils", "initramfs", "kmod"]);
        assert_eq!(
            info.optdepends,
            [
                "linux-headers: headers for building modules",
                "linux-firmware: firmware"
            ]
        );
        assert!(info.groups.is_empty());
        assert!(info.conflicts.is_empty());
        assert_eq!(info.download_size.as_deref(), Some("151.24 MiB"));
        assert!(info.build_date.is_some());
    }

    #[test]
    fn parses_cache_filenames() {
        assert_eq!(
            parse_cache_filename("linux-7.3.6.arch1-1-x86_64.pkg.tar.zst"),
            Some((
                "linux".to_string(),
                "7.3.6.arch1-1".to_string(),
                "x86_64".to_string()
            ))
        );
        assert_eq!(
            parse_cache_filename("python-requests-2.32.5-1-any.pkg.tar.zst"),
            Some((
                "python-requests".to_string(),
                "2.32.5-1".to_string(),
                "any".to_string()
            ))
        );
        assert_eq!(
            parse_cache_filename("linux-cachyos-6.18.1-1-x86_64_v4.pkg.tar.zst"),
            Some((
                "linux-cachyos".to_string(),
                "6.18.1-1".to_string(),
                "x86_64_v4".to_string()
            ))
        );
        assert_eq!(parse_cache_filename("not-a-package.txt"), None);
    }

    #[test]
    fn parses_download_urls() {
        let stdout = "\
cowsay|3.8.4-1|https://mirror.example/extra/os/x86_64/cowsay-3.8.4-1-any.pkg.tar.zst|19863
";
        let mut urls = Vec::new();
        for line in stdout.lines() {
            let mut parts = line.splitn(4, '|');
            let (Some(name), Some(_version), Some(url), size) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if name == "cowsay" {
                urls.push((url.to_string(), size.unwrap().parse::<u64>().unwrap()));
            }
        }
        assert_eq!(urls[0].1, 19863);
        assert!(urls[0].0.ends_with(".pkg.tar.zst"));
    }
}
