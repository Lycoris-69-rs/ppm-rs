use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use flate2::read::GzDecoder;
use serde::Deserialize;

use crate::model::{PkgInfo, Source, UpdateEntry, vercmp};

pub const RPC_BASE: &str = "https://aur.archlinux.org/rpc/v5";
pub const PACKAGES_URL: &str = "https://aur.archlinux.org/packages.gz";
pub const AUR_ROOT: &str = "https://aur.archlinux.org";

pub const NAMES_MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);

pub const RPC_BATCH: usize = 50;

pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("ppm-rs/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|err| format!("could not build HTTP client: {err}"))
}

pub struct NamesLoad {
    pub names: Vec<String>,
    pub warning: Option<String>,
    pub from_cache: bool,
}

pub async fn load_names(client: &reqwest::Client, cache_dir: &Path) -> Result<NamesLoad, String> {
    let cache_path = cache_dir.join("aur-packages.gz");
    let fresh = cache_path
        .metadata()
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < NAMES_MAX_AGE);

    if fresh && let Ok(names) = read_names_file(&cache_path).await {
        return Ok(NamesLoad {
            names,
            warning: None,
            from_cache: true,
        });
    }

    let downloaded = async {
        let response = client
            .get(PACKAGES_URL)
            .send()
            .await
            .map_err(|err| format!("AUR: request failed: {err}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "AUR: HTTP {} for {PACKAGES_URL}",
                response.status()
            ));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|err| format!("AUR: could not read package list: {err}"))?;
        tokio::fs::create_dir_all(cache_dir)
            .await
            .map_err(|err| format!("could not create cache dir: {err}"))?;
        let _ = tokio::fs::write(&cache_path, &bytes).await;
        decode_names(&bytes)
    }
    .await;

    match downloaded {
        Ok(names) => Ok(NamesLoad {
            names,
            warning: None,
            from_cache: false,
        }),
        Err(err) => {
            if let Ok(names) = read_names_file(&cache_path).await {
                Ok(NamesLoad {
                    names,
                    warning: Some(format!("{err}; using cached AUR list")),
                    from_cache: true,
                })
            } else {
                Err(err)
            }
        }
    }
}

async fn read_names_file(path: &Path) -> Result<Vec<String>, String> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|err| format!("could not read cached AUR list: {err}"))?;
    decode_names(&bytes)
}

fn decode_names(bytes: &[u8]) -> Result<Vec<String>, String> {
    let mut decoder = GzDecoder::new(bytes);
    let mut text = String::new();
    use std::io::Read;
    decoder
        .read_to_string(&mut text)
        .map_err(|err| format!("could not decompress AUR package list: {err}"))?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    #[serde(default)]
    results: Vec<AurPackage>,
}

#[derive(Debug, Default, Deserialize)]
struct AurPackage {
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "Version", default)]
    version: String,
    #[serde(rename = "Description", default)]
    description: Option<String>,
    #[serde(rename = "URL", default)]
    url: Option<String>,
    #[serde(rename = "License", default)]
    license: Vec<String>,
    #[serde(rename = "Depends", default)]
    depends: Vec<String>,
    #[serde(rename = "OptDepends", default)]
    optdepends: Vec<String>,
    #[serde(rename = "Conflicts", default)]
    conflicts: Vec<String>,
    #[serde(rename = "Provides", default)]
    provides: Vec<String>,
    #[serde(rename = "Replaces", default)]
    replaces: Vec<String>,
    #[serde(rename = "Maintainer", default)]
    maintainer: Option<String>,
    #[serde(rename = "NumVotes", default)]
    votes: Option<u64>,
    #[serde(rename = "Popularity", default)]
    popularity: Option<f64>,
    #[serde(rename = "OutOfDate", default)]
    out_of_date: Option<u64>,
    #[serde(rename = "URLPath", default)]
    url_path: Option<String>,
}

pub async fn info(client: &reqwest::Client, name: &str) -> Result<PkgInfo, String> {
    let mut infos = fetch_infos(client, std::slice::from_ref(&name.to_string())).await?;
    infos
        .pop()
        .ok_or_else(|| format!("AUR has no package named `{name}`"))
}

pub async fn fetch_infos(
    client: &reqwest::Client,
    names: &[String],
) -> Result<Vec<PkgInfo>, String> {
    let mut all = Vec::new();
    for chunk in names.chunks(RPC_BATCH) {
        let params: Vec<(&str, &str)> = chunk.iter().map(|name| ("arg[]", name.as_str())).collect();
        let response = client
            .get(format!("{RPC_BASE}/info"))
            .query(&params)
            .send()
            .await
            .map_err(|err| format!("AUR RPC request failed: {err}"))?;
        if !response.status().is_success() {
            return Err(format!("AUR RPC returned HTTP {}", response.status()));
        }
        let payload: RpcResponse = response
            .json()
            .await
            .map_err(|err| format!("AUR RPC returned invalid JSON: {err}"))?;
        all.extend(payload.results.into_iter().map(aur_to_info));
    }
    Ok(all)
}

