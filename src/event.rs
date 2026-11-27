//! Messages exchanged between background workers and the UI loop.

use std::collections::HashMap;

use crossterm::event::Event;

use crate::model::{CacheEntry, Package, PkgInfo, SystemInfo, UpdateEntry};

/// Byte-level progress of a running download.
#[derive(Debug, Clone)]
pub struct Progress {
    /// File currently being fetched (or copied).
    pub file: String,
    /// Bytes transferred for the current file.
    pub done: u64,
    /// Expected size of the current file, when the server announced it.
    pub total: Option<u64>,
    /// Number of completed files.
    pub files_done: usize,
    /// Number of files in the whole batch.
    pub files_total: usize,
    /// Smoothed transfer rate in bytes/second.
    pub speed_bps: Option<f64>,
}

impl Progress {
    pub fn fraction(&self) -> f64 {
        match self.total {
            Some(total) if total > 0 => (self.done as f64 / total as f64).clamp(0.0, 1.0),
            Some(_) => 1.0,
            None => 0.0,
        }
    }
}

/// Work that should be re-read once a privileged operation finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadKind {
    None,
    Installed,
    All,
}

/// Everything a background task can report back to the app loop.
#[derive(Debug)]
pub enum AppEvent {
    Input(Event),
    Tick,
    PacmanPackages(Vec<Package>),
    AurNames(Vec<String>),
    Installed(HashMap<String, String>),
    Foreign(Vec<String>),
    Info {
        name: String,
        info: Box<PkgInfo>,
    },
    InfoFailed {
        name: String,
        error: String,
    },
    PacmanUpdates(Vec<UpdateEntry>),
    AurUpdates(Vec<UpdateEntry>),
    CacheEntries(Vec<CacheEntry>),
    CacheSize(u64),
    Orphans(Vec<String>),
    SystemInfo(SystemInfo),
    OpLine {
        text: String,
        error: bool,
    },
    OpProgress(Progress),
    OpStatus(String),
    OpDone {
        ok: bool,
        message: String,
        reload: ReloadKind,
    },
    Toast(String),
    LoadError {
        what: &'static str,
        error: String,
    },
}
