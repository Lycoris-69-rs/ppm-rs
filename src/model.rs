use std::cmp::Ordering;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Source {
    #[default]
    Pacman,
    Aur,
}

impl Source {
    pub fn short(self) -> &'static str {
        match self {
            Source::Pacman => "pac",
            Source::Aur => "aur",
        }
    }

    pub fn is_aur(self) -> bool {
        matches!(self, Source::Aur)
    }
}

#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,

    pub version: String,
    pub source: Source,
}

impl Package {
    pub fn new(name: impl Into<String>, version: impl Into<String>, source: Source) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            source,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Unknown,
    NotInstalled,
    Installed,

    Upgrade,

    Newer,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Unknown => "UNKNOWN",
            Status::NotInstalled => "NOT INSTALLED",
            Status::Installed => "INSTALLED",
            Status::Upgrade => "UPGRADE AVAILABLE",
            Status::Newer => "INSTALLED (NEWER)",
        }
    }
}

pub fn status_for(available: Option<&str>, installed: Option<&str>) -> Status {
    let available = available.filter(|v| !v.is_empty());
    match (available, installed) {
        (_, None) => Status::NotInstalled,
        (None, Some(_)) => Status::Unknown,
        (Some(a), Some(i)) => match vercmp(a, i) {
            Ordering::Greater => Status::Upgrade,
            Ordering::Less => Status::Newer,
            Ordering::Equal => Status::Installed,
        },
    }
}