fn aur_to_info(pkg: AurPackage) -> PkgInfo {
    PkgInfo {
        name: pkg.name,
        version: pkg.version,
        description: pkg.description,
        source: Source::Aur,
        url: pkg.url,
        licenses: pkg.license,
        groups: Vec::new(),
        provides: pkg.provides,
        depends: pkg.depends,
        optdepends: pkg.optdepends,
        conflicts: pkg.conflicts,
        replaces: pkg.replaces,
        download_size: None,
        installed_size: None,
        packager: pkg.maintainer.clone(),
        build_date: None,
        maintainer: pkg.maintainer,
        votes: pkg.votes,
        popularity: pkg.popularity,
        out_of_date: pkg.out_of_date.map(format_timestamp),
        url_path: pkg.url_path,
    }
}

pub async fn updates(
    client: &reqwest::Client,
    foreign: &std::collections::HashMap<String, String>,
) -> Result<Vec<UpdateEntry>, String> {
    if foreign.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<String> = foreign.keys().cloned().collect();
    let infos = fetch_infos(client, &names).await?;
    let mut updates = Vec::new();
    for info in infos {
        let Some(installed) = foreign.get(&info.name) else {
            continue;
        };
        if vercmp(&info.version, installed) == std::cmp::Ordering::Greater {
            updates.push(UpdateEntry {
                name: info.name.clone(),
                old: installed.clone(),
                new: info.version.clone(),
                source: Source::Aur,
            });
        }
    }
    Ok(updates)
}

pub fn snapshot_url(name: &str, url_path: Option<&str>) -> String {
    match url_path {
        Some(path) if path.starts_with('/') => format!("{AUR_ROOT}{path}"),
        _ => format!("{AUR_ROOT}/cgit/aur.git/snapshot/{name}.tar.gz"),
    }
}

pub fn format_timestamp(timestamp: u64) -> String {
    let days = (timestamp / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn package_url(name: &str) -> String {
    format!("{AUR_ROOT}/packages/{name}")
}

pub fn cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("aur-packages.gz")
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO_JSON: &str = r#"{
      "resultcount": 2,
      "results": [
        {
          "Name": "yay",
          "Version": "13.1.0-1",
          "Description": "Yet another yogurt.",
          "URL": "https://github.com/Jguer/yay",
          "License": ["GPL-3.0-or-later"],
          "Depends": ["pacman>6.1", "git"],
          "OptDepends": ["sudo", "doas"],
          "Conflicts": [],
          "Provides": ["yay"],
          "Replaces": [],
          "Maintainer": "jguer",
          "NumVotes": 2651,
          "Popularity": 10.75,
          "OutOfDate": null,
          "URLPath": "/cgit/aur.git/snapshot/yay.tar.gz"
        },
        {
          "Name": "oddpkg",
          "Version": "0.1-1",
          "Description": "Out of date package",
          "OutOfDate": 1700000000,
          "Maintainer": null
        }
      ]
    }"#;

    #[test]
    fn parses_rpc_payload() {
        let response: RpcResponse = serde_json::from_str(INFO_JSON).unwrap();
        assert_eq!(response.results.len(), 2);
        let info = aur_to_info(response.results.into_iter().next().unwrap());
        assert_eq!(info.name, "yay");
        assert_eq!(info.version, "13.1.0-1");
        assert_eq!(info.source, Source::Aur);
        assert_eq!(info.licenses, ["GPL-3.0-or-later"]);
        assert_eq!(info.depends, ["pacman>6.1", "git"]);
        assert_eq!(info.votes, Some(2651));
        assert_eq!(info.maintainer.as_deref(), Some("jguer"));
        assert!(info.out_of_date.is_none());
        assert_eq!(
            info.url_path.as_deref(),
            Some("/cgit/aur.git/snapshot/yay.tar.gz")
        );
    }

    #[test]
    fn parses_out_of_date_date() {
        let response: RpcResponse = serde_json::from_str(INFO_JSON).unwrap();
        let info = aur_to_info(response.results.into_iter().nth(1).unwrap());
        assert_eq!(info.out_of_date.as_deref(), Some("2023-11-14"));
    }

    #[test]
    fn snapshot_urls() {
        assert_eq!(
            snapshot_url("yay", Some("/cgit/aur.git/snapshot/yay.tar.gz")),
            "https://aur.archlinux.org/cgit/aur.git/snapshot/yay.tar.gz"
        );
        assert_eq!(
            snapshot_url("yay", None),
            "https://aur.archlinux.org/cgit/aur.git/snapshot/yay.tar.gz"
        );
        assert_eq!(
            snapshot_url("weird", Some("nonsense")),
            "https://aur.archlinux.org/cgit/aur.git/snapshot/weird.tar.gz"
        );
    }

    #[test]
    fn decodes_package_names() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(b"yay\nparu\n\nspotify\n").unwrap();
        let bytes = encoder.finish().unwrap();
        assert_eq!(decode_names(&bytes).unwrap(), ["yay", "paru", "spotify"]);
    }

    #[test]
    fn formats_timestamps() {
        assert_eq!(format_timestamp(0), "1970-01-01");
        assert_eq!(format_timestamp(1_700_000_000), "2023-11-14");
        assert_eq!(format_timestamp(1_760_000_000), "2025-10-09");
    }
}
