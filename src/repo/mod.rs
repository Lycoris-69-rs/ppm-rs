//! Repository layer: pacman queries and the AUR client.

pub mod aur;
pub mod pacman;

use std::path::PathBuf;

/// Filesystem locations used by PPM.  Paths can be overridden with
/// `PPM_CACHE_DIR` / `PPM_DOWNLOADS_DIR`; otherwise XDG-style defaults are
/// used, falling back to a project-local directory when the home directory is
/// not writable (useful in sandboxes and containers).
#[derive(Debug, Clone)]
pub struct Paths {
    pub cache_dir: PathBuf,
    pub downloads_dir: PathBuf,
}

impl Paths {
    pub fn discover() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let cache_dir = env_path("PPM_CACHE_DIR").unwrap_or_else(|| {
            let preferred = std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .map(|dir| dir.join("ppm"))
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .map(|home| home.join(".cache").join("ppm"))
                });
            match preferred {
                Some(path) if ensure_dir(&path) => path,
                _ => {
                    let fallback = cwd.join(".ppm-cache");
                    let _ = ensure_dir(&fallback);
                    fallback
                }
            }
        });
        let downloads_dir = env_path("PPM_DOWNLOADS_DIR").unwrap_or_else(|| cwd.join("downloads"));
        let _ = ensure_dir(&downloads_dir);
        Self {
            cache_dir,
            downloads_dir,
        }
    }

    #[cfg(test)]
    pub fn test_paths() -> Self {
        let base = std::env::temp_dir().join("ppm-tests");
        Self {
            cache_dir: base.join("cache"),
            downloads_dir: base.join("downloads"),
        }
    }
}

fn env_path(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| ensure_dir(path))
}

fn ensure_dir(path: &std::path::Path) -> bool {
    std::fs::create_dir_all(path)
        .map(|_| true)
        .unwrap_or_else(|_| path.is_dir())
}