#[derive(Debug, Clone)]
pub struct UpdateEntry {
    pub name: String,
    pub old: String,
    pub new: String,
    pub source: Source,
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub name: String,
    pub version: String,
    pub arch: String,
    pub path: PathBuf,
    pub size: u64,
    pub installed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PkgInfo {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub source: Source,
    pub url: Option<String>,
    pub licenses: Vec<String>,
    pub groups: Vec<String>,
    pub provides: Vec<String>,
    pub depends: Vec<String>,
    pub optdepends: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
    pub download_size: Option<String>,
    pub installed_size: Option<String>,
    pub packager: Option<String>,
    pub build_date: Option<String>,

    pub maintainer: Option<String>,
    pub votes: Option<u64>,
    pub popularity: Option<f64>,

    pub out_of_date: Option<String>,

    pub url_path: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SystemInfo {
    pub pacman_version: String,
    pub cache_dirs: Vec<PathBuf>,
    pub db_path: Option<String>,
    pub config_file: Option<String>,
    pub aur_helper: Option<String>,
    pub is_root: bool,
}

pub fn vercmp(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (a_epoch, a_ver, a_rel) = parse_evr(a);
    let (b_epoch, b_ver, b_rel) = parse_evr(b);

    let mut ret = rpmvercmp(a_epoch, b_epoch);
    if ret == 0 {
        ret = rpmvercmp(a_ver, b_ver);
        if ret == 0
            && let (Some(x), Some(y)) = (a_rel, b_rel)
        {
            ret = rpmvercmp(x, y);
        }
    }
    ret.cmp(&0)
}

fn parse_evr(s: &str) -> (&str, &str, Option<&str>) {
    let digits = s.bytes().take_while(|b| b.is_ascii_digit()).count();
    let rest = &s[digits..];
    let dash = rest.rfind('-');

    let (epoch, version_start) = if rest.starts_with(':') {
        (if digits == 0 { "0" } else { &s[..digits] }, digits + 1)
    } else {
        ("0", 0)
    };

    match dash {
        Some(idx) => {
            let abs = digits + idx;
            let version = if version_start <= abs {
                &s[version_start..abs]
            } else {
                ""
            };
            (epoch, version, Some(&s[(abs + 1).min(s.len())..]))
        }
        None => (epoch, &s[version_start..], None),
    }
}

fn rpmvercmp(a: &str, b: &str) -> i32 {
    if a == b {
        return 0;
    }
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let (mut one, mut two) = (0usize, 0usize);
    let (mut ptr1, mut ptr2) = (0usize, 0usize);

    while one < ab.len() && two < bb.len() {
        while one < ab.len() && !ab[one].is_ascii_alphanumeric() {
            one += 1;
        }
        while two < bb.len() && !bb[two].is_ascii_alphanumeric() {
            two += 1;
        }

        if one >= ab.len() || two >= bb.len() {
            break;
        }

        if (one - ptr1) != (two - ptr2) {
            return if one - ptr1 < two - ptr2 { -1 } else { 1 };
        }
        ptr1 = one;
        ptr2 = two;

        let isnum = ab[ptr1].is_ascii_digit();
        if isnum {
            while ptr1 < ab.len() && ab[ptr1].is_ascii_digit() {
                ptr1 += 1;
            }
            while ptr2 < bb.len() && bb[ptr2].is_ascii_digit() {
                ptr2 += 1;
            }
        } else {
            while ptr1 < ab.len() && ab[ptr1].is_ascii_alphabetic() {
                ptr1 += 1;
            }
            while ptr2 < bb.len() && bb[ptr2].is_ascii_alphabetic() {
                ptr2 += 1;
            }
        }

        let seg1 = &ab[one..ptr1];
        let seg2 = &bb[two..ptr2];

        if seg1.is_empty() {
            return -1;
        }
        if seg2.is_empty() {
            return if isnum { 1 } else { -1 };
        }

        if isnum {
            let s1 = trim_zeros(seg1);
            let s2 = trim_zeros(seg2);
            if s1.len() > s2.len() {
                return 1;
            }
            if s2.len() > s1.len() {
                return -1;
            }
            match s1.cmp(s2) {
                Ordering::Greater => return 1,
                Ordering::Less => return -1,
                Ordering::Equal => {}
            }
        } else {
            match seg1.cmp(seg2) {
                Ordering::Greater => return 1,
                Ordering::Less => return -1,
                Ordering::Equal => {}
            }
        }

        one = ptr1;
        two = ptr2;
    }

    let one_done = one >= ab.len();
    let two_done = two >= bb.len();
    if one_done && two_done {
        return 0;
    }

    let one_alpha = !one_done && ab[one].is_ascii_alphabetic();
    let two_alpha = !two_done && bb[two].is_ascii_alphabetic();
    if (one_done && !two_alpha) || one_alpha {
        -1
    } else {
        1
    }
}

fn trim_zeros(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| b != b'0').unwrap_or(s.len());
    &s[start..]
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmp(a: &str, b: &str) -> i32 {
        match vercmp(a, b) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }

    #[test]
    fn version_comparison_matches_alpm_expectations() {
        let cases: &[(&str, &str, i32)] = &[
            ("1.0", "1.0", 0),
            ("1.0-1", "1.0-1", 0),
            ("1.0", "1.0.0", -1),
            ("1.0.0", "1.0", 1),
            ("1.0-1", "1.0-2", -1),
            ("1.0-2", "1.0-1", 1),
            ("1.0", "1.0-1", 0),
            ("1.0-1", "1.0", 0),
            ("1.0rc1", "1.0", -1),
            ("1.0", "1.0rc1", 1),
            ("1.0a", "1.0", -1),
            ("1.0", "1.0a", 1),
            ("1.0.1", "1.0.1", 0),
            ("1.0.1", "1.0.10", -1),
            ("2:1.0-1", "1:9.9-9", 1),
            ("0:1.0-1", "1.0-1", 0),
            ("1:1.0-1", "1.0-1", 1),
            ("1.0-1", "1:1.0-1", -1),
            ("1.0.1-1", "1.0.1-1.1", -1),
            ("20240101", "20240102", -1),
            ("1.0.0.r1.gabc", "1.0.0.r2.gabc", -1),
            ("1.0.0-1", "1.0.0-01", 0),
            ("1.0.0-1", "1.0.0-1.1", -1),
            ("1.0.0", "1.0.0_1", -1),
            ("1.0.0+1", "1.0.0-1", 1),
            ("1.0~rc1", "1.0", 1),
            ("1.0^1", "1.0", 1),
            (":1.0-1", "1.0-1", 0),
            ("1.0-1", "1.0-", 1),
            ("1.0+", "1.0", 1),
            ("1.0.0alpha", "1.0.0beta", -1),
            ("7.3.6.arch1-1", "7.3.6.arch1-2", -1),
            ("1:1.6.9-1", "1:1.6.12-1", -1),
            ("6.18..", "6.18..", 0),
            ("1.0", "1.0-0", 0),
            ("1.0-0.1", "1.0-0", 1),
            ("1.0-1", "1.0-1.0", -1),
        ];
        for &(a, b, expected) in cases {
            assert_eq!(
                cmp(a, b),
                expected,
                "vercmp({a:?}, {b:?}) expected {expected}, got {}",
                cmp(a, b)
            );
            assert_eq!(cmp(b, a), -expected, "vercmp({b:?}, {a:?}) antisymmetry");
        }
    }

    #[test]
    fn vercmp_binary_oracle_if_available() {
        let pairs = oracle_pairs();
        if pairs.is_empty() {
            eprintln!("vercmp oracle skipped: no pacman database available");
            return;
        }
        for (a, b) in pairs {
            let out = std::process::Command::new("vercmp")
                .arg(&a)
                .arg(&b)
                .env("LC_ALL", "C")
                .output();
            let Ok(out) = out else {
                eprintln!("vercmp oracle skipped: vercmp binary missing");
                return;
            };
            if !out.status.success() {
                continue;
            }
            let expected: i32 = String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse()
                .unwrap_or(0);
            assert_eq!(
                cmp(&a, &b),
                expected,
                "vercmp({a:?}, {b:?}) expected {expected}, got {}",
                cmp(&a, &b)
            );
        }
    }

    fn oracle_pairs() -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        let output = std::process::Command::new("pacman")
            .args(["-Sl"])
            .env("LC_ALL", "C")
            .output();
        if let Ok(out) = output
            && out.status.success()
        {
            for line in String::from_utf8_lossy(&out.stdout).lines().step_by(97) {
                let mut parts = line.split_whitespace();
                let (_repo, _name, version) = match (parts.next(), parts.next(), parts.next()) {
                    (Some(r), Some(n), Some(v)) => (r, n, v),
                    _ => continue,
                };
                pairs.push((version.to_string(), version.to_string()));
                pairs.push((format!("{version}.1"), version.to_string()));
                pairs.push((version.to_string(), format!("{version}.1")));
                if let Some((base, _)) = version.rsplit_once('-') {
                    pairs.push((format!("{base}-2"), version.to_string()));
                }
            }
        }
        if let Ok(out) = std::process::Command::new("pacman")
            .args(["-Q"])
            .env("LC_ALL", "C")
            .output()
            && out.status.success()
        {
            for line in String::from_utf8_lossy(&out.stdout).lines().step_by(11) {
                let mut parts = line.split_whitespace();
                if let (Some(_name), Some(version)) = (parts.next(), parts.next()) {
                    pairs.push((version.to_string(), format!("{version}.0")));
                    pairs.push((format!("{version}-1"), format!("{version}-2")));
                }
            }
        }
        pairs.sort();
        pairs.dedup();
        pairs
    }

    #[test]
    fn status_derivation() {
        assert_eq!(status_for(Some("1.0"), None), Status::NotInstalled);
        assert_eq!(status_for(Some("1.0"), Some("1.0")), Status::Installed);
        assert_eq!(status_for(Some("2.0"), Some("1.0")), Status::Upgrade);
        assert_eq!(status_for(Some("1.0"), Some("2.0")), Status::Newer);
        assert_eq!(status_for(None, Some("1.0")), Status::Unknown);
        assert_eq!(status_for(Some(""), Some("1.0")), Status::Unknown);
    }

    #[test]
    fn byte_formatting() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1024 * 1024 * 3 / 2), "1.5 MiB");
    }
}
