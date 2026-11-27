use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::download::{DlCtx, DlOutcome, Downloader};
use crate::event::{AppEvent, Progress, ReloadKind};
use crate::exec::{self, CmdSpec, RunOutcome};
use crate::model::{
    CacheEntry, Package, PkgInfo, Source, Status, SystemInfo, UpdateEntry, format_bytes,
    status_for, vercmp,
};
use crate::repo::{Paths, aur, pacman};

const INFO_DEBOUNCE: Duration = Duration::from_millis(140);
const TOAST_LIFETIME: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    MainMenu,
    Browse,
    Orphans,
    CacheClean,
    SystemTools,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseMode {
    Install,
    Remove,
    Update,
    Downgrade,
}

impl BrowseMode {
    pub fn title(self) -> &'static str {
        match self {
            BrowseMode::Install => "INSTALL PACKAGES",
            BrowseMode::Remove => "REMOVE PACKAGES",
            BrowseMode::Update => "UPDATE PACKAGES",
            BrowseMode::Downgrade => "DOWNGRADE PACKAGES",
        }
    }

    pub fn search_hint(self) -> &'static str {
        match self {
            BrowseMode::Install => "Search every package (pacman repositories + AUR)",
            BrowseMode::Remove => "Search installed packages",
            BrowseMode::Update => "Search available updates",
            BrowseMode::Downgrade => "Search cached package versions",
        }
    }

    pub fn primary_verb(self) -> &'static str {
        match self {
            BrowseMode::Install => "install",
            BrowseMode::Remove => "remove",
            BrowseMode::Update => "upgrade",
            BrowseMode::Downgrade => "reinstall cached version of",
        }
    }

    pub fn supports_download(self) -> bool {
        matches!(self, BrowseMode::Install | BrowseMode::Update)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Install,
    Remove,
    Update,
    Downgrade,
    CleanOrphans,
    CleanCache,
    SystemTools,
    Exit,
}

pub const MENU_ITEMS: [MenuItem; 8] = [
    MenuItem::Install,
    MenuItem::Remove,
    MenuItem::Update,
    MenuItem::Downgrade,
    MenuItem::CleanOrphans,
    MenuItem::CleanCache,
    MenuItem::SystemTools,
    MenuItem::Exit,
];

impl MenuItem {
    pub fn title(self) -> &'static str {
        match self {
            MenuItem::Install => "Install Packages",
            MenuItem::Remove => "Remove Packages",
            MenuItem::Update => "Update Packages",
            MenuItem::Downgrade => "Downgrade Packages",
            MenuItem::CleanOrphans => "Clean Orphans",
            MenuItem::CleanCache => "Clean Package Cache",
            MenuItem::SystemTools => "System Tools",
            MenuItem::Exit => "Exit",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            MenuItem::Install => "browse, download and install packages",
            MenuItem::Remove => "uninstall packages and their dependencies",
            MenuItem::Update => "review and apply available upgrades",
            MenuItem::Downgrade => "restore an older version from the cache",
            MenuItem::CleanOrphans => "remove packages nothing depends on",
            MenuItem::CleanCache => "delete cached package tarballs",
            MenuItem::SystemTools => "pacman diagnostics and maintenance",
            MenuItem::Exit => "leave PPM",
        }
    }

    pub fn screen(self) -> Option<Screen> {
        match self {
            MenuItem::Install | MenuItem::Remove | MenuItem::Update | MenuItem::Downgrade => {
                Some(Screen::Browse)
            }
            MenuItem::CleanOrphans => Some(Screen::Orphans),
            MenuItem::CleanCache => Some(Screen::CacheClean),
            MenuItem::SystemTools => Some(Screen::SystemTools),
            MenuItem::Exit => None,
        }
    }

    pub fn browse_mode(self) -> Option<BrowseMode> {
        match self {
            MenuItem::Install => Some(BrowseMode::Install),
            MenuItem::Remove => Some(BrowseMode::Remove),
            MenuItem::Update => Some(BrowseMode::Update),
            MenuItem::Downgrade => Some(BrowseMode::Downgrade),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum InfoState {
    Loading,
    Ready(Box<PkgInfo>),
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum RowKind {
    Package,
    Update {
        old: String,
        new: String,
    },
    Cached {
        path: PathBuf,
        size: u64,
        arch: String,
    },
}

#[derive(Debug, Clone)]
pub struct Row {
    pub name: String,
    pub source: Source,

    pub version: String,
    pub installed: Option<String>,
    pub kind: RowKind,
}

#[derive(Debug, Clone, Default)]
pub struct Data {
    pub pacman_packages: Vec<Package>,
    pub aur_names: Vec<String>,
    pub packages: Vec<Package>,
    pub by_name: HashMap<String, usize>,
    pub installed: HashMap<String, String>,
    pub foreign: Vec<String>,
    pub info: HashMap<String, InfoState>,
    pub aur_versions: HashMap<String, String>,
    pub updates: Vec<UpdateEntry>,
    pub cache: Vec<CacheEntry>,
    pub cache_size: u64,
    pub orphans: Vec<String>,
    pub system: Option<SystemInfo>,

    pub loading_pacman: bool,
    pub loading_installed: bool,
    pub loading_aur: bool,
    pub loading_updates: bool,
    pub loading_cache: bool,
    pub loading_orphans: bool,
    pub aur_updates_pending: bool,

    pub pacman_loaded: bool,
    pub installed_loaded: bool,
    pub aur_loaded: bool,
    pub updates_loaded: bool,
    pub cache_loaded: bool,
    pub orphan_loaded: bool,

    pub revision: u64,
}

impl Data {
    pub fn aur_contains(&self, name: &str) -> bool {
        self.aur_names
            .binary_search_by(|candidate| candidate.as_str().cmp(name))
            .is_ok()
    }

    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    pub fn installed_count(&self) -> usize {
        self.installed.len()
    }

    pub fn package(&self, name: &str) -> Option<&Package> {
        self.by_name
            .get(name)
            .and_then(|index| self.packages.get(*index))
    }

    pub fn package_source(&self, name: &str) -> Source {
        if let Some(package) = self.package(name) {
            package.source
        } else if self.aur_contains(name) {
            Source::Aur
        } else {
            Source::Pacman
        }
    }

    pub fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

#[derive(Debug)]
pub struct BrowseState {
    pub mode: BrowseMode,
    pub filter: String,
    pub rows: Vec<Row>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub offset: usize,
    pub marked: HashSet<String>,
    pub loading: bool,
    pub built_revision: u64,
    pub info_for: Option<String>,
}

impl Default for BrowseState {
    fn default() -> Self {
        Self {
            mode: BrowseMode::Install,
            filter: String::new(),
            rows: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            offset: 0,
            marked: HashSet::new(),
            loading: false,
            built_revision: u64::MAX,
            info_for: None,
        }
    }
}

impl BrowseState {
    pub fn new(mode: BrowseMode) -> Self {
        Self {
            mode,
            ..Default::default()
        }
    }

    pub fn current(&self) -> Option<&Row> {
        self.filtered
            .get(self.selected)
            .and_then(|index| self.rows.get(*index))
    }

    pub fn current_name(&self) -> Option<&str> {
        self.current().map(|row| row.name.as_str())
    }

    pub fn apply_filter(&mut self) {
        let query = self.filter.trim().to_ascii_lowercase();
        self.filtered.clear();
        if query.is_empty() {
            self.filtered.extend(0..self.rows.len());
        } else {
            let mut exact = Vec::new();
            let mut prefix = Vec::new();
            let mut contains = Vec::new();
            for (index, row) in self.rows.iter().enumerate() {
                let name = row.name.to_ascii_lowercase();
                if name == query {
                    exact.push(index);
                } else if name.starts_with(&query) {
                    prefix.push(index);
                } else if name.contains(&query) {
                    contains.push(index);
                }
            }
            self.filtered.extend(exact);
            self.filtered.extend(prefix);
            self.filtered.extend(contains);
        }

        self.selected = 0;
        self.offset = 0;
    }

    pub fn marked_or_current(&self) -> Vec<&Row> {
        if self.marked.is_empty() {
            self.current().into_iter().collect()
        } else {
            self.rows
                .iter()
                .filter(|row| self.marked.contains(&row.name))
                .collect()
        }
    }

    pub fn toggle_mark(&mut self) {
        if let Some(row) = self.current() {
            let name = row.name.clone();
            if !self.marked.remove(&name) {
                self.marked.insert(name);
            }
        }
    }
}

#[derive(Debug)]
pub struct Toast {
    pub text: String,
    pub created: Instant,
    pub error: bool,
}

#[derive(Debug)]
pub struct Operation {
    pub title: String,
    pub lines: Vec<(String, bool)>,
    pub progress: Option<Progress>,
    pub status: String,
    pub running: bool,
    pub result: Option<bool>,
    pub cancel: CancellationToken,
}

impl Operation {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            lines: Vec::new(),
            progress: None,
            status: String::new(),
            running: true,
            result: None,
            cancel: CancellationToken::new(),
        }
    }

    pub fn push_line(&mut self, text: String, error: bool) {
        self.lines.push((text, error));
        if self.lines.len() > 2000 {
            self.lines.drain(..500);
        }
    }
}

#[derive(Debug)]
pub enum PendingAction {
    Install(Vec<(String, Source)>),
    Remove(Vec<String>),
    Update(Vec<(String, Source)>),
    UpdateAll,
    Downgrade(Vec<PathBuf>),
    RemoveOrphans(Vec<String>),
    CleanCache,
    RefreshDatabases,
    FullUpgrade,
    UpdateAur,
}

#[derive(Debug)]
pub struct ConfirmDialog {
    pub title: String,
    pub summary: Vec<String>,
    pub commands: Vec<String>,
    pub action: PendingAction,
    pub danger: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAction {
    RefreshDatabases,
    FullUpgrade,
    UpdateAur,
    ReloadData,
    ClearAurCache,
}

pub const SYSTEM_ACTIONS: [SystemAction; 5] = [
    SystemAction::RefreshDatabases,
    SystemAction::FullUpgrade,
    SystemAction::UpdateAur,
    SystemAction::ReloadData,
    SystemAction::ClearAurCache,
];

impl SystemAction {
    pub fn title(self) -> &'static str {
        match self {
            SystemAction::RefreshDatabases => "Refresh package databases",
            SystemAction::FullUpgrade => "Full system upgrade (pacman -Syu)",
            SystemAction::UpdateAur => "Update AUR packages (AUR helper -Sua)",
            SystemAction::ReloadData => "Reload package data",
            SystemAction::ClearAurCache => "Clear cached AUR package list",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            SystemAction::RefreshDatabases => "download fresh sync databases with pacman -Sy",
            SystemAction::FullUpgrade => "upgrade every installed package (asks for confirmation)",
            SystemAction::UpdateAur => "upgrade foreign (AUR) packages through the AUR helper",
            SystemAction::ReloadData => "re-read pacman -Sl / -Q / -Qu and the AUR package list",
            SystemAction::ClearAurCache => "force the AUR package list to be downloaded again",
        }
    }
}

pub struct App {
    pub screen: Screen,
    pub should_quit: bool,
    pub dirty: bool,

    pub menu_index: usize,
    pub browse: BrowseState,
    pub data: Data,

    pub overlay: Option<Operation>,
    pub confirm: Option<ConfirmDialog>,
    pub help_visible: bool,
    pub toast: Option<Toast>,

    pub system_action_index: usize,
    pub orphans_selected: usize,

    pub paths: Paths,
    pub tx: UnboundedSender<AppEvent>,
    pub client: reqwest::Client,

    pub spinner: u64,
    pub pending_info: Option<(String, Source, Instant)>,
    pub info_inflight: HashSet<String>,
    pub aur_updates_started: bool,
    pub last_tick: Instant,
}

impl App {
    pub fn new(tx: UnboundedSender<AppEvent>, paths: Paths, client: reqwest::Client) -> Self {
        Self {
            screen: Screen::MainMenu,
            should_quit: false,
            dirty: true,
            menu_index: 0,
            browse: BrowseState::default(),
            data: Data {
                loading_pacman: true,
                loading_installed: true,
                loading_aur: true,
                loading_updates: true,
                ..Default::default()
            },
            overlay: None,
            confirm: None,
            help_visible: false,
            toast: None,
            system_action_index: 0,
            orphans_selected: 0,
            paths,
            tx,
            client,
            spinner: 0,
            pending_info: None,
            info_inflight: HashSet::new(),
            aur_updates_started: false,
            last_tick: Instant::now(),
        }
    }

    pub fn start_loading(&mut self) {
        let tx = self.tx.clone();
        let tx_aur = self.tx.clone();
        let client = self.client.clone();
        let cache_dir = self.paths.cache_dir.clone();

        self.data.loading_pacman = true;
        self.data.loading_installed = true;
        self.data.loading_updates = true;
        self.dirty = true;

        tokio::spawn(async move {
            match pacman::list_available().await {
                Ok(packages) => {
                    let _ = tx.send(AppEvent::PacmanPackages(packages));
                }
                Err(err) => {
                    let _ = tx.send(AppEvent::LoadError {
                        what: "pacman -Sl",
                        error: err,
                    });
                }
            }
            match pacman::list_installed().await {
                Ok(installed) => {
                    let _ = tx.send(AppEvent::Installed(installed));
                }
                Err(err) => {
                    let _ = tx.send(AppEvent::LoadError {
                        what: "pacman -Q",
                        error: err,
                    });
                }
            }
            match pacman::list_foreign().await {
                Ok(foreign) => {
                    let _ = tx.send(AppEvent::Foreign(foreign));
                }
                Err(err) => {
                    let _ = tx.send(AppEvent::LoadError {
                        what: "pacman -Qm",
                        error: err,
                    });
                }
            }
            match pacman::list_updates().await {
                Ok(updates) => {
                    let _ = tx.send(AppEvent::PacmanUpdates(updates));
                }
                Err(err) => {
                    let _ = tx.send(AppEvent::LoadError {
                        what: "pacman -Qu",
                        error: err,
                    });
                }
            }
        });

        let tx_overview = self.tx.clone();
        tokio::spawn(async move {
            let dirs = pacman::cache_dirs().await;
            let (size, _count) = pacman::cache_stats(dirs).await;
            let _ = tx_overview.send(AppEvent::CacheSize(size));
            let info = pacman::system_info().await;
            let _ = tx_overview.send(AppEvent::SystemInfo(info));
        });

        self.data.loading_aur = true;
        tokio::spawn(async move {
            match aur::load_names(&client, &cache_dir).await {
                Ok(loaded) => {
                    if let Some(warning) = loaded.warning {
                        let _ = tx_aur.send(AppEvent::Toast(warning));
                    }
                    let _ = tx_aur.send(AppEvent::AurNames(loaded.names));
                }
                Err(err) => {
                    let _ = tx_aur.send(AppEvent::LoadError {
                        what: "AUR package list",
                        error: err,
                    });
                }
            }
        });
    }

    fn start_reload(&mut self) {
        self.start_loading();
        if self.data.cache_loaded {
            self.load_cache();
        }
        if self.data.orphan_loaded {
            self.load_orphans();
        }
    }

    fn load_cache(&mut self) {
        if self.data.loading_cache {
            return;
        }
        self.data.loading_cache = true;
        let tx = self.tx.clone();
        let installed = self.data.installed.clone();
        tokio::spawn(async move {
            let dirs = pacman::cache_dirs().await;
            let scan = pacman::scan_cache(dirs, installed).await;
            let _ = tx.send(AppEvent::CacheEntries(scan.entries));
            let _ = tx.send(AppEvent::CacheSize(scan.total_size));
        });
    }

    fn load_orphans(&mut self) {
        if self.data.loading_orphans {
            return;
        }
        self.data.loading_orphans = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match pacman::list_orphans().await {
                Ok(orphans) => {
                    let _ = tx.send(AppEvent::Orphans(orphans));
                }
                Err(err) => {
                    let _ = tx.send(AppEvent::LoadError {
                        what: "pacman -Qtdq",
                        error: err,
                    });
                }
            }
        });
    }

    fn load_system_info(&mut self) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let info = pacman::system_info().await;
            let _ = tx.send(AppEvent::SystemInfo(info));
        });
    }

    fn maybe_start_aur_update_check(&mut self) {
        if self.aur_updates_started
            || !self.data.installed_loaded
            || !self.data.aur_loaded
            || !self.data.updates_loaded
        {
            return;
        }
        self.aur_updates_started = true;
        let mut foreign: HashMap<String, String> = HashMap::new();
        for name in &self.data.foreign {
            if let Some(version) = self.data.installed.get(name) {
                foreign.insert(name.clone(), version.clone());
            }
        }
        if foreign.is_empty() {
            return;
        }
        self.data.aur_updates_pending = true;
        self.dirty = true;
        let tx = self.tx.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            match aur::updates(&client, &foreign).await {
                Ok(updates) => {
                    let _ = tx.send(AppEvent::AurUpdates(updates));
                }
                Err(err) => {
                    let _ = tx.send(AppEvent::LoadError {
                        what: "AUR update check",
                        error: err,
                    });
                }
            }
        });
    }

    pub fn row_version<'a>(&'a self, row: &'a Row) -> Option<&'a str> {
        match &row.kind {
            RowKind::Update { new, .. } => Some(new),
            RowKind::Cached { .. } => Some(&row.version),
            RowKind::Package => {
                if row.version.is_empty() {
                    self.data.aur_versions.get(&row.name).map(String::as_str)
                } else {
                    Some(&row.version)
                }
            }
        }
    }

    pub fn row_status(&self, row: &Row) -> Status {
        match &row.kind {
            RowKind::Update { .. } => Status::Upgrade,
            RowKind::Cached { .. } => status_for(Some(&row.version), row.installed.as_deref()),
            RowKind::Package => status_for(self.row_version(row), row.installed.as_deref()),
        }
    }

    pub fn row_display_version(&self, row: &Row) -> String {
        match &row.kind {
            RowKind::Update { new, .. } => new.clone(),
            RowKind::Cached { .. } => self.row_version(row).unwrap_or("").to_string(),
            RowKind::Package => self
                .row_version(row)
                .map(str::to_string)
                .unwrap_or_else(|| "—".to_string()),
        }
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            created: Instant::now(),
            error: false,
        });
        self.dirty = true;
    }

    pub fn toast_error(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            created: Instant::now(),
            error: true,
        });
        self.dirty = true;
    }

    fn current_row(&self) -> Option<&Row> {
        self.browse.current()
    }

    fn sync_info_target(&mut self) {
        let Some(row) = self.current_row() else {
            self.pending_info = None;
            return;
        };
        let name = row.name.clone();
        let source = row.source;
        if self.data.info.contains_key(&name) || self.info_inflight.contains(&name) {
            self.pending_info = None;
            self.browse.info_for = Some(name);
            return;
        }
        self.pending_info = Some((name, source, Instant::now()));
    }

    fn request_info(&mut self, name: String, source: Source) {
        if self.data.info.contains_key(&name) || !self.info_inflight.insert(name.clone()) {
            return;
        }
        self.browse.info_for = Some(name.clone());
        self.data.info.insert(name.clone(), InfoState::Loading);
        let tx = self.tx.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let result = match source {
                Source::Pacman => match pacman::info(&name).await {
                    Ok(info) => Ok(info),
                    Err(error) => pacman::installed_info(&name).await.map_err(|_| error),
                },
                Source::Aur => aur::info(&client, &name).await,
            };
            match result {
                Ok(info) => {
                    let _ = tx.send(AppEvent::Info {
                        name,
                        info: Box::new(info),
                    });
                }
                Err(err) => {
                    let _ = tx.send(AppEvent::InfoFailed { name, error: err });
                }
            }
        });
    }

    pub fn handle_event(&mut self, event: AppEvent) {
        self.dirty = true;
        match event {
            AppEvent::Input(input) => self.handle_input(input),
            AppEvent::Tick => self.handle_tick(),
            AppEvent::PacmanPackages(packages) => {
                self.data.pacman_packages = packages;
                self.data.pacman_loaded = true;
                self.data.loading_pacman = false;
                self.rebuild_packages();
                self.maybe_start_aur_update_check();
            }
            AppEvent::AurNames(names) => {
                let mut names = names;
                names.sort_unstable();
                names.dedup();
                self.data.aur_names = names;
                self.data.aur_loaded = true;
                self.data.loading_aur = false;
                self.rebuild_packages();
                self.maybe_start_aur_update_check();
            }
            AppEvent::Installed(installed) => {
                self.data.installed = installed;
                self.data.installed_loaded = true;
                self.data.loading_installed = false;
                self.rebuild_packages();
                self.maybe_start_aur_update_check();
            }
            AppEvent::Foreign(foreign) => {
                self.data.foreign = foreign;
                self.maybe_start_aur_update_check();
            }
            AppEvent::PacmanUpdates(updates) => {
                self.data
                    .updates
                    .retain(|update| update.source != Source::Pacman);
                self.data.updates.extend(updates);
                self.data.updates.sort_by(|a, b| a.name.cmp(&b.name));
                self.data.updates_loaded = true;
                self.data.loading_updates = false;
                self.data.bump();
                self.maybe_start_aur_update_check();
            }
            AppEvent::AurUpdates(updates) => {
                self.data
                    .updates
                    .retain(|update| update.source != Source::Aur);
                self.data.updates.extend(updates);
                self.data.updates.sort_by(|a, b| a.name.cmp(&b.name));
                self.data.aur_updates_pending = false;
                self.data.bump();
            }
            AppEvent::Info { name, info } => {
                self.info_inflight.remove(&name);
                if info.source == Source::Aur {
                    self.data
                        .aur_versions
                        .insert(name.clone(), info.version.clone());
                }
                self.data.info.insert(name, InfoState::Ready(info));
                self.dirty = true;
            }
            AppEvent::InfoFailed { name, error } => {
                self.info_inflight.remove(&name);
                self.data.info.insert(name, InfoState::Failed(error));
            }
            AppEvent::CacheEntries(entries) => {
                self.data.cache = entries;
                self.data.cache_loaded = true;
                self.data.loading_cache = false;
                self.data.bump();
            }
            AppEvent::CacheSize(size) => {
                self.data.cache_size = size;
            }
            AppEvent::Orphans(orphans) => {
                self.data.orphans = orphans;
                self.data.orphan_loaded = true;
                self.data.loading_orphans = false;
            }
            AppEvent::SystemInfo(info) => {
                self.data.system = Some(info);
            }
            AppEvent::OpLine { text, error } => {
                if let Some(operation) = &mut self.overlay {
                    operation.push_line(text, error);
                }
            }
            AppEvent::OpProgress(progress) => {
                if let Some(operation) = &mut self.overlay {
                    operation.progress = Some(progress);
                }
            }
            AppEvent::OpStatus(status) => {
                if let Some(operation) = &mut self.overlay {
                    operation.status = status;
                }
            }
            AppEvent::OpDone {
                ok,
                message,
                reload,
            } => {
                if let Some(operation) = &mut self.overlay {
                    operation.running = false;
                    operation.result = Some(ok);
                    operation.status = message.clone();
                }
                if ok {
                    match reload {
                        ReloadKind::None => {}
                        ReloadKind::Installed => self.start_loading(),
                        ReloadKind::All => self.start_reload(),
                    }
                }
                if ok {
                    self.toast(message);
                } else {
                    self.toast_error(message);
                }
            }
            AppEvent::Toast(text) => self.toast(text),
            AppEvent::LoadError { what, error } => {
                self.toast_error(format!("{what}: {error}"));
            }
        }
    }

    fn handle_tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
        self.last_tick = Instant::now();

        if let Some(toast) = &self.toast
            && toast.created.elapsed() > TOAST_LIFETIME
        {
            self.toast = None;
            self.dirty = true;
        }

        if let Some((name, source, at)) = self.pending_info.clone()
            && at.elapsed() >= INFO_DEBOUNCE
        {
            self.pending_info = None;
            self.request_info(name, source);
        }

        if self.screen == Screen::Browse && self.browse.built_revision != self.data.revision {
            self.rebuild_rows();
        }
    }

    fn rebuild_packages(&mut self) {
        let mut merged: Vec<Package> = self.data.pacman_packages.clone();
        let mut seen: HashSet<String> = merged.iter().map(|package| package.name.clone()).collect();
        for name in &self.data.aur_names {
            if seen.insert(name.clone()) {
                merged.push(Package::new(name.clone(), String::new(), Source::Aur));
            }
        }
        for (name, version) in &self.data.installed {
            if !seen.contains(name) {
                let source = if self.data.aur_contains(name) {
                    Source::Aur
                } else {
                    Source::Pacman
                };
                merged.push(Package::new(name.clone(), version.clone(), source));
            }
        }
        merged.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        });
        self.data.by_name = merged
            .iter()
            .enumerate()
            .map(|(index, package)| (package.name.clone(), index))
            .collect();
        self.data.packages = merged;
        self.data.bump();
    }

    fn rows_for_mode(&self, mode: BrowseMode) -> Vec<Row> {
        match mode {
            BrowseMode::Install => self
                .data
                .packages
                .iter()
                .map(|package| Row {
                    name: package.name.clone(),
                    source: package.source,
                    version: package.version.clone(),
                    installed: self.data.installed.get(&package.name).cloned(),
                    kind: RowKind::Package,
                })
                .collect(),
            BrowseMode::Remove => {
                let mut rows: Vec<Row> = self
                    .data
                    .installed
                    .iter()
                    .map(|(name, version)| {
                        let package = self.data.package(name);
                        let source = package.map(|package| package.source).unwrap_or_else(|| {
                            if self.data.aur_contains(name) {
                                Source::Aur
                            } else {
                                Source::Pacman
                            }
                        });
                        Row {
                            name: name.clone(),
                            source,
                            version: package
                                .map(|package| package.version.clone())
                                .unwrap_or_else(|| version.clone()),
                            installed: Some(version.clone()),
                            kind: RowKind::Package,
                        }
                    })
                    .collect();
                rows.sort_by(|a, b| a.name.cmp(&b.name));
                rows
            }
            BrowseMode::Update => self
                .data
                .updates
                .iter()
                .map(|update| Row {
                    name: update.name.clone(),
                    source: update.source,
                    version: update.new.clone(),
                    installed: Some(update.old.clone()),
                    kind: RowKind::Update {
                        old: update.old.clone(),
                        new: update.new.clone(),
                    },
                })
                .collect(),
            BrowseMode::Downgrade => {
                let mut rows: Vec<Row> = self
                    .data
                    .cache
                    .iter()
                    .map(|entry| Row {
                        name: entry.name.clone(),
                        source: Source::Pacman,
                        version: entry.version.clone(),
                        installed: self.data.installed.get(&entry.name).cloned(),
                        kind: RowKind::Cached {
                            path: entry.path.clone(),
                            size: entry.size,
                            arch: entry.arch.clone(),
                        },
                    })
                    .collect();
                rows.sort_by(|a, b| {
                    a.name
                        .cmp(&b.name)
                        .then_with(|| vercmp(&b.version, &a.version))
                });
                rows
            }
        }
    }

    fn rebuild_rows(&mut self) {
        let previous_name = self.browse.current_name().map(str::to_string);
        self.browse.rows = self.rows_for_mode(self.browse.mode);
        self.browse.built_revision = self.data.revision;
        self.browse.loading = match self.browse.mode {
            BrowseMode::Install => !self.data.pacman_loaded && !self.data.aur_loaded,
            BrowseMode::Remove => !self.data.installed_loaded,
            BrowseMode::Update => !self.data.updates_loaded,
            BrowseMode::Downgrade => !self.data.cache_loaded,
        };
        let names: HashSet<String> = self
            .browse
            .rows
            .iter()
            .map(|row| row.name.clone())
            .collect();
        self.browse.marked.retain(|name| names.contains(name));
        self.browse.apply_filter();
        if let Some(name) = previous_name
            && let Some(position) = self
                .browse
                .filtered
                .iter()
                .position(|index| self.browse.rows[*index].name == name)
        {
            self.browse.selected = position;
        }
        self.clamp_selection();
        self.sync_info_target();
    }

    fn clamp_selection(&mut self) {
        if self.browse.filtered.is_empty() {
            self.browse.selected = 0;
            self.browse.offset = 0;
        } else {
            self.browse.selected = self.browse.selected.min(self.browse.filtered.len() - 1);
        }
    }

    fn handle_input(&mut self, event: Event) {
        let Event::Key(key) = event else {
            return;
        };
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if self.help_visible {
            self.help_visible = false;
            return;
        }
        if let Some(confirm) = self.confirm.take() {
            self.handle_confirm_input(key, confirm);
            return;
        }
        if self.overlay.is_some() {
            self.handle_overlay_input(key);
            return;
        }
        if key.code == KeyCode::F(1) {
            self.help_visible = true;
            return;
        }
        if key.code == KeyCode::Char('?') {
            self.help_visible = true;
            return;
        }
        match self.screen {
            Screen::MainMenu => self.handle_menu_input(key),
            Screen::Browse => self.handle_browse_input(key),
            Screen::Orphans => self.handle_orphans_input(key),
            Screen::CacheClean => self.handle_cache_input(key),
            Screen::SystemTools => self.handle_system_input(key),
        }
    }

    fn handle_confirm_input(&mut self, key: KeyEvent, dialog: ConfirmDialog) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.execute_action(dialog.action);
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {
                self.confirm = None;
            }
            _ => self.confirm = Some(dialog),
        }
    }

    fn handle_overlay_input(&mut self, key: KeyEvent) {
        let Some(running) = self.overlay.as_ref().map(|operation| operation.running) else {
            return;
        };
        if running {
            if key.code == KeyCode::Esc {
                if let Some(operation) = &self.overlay {
                    operation.cancel.cancel();
                }
                if let Some(operation) = &mut self.overlay {
                    operation.status = "cancelling…".to_string();
                }
            }
            return;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = None;
            }
            _ => {}
        }
    }

    fn handle_menu_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.menu_index = (self.menu_index + MENU_ITEMS.len() - 1) % MENU_ITEMS.len();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.menu_index = (self.menu_index + 1) % MENU_ITEMS.len();
            }
            KeyCode::Home => self.menu_index = 0,
            KeyCode::End => self.menu_index = MENU_ITEMS.len() - 1,
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l') => {
                self.activate_menu_item(MENU_ITEMS[self.menu_index]);
            }
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('R') => {
                self.toast("Reloading package data…");
                self.start_reload();
            }
            _ => {}
        }
    }

    fn activate_menu_item(&mut self, item: MenuItem) {
        match item {
            MenuItem::Exit => self.should_quit = true,
            MenuItem::Install => self.open_browse(BrowseMode::Install),
            MenuItem::Remove => self.open_browse(BrowseMode::Remove),
            MenuItem::Update => self.open_browse(BrowseMode::Update),
            MenuItem::Downgrade => self.open_browse(BrowseMode::Downgrade),
            MenuItem::CleanOrphans => {
                self.screen = Screen::Orphans;
                self.orphans_selected = 0;
                if !self.data.orphan_loaded {
                    self.load_orphans();
                }
            }
            MenuItem::CleanCache => {
                self.screen = Screen::CacheClean;
                self.load_cache();
            }
            MenuItem::SystemTools => {
                self.screen = Screen::SystemTools;
                self.system_action_index = 0;
                if self.data.system.is_none() {
                    self.load_system_info();
                }
            }
        }
    }

    fn open_browse(&mut self, mode: BrowseMode) {
        if mode == BrowseMode::Downgrade && !self.data.cache_loaded {
            self.load_cache();
        }
        if mode == BrowseMode::Install && !self.data.pacman_loaded && !self.data.loading_pacman {
            self.start_loading();
        }
        self.screen = Screen::Browse;
        self.browse = BrowseState::new(mode);
        self.browse.loading = true;
        self.rebuild_rows();
        self.schedule_selection_info();
    }

    fn back_to_menu(&mut self) {
        self.screen = Screen::MainMenu;
        self.pending_info = None;
        self.confirm = None;
    }

    fn handle_browse_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                if !self.browse.filter.is_empty() {
                    self.browse.filter.clear();
                    self.browse.apply_filter();
                    self.schedule_selection_info();
                } else {
                    self.back_to_menu();
                }
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home => self.set_selection(0),
            KeyCode::End => self.set_selection(self.browse.filtered.len().saturating_sub(1)),
            KeyCode::Left => {
                if !self.browse.marked.is_empty() {
                    self.browse.marked.clear();
                    self.toast("Selection cleared");
                }
            }
            KeyCode::Enter => self.ask_primary_action(),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.ask_download()
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.toast("Reloading package data…");
                self.start_reload();
            }
            KeyCode::Char(' ') => {
                self.browse.toggle_mark();
                self.dirty = true;
            }
            KeyCode::Backspace => {
                self.browse.filter.pop();
                self.browse.apply_filter();
                self.schedule_selection_info();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.browse.filter.clear();
                self.browse.apply_filter();
                self.schedule_selection_info();
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.browse.filter.push(character);
                self.browse.apply_filter();
                self.schedule_selection_info();
            }
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.browse.filtered.is_empty() {
            return;
        }
        let len = self.browse.filtered.len() as isize;
        let next = (self.browse.selected as isize + delta).clamp(0, len - 1);
        self.browse.selected = next as usize;
        self.schedule_selection_info();
        self.dirty = true;
    }

    fn set_selection(&mut self, index: usize) {
        if self.browse.filtered.is_empty() {
            return;
        }
        self.browse.selected = index.min(self.browse.filtered.len() - 1);
        self.schedule_selection_info();
        self.dirty = true;
    }

    fn schedule_selection_info(&mut self) {
        self.browse.info_for = self.browse.current_name().map(str::to_string);
        self.sync_info_target();
    }

    fn handle_orphans_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.back_to_menu(),
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.data.orphan_loaded = false;
                self.load_orphans();
                self.toast("Re-reading orphan list…");
            }
            KeyCode::Enter => self.ask_remove_orphans(),
            _ => {}
        }
    }

    fn handle_cache_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.back_to_menu(),
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.load_cache();
                self.toast("Re-scanning package cache…");
            }
            KeyCode::Enter => self.ask_clean_cache(),
            _ => {}
        }
    }

    fn handle_system_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.back_to_menu(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.system_action_index =
                    (self.system_action_index + SYSTEM_ACTIONS.len() - 1) % SYSTEM_ACTIONS.len();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.system_action_index = (self.system_action_index + 1) % SYSTEM_ACTIONS.len();
            }
            KeyCode::Enter => {
                let action = SYSTEM_ACTIONS[self.system_action_index];
                match action {
                    SystemAction::RefreshDatabases => self.ask_confirm(
                        "Refresh package databases",
                        vec!["Download fresh sync databases with pacman -Sy.".to_string()],
                        vec![CmdSpec::privileged(
                            "pacman",
                            ["-Sy", "--noconfirm", "--color=never", "--noprogressbar"],
                        )],
                        PendingAction::RefreshDatabases,
                        false,
                    ),
                    SystemAction::FullUpgrade => self.ask_full_upgrade(),
                    SystemAction::UpdateAur => self.ask_update_aur(),
                    SystemAction::ReloadData => {
                        self.toast("Reloading package data…");
                        self.start_reload();
                    }
                    SystemAction::ClearAurCache => {
                        let path = aur::cache_path(&self.paths.cache_dir);
                        match std::fs::remove_file(&path) {
                            Ok(()) => {
                                self.toast("Cached AUR package list removed");
                                self.data.aur_loaded = false;
                                self.data.loading_aur = false;
                                self.start_loading();
                            }
                            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                                self.toast("AUR cache is already empty");
                            }
                            Err(err) => self.toast_error(format!("Could not remove cache: {err}")),
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn selected_targets(&self) -> Vec<(String, Source)> {
        self.browse
            .marked_or_current()
            .into_iter()
            .map(|row| (row.name.clone(), row.source))
            .collect()
    }

    fn install_specs(&self, targets: &[(String, Source)]) -> Result<Vec<CmdSpec>, String> {
        let mut pacman_names = Vec::new();
        let mut aur_names = Vec::new();
        for (name, source) in targets {
            match source {
                Source::Pacman => pacman_names.push(name.clone()),
                Source::Aur => aur_names.push(name.clone()),
            }
        }
        let mut specs = Vec::new();
        if !pacman_names.is_empty() {
            if !exec::have_sudo() {
                return Err("sudo is not available: cannot install system packages".to_string());
            }
            push_batched(
                &mut specs,
                "pacman",
                &[
                    "-S",
                    "--needed",
                    "--noconfirm",
                    "--color=never",
                    "--noprogressbar",
                ],
                &pacman_names,
                true,
            );
        }
        if !aur_names.is_empty() {
            let helper = exec::aur_helper().ok_or_else(|| {
                "no AUR helper found: install yay or paru to build AUR packages".to_string()
            })?;
            push_batched(
                &mut specs,
                &helper,
                &["-S", "--needed", "--noconfirm"],
                &aur_names,
                false,
            );
        }
        Ok(specs)
    }

    fn remove_specs(&self, names: &[String]) -> Vec<CmdSpec> {
        let mut specs = Vec::new();
        push_batched(
            &mut specs,
            "pacman",
            &["-Rns", "--noconfirm", "--color=never", "--noprogressbar"],
            names,
            true,
        );
        specs
    }

    fn update_specs(&self, targets: &[(String, Source)]) -> Vec<CmdSpec> {
        let mut pacman_names = Vec::new();
        let mut aur_names = Vec::new();
        for (name, source) in targets {
            match source {
                Source::Pacman => pacman_names.push(name.clone()),
                Source::Aur => aur_names.push(name.clone()),
            }
        }
        let mut specs = Vec::new();
        if !pacman_names.is_empty() {
            push_batched(
                &mut specs,
                "pacman",
                &[
                    "-S",
                    "--needed",
                    "--noconfirm",
                    "--color=never",
                    "--noprogressbar",
                ],
                &pacman_names,
                true,
            );
        }
        if !aur_names.is_empty()
            && let Some(helper) = exec::aur_helper()
        {
            push_batched(
                &mut specs,
                &helper,
                &["-S", "--needed", "--noconfirm"],
                &aur_names,
                false,
            );
        }
        specs
    }

    fn downgrade_specs(&self, paths: &[PathBuf]) -> Vec<CmdSpec> {
        let files: Vec<String> = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        let mut specs = Vec::new();
        push_batched(
            &mut specs,
            "pacman",
            &["-U", "--noconfirm", "--color=never", "--noprogressbar"],
            &files,
            true,
        );
        specs
    }

    fn ask_primary_action(&mut self) {
        match self.browse.mode {
            BrowseMode::Install => self.ask_install(),
            BrowseMode::Remove => self.ask_remove(),
            BrowseMode::Update => self.ask_update_selected(),
            BrowseMode::Downgrade => self.ask_downgrade(),
        }
    }

    fn ask_install(&mut self) {
        let targets = self.selected_targets();
        if targets.is_empty() {
            self.toast("Select a package first");
            return;
        }
        let specs = match self.install_specs(&targets) {
            Ok(specs) => specs,
            Err(err) => {
                self.toast_error(err);
                return;
            }
        };
        let names: Vec<String> = targets
            .iter()
            .map(|(name, source)| {
                if *source == Source::Aur {
                    format!("{name} (AUR)")
                } else {
                    name.clone()
                }
            })
            .collect();
        self.ask_confirm(
            &format!("Install {} package(s)", targets.len()),
            summary_lines(&names),
            specs,
            PendingAction::Install(targets),
            false,
        );
    }

    fn ask_remove(&mut self) {
        let names: Vec<String> = self
            .selected_targets()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        if names.is_empty() {
            self.toast("Select a package first");
            return;
        }
        if !exec::have_sudo() {
            self.toast_error("sudo is not available: cannot remove system packages");
            return;
        }
        let specs = self.remove_specs(&names);
        self.ask_confirm(
            &format!("Remove {} package(s)", names.len()),
            summary_lines(&names),
            specs,
            PendingAction::Remove(names),
            true,
        );
    }

    fn ask_update_selected(&mut self) {
        let targets = self.selected_targets();
        if targets.is_empty() {
            self.toast("No updates selected");
            return;
        }
        if targets.iter().any(|(_, source)| *source == Source::Pacman) && !exec::have_sudo() {
            self.toast_error("sudo is not available: cannot upgrade system packages");
            return;
        }
        if targets.iter().any(|(_, source)| *source == Source::Aur) && exec::aur_helper().is_none()
        {
            self.toast_error("no AUR helper found: install yay or paru");
            return;
        }
        let specs = self.update_specs(&targets);
        let summary: Vec<String> = self
            .browse
            .marked_or_current()
            .into_iter()
            .map(|row| match &row.kind {
                RowKind::Update { old, new } => format!("{}: {old} → {new}", row.name),
                _ => row.name.clone(),
            })
            .collect();
        self.ask_confirm(
            &format!("Upgrade {} package(s)", targets.len()),
            summary,
            specs,
            PendingAction::Update(targets),
            false,
        );
    }

    fn ask_update_all(&mut self) {
        let mut specs = Vec::new();
        if exec::have_sudo() {
            specs.push(CmdSpec::privileged(
                "pacman",
                ["-Syu", "--noconfirm", "--color=never", "--noprogressbar"],
            ));
        }
        if let Some(helper) = exec::aur_helper() {
            specs.push(CmdSpec::new(helper, ["-Sua", "--noconfirm"]));
        }
        if specs.is_empty() {
            self.toast_error("sudo is not available: cannot upgrade the system");
            return;
        }
        self.ask_confirm(
            "Full system upgrade",
            vec![
                "This will upgrade every installed package and all AUR packages.".to_string(),
                "Make sure important work is saved before continuing.".to_string(),
            ],
            specs,
            PendingAction::UpdateAll,
            true,
        );
    }

    fn ask_downgrade(&mut self) {
        let rows = self.browse.marked_or_current();
        if rows.is_empty() {
            self.toast("Select a cached package first");
            return;
        }
        let mut paths = Vec::new();
        let mut summary = Vec::new();
        for row in rows {
            if let RowKind::Cached { path, size, .. } = &row.kind {
                paths.push(path.clone());
                let installed = row.installed.as_deref().unwrap_or("not installed");
                summary.push(format!(
                    "{} {} ({}), installed: {installed}",
                    row.name,
                    row.version,
                    format_bytes(*size)
                ));
            }
        }
        if paths.is_empty() {
            return;
        }
        if !exec::have_sudo() {
            self.toast_error("sudo is not available: cannot reinstall cached packages");
            return;
        }
        self.ask_confirm(
            "Reinstall cached package version",
            summary,
            self.downgrade_specs(&paths),
            PendingAction::Downgrade(paths),
            true,
        );
    }

    fn ask_download(&mut self) {
        if !self.browse.mode.supports_download() {
            self.toast("Download is only available on install/update screens");
            return;
        }
        let targets = self.selected_targets();
        if targets.is_empty() {
            self.toast("Select a package first");
            return;
        }
        self.start_download(targets);
    }

    fn ask_remove_orphans(&mut self) {
        if self.data.orphans.is_empty() {
            self.toast("No orphans to remove");
            return;
        }
        if !exec::have_sudo() {
            self.toast_error("sudo is not available: cannot remove orphan packages");
            return;
        }
        let orphans = self.data.orphans.clone();
        let specs = self.remove_specs(&orphans);
        self.ask_confirm(
            &format!("Remove {} orphan(s)", orphans.len()),
            summary_lines(&orphans),
            specs,
            PendingAction::RemoveOrphans(orphans),
            true,
        );
    }

    fn ask_clean_cache(&mut self) {
        if !exec::have_sudo() {
            self.toast_error("sudo is not available: cannot clean the package cache");
            return;
        }
        self.ask_confirm(
            "Clean package cache",
            vec![
                format!(
                    "pacman -Sc will remove cached tarballs that are no longer installed (cache is {}).",
                    format_bytes(self.data.cache_size)
                ),
                "Installed packages are not affected, but downgrades may become impossible.".to_string(),
            ],
            vec![CmdSpec::privileged(
                "pacman",
                ["-Sc", "--noconfirm", "--color=never", "--noprogressbar"],
            )],
            PendingAction::CleanCache,
            true,
        );
    }

    fn ask_full_upgrade(&mut self) {
        self.ask_update_all();
    }

    fn ask_update_aur(&mut self) {
        let Some(helper) = exec::aur_helper() else {
            self.toast_error("no AUR helper found: install yay or paru");
            return;
        };
        self.ask_confirm(
            "Update AUR packages",
            vec![format!(
                "Upgrade all foreign packages through {helper} -Sua."
            )],
            vec![CmdSpec::new(helper, ["-Sua", "--noconfirm"])],
            PendingAction::UpdateAur,
            true,
        );
    }

    fn ask_confirm(
        &mut self,
        title: &str,
        summary: Vec<String>,
        specs: Vec<CmdSpec>,
        action: PendingAction,
        danger: bool,
    ) {
        if specs.is_empty() {
            self.toast_error("Nothing to run");
            return;
        }
        let commands = specs.iter().map(CmdSpec::display).collect();
        self.confirm = Some(ConfirmDialog {
            title: title.to_string(),
            summary,
            commands,
            action,
            danger,
        });
    }

    fn execute_action(&mut self, action: PendingAction) {
        match action {
            PendingAction::Install(targets) => match self.install_specs(&targets) {
                Ok(specs) => {
                    let title = format!("Installing {} package(s)", targets.len());
                    self.start_commands(title, specs, ReloadKind::All);
                }
                Err(err) => self.toast_error(err),
            },
            PendingAction::Remove(names) => {
                let specs = self.remove_specs(&names);
                self.start_commands(
                    format!("Removing {} package(s)", names.len()),
                    specs,
                    ReloadKind::All,
                );
            }
            PendingAction::Update(targets) => {
                let specs = self.update_specs(&targets);
                self.start_commands("Upgrading packages", specs, ReloadKind::All);
            }
            PendingAction::UpdateAll => {
                let mut specs = Vec::new();
                if exec::have_sudo() {
                    specs.push(CmdSpec::privileged(
                        "pacman",
                        ["-Syu", "--noconfirm", "--color=never", "--noprogressbar"],
                    ));
                }
                if let Some(helper) = exec::aur_helper() {
                    specs.push(CmdSpec::new(helper, ["-Sua", "--noconfirm"]));
                }
                self.start_commands("Full system upgrade", specs, ReloadKind::All);
            }
            PendingAction::Downgrade(paths) => {
                let specs = self.downgrade_specs(&paths);
                self.start_commands("Reinstalling cached packages", specs, ReloadKind::All);
            }
            PendingAction::RemoveOrphans(orphans) => {
                let specs = self.remove_specs(&orphans);
                self.start_commands("Removing orphan packages", specs, ReloadKind::All);
            }
            PendingAction::CleanCache => {
                self.start_commands(
                    "Cleaning package cache",
                    vec![CmdSpec::privileged(
                        "pacman",
                        ["-Sc", "--noconfirm", "--color=never", "--noprogressbar"],
                    )],
                    ReloadKind::All,
                );
            }
            PendingAction::RefreshDatabases => {
                self.start_commands(
                    "Refreshing package databases",
                    vec![CmdSpec::privileged(
                        "pacman",
                        ["-Sy", "--noconfirm", "--color=never", "--noprogressbar"],
                    )],
                    ReloadKind::All,
                );
            }
            PendingAction::FullUpgrade => {
                let mut specs = Vec::new();
                if exec::have_sudo() {
                    specs.push(CmdSpec::privileged(
                        "pacman",
                        ["-Syu", "--noconfirm", "--color=never", "--noprogressbar"],
                    ));
                }
                if let Some(helper) = exec::aur_helper() {
                    specs.push(CmdSpec::new(helper, ["-Sua", "--noconfirm"]));
                }
                self.start_commands("Full system upgrade", specs, ReloadKind::All);
            }
            PendingAction::UpdateAur => {
                let Some(helper) = exec::aur_helper() else {
                    self.toast_error("no AUR helper found: install yay or paru");
                    return;
                };
                self.start_commands(
                    "Updating AUR packages",
                    vec![CmdSpec::new(helper, ["-Sua", "--noconfirm"])],
                    ReloadKind::All,
                );
            }
        }
    }

    fn start_commands(
        &mut self,
        title: impl Into<String>,
        specs: Vec<CmdSpec>,
        reload: ReloadKind,
    ) {
        if specs.is_empty() {
            self.toast_error("Nothing to run");
            return;
        }
        let mut operation = Operation::new(title);
        operation
            .lines
            .push((format!("running {} command(s)…", specs.len()), false));
        let cancel = operation.cancel.clone();
        self.overlay = Some(operation);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut last_code = 0;
            let mut cancelled = false;
            let mut failures = 0;
            for spec in &specs {
                let _ = tx.send(AppEvent::OpLine {
                    text: format!("$ {}", spec.display()),
                    error: false,
                });
                let _ = tx.send(AppEvent::OpStatus(format!("running {}", spec.program)));
                match exec::run_streamed(spec, &tx, &cancel).await {
                    RunOutcome::Exited(0) => {}
                    RunOutcome::Exited(code) => {
                        last_code = code;
                        failures += 1;
                        let _ = tx.send(AppEvent::OpLine {
                            text: format!("command exited with code {code}"),
                            error: true,
                        });
                    }
                    RunOutcome::Cancelled => {
                        cancelled = true;
                        break;
                    }
                    RunOutcome::Failed(message) => {
                        last_code = -1;
                        failures += 1;
                        let _ = tx.send(AppEvent::OpLine {
                            text: message,
                            error: true,
                        });
                    }
                }
                if cancelled {
                    break;
                }
            }
            let ok = !cancelled && last_code == 0;
            let message = if cancelled {
                "Cancelled".to_string()
            } else if ok {
                format!("{}: done", specs.len())
            } else {
                format!(
                    "{} command(s) failed (exit code {last_code})",
                    failures.max(1)
                )
            };
            let _ = tx.send(AppEvent::OpDone {
                ok,
                message,
                reload: if cancelled { ReloadKind::None } else { reload },
            });
        });
    }

    fn start_download(&mut self, targets: Vec<(String, Source)>) {
        let count = targets.len();
        let mut operation = Operation::new(format!("Downloading {count} package(s)"));
        operation.status = "resolving download URLs…".to_string();
        let cancel = operation.cancel.clone();
        self.overlay = Some(operation);

        let tx = self.tx.clone();
        let client = self.client.clone();
        let downloads_dir = self.paths.downloads_dir.clone();
        tokio::spawn(async move {
            let downloader = Downloader::new(client.clone());
            let mut completed = 0usize;
            let mut skipped = 0usize;
            let mut failed = 0usize;
            let mut total_files = 0usize;
            for (index, (name, source)) in targets.iter().enumerate() {
                if cancel.is_cancelled() {
                    break;
                }
                let _ = tx.send(AppEvent::OpStatus(format!("resolving {name}…")));
                let urls = match source {
                    Source::Pacman => match pacman::download_urls(name).await {
                        Ok(urls) => urls,
                        Err(err) => {
                            let _ = tx.send(AppEvent::OpLine {
                                text: format!("{name}: {err}"),
                                error: true,
                            });
                            failed += 1;
                            continue;
                        }
                    },
                    Source::Aur => {
                        let url = aur::snapshot_url(name, None);
                        vec![(url, 0u64)]
                    }
                };
                for (url, size) in urls {
                    total_files += 1;
                    let ctx = DlCtx {
                        name,
                        index,
                        count: targets.len(),
                    };
                    let outcome = downloader
                        .fetch(&url, &downloads_dir, size, &ctx, &tx, &cancel)
                        .await;
                    match outcome {
                        DlOutcome::Done(path) => {
                            completed += 1;
                            let _ = tx.send(AppEvent::OpLine {
                                text: format!("saved {}", path.display()),
                                error: false,
                            });
                        }
                        DlOutcome::Skipped(path) => {
                            skipped += 1;
                            let _ = tx.send(AppEvent::OpLine {
                                text: format!("already downloaded: {}", path.display()),
                                error: false,
                            });
                        }
                        DlOutcome::Cancelled => {
                            let _ = tx.send(AppEvent::OpDone {
                                ok: false,
                                message: "Download cancelled".to_string(),
                                reload: ReloadKind::None,
                            });
                            return;
                        }
                        DlOutcome::Failed(err) => {
                            failed += 1;
                            let _ = tx.send(AppEvent::OpLine {
                                text: format!("{name}: {err}"),
                                error: true,
                            });
                        }
                    }
                }
            }
            let _ = tx.send(AppEvent::OpDone {
                ok: failed == 0,
                message: format!(
                    "Downloaded {completed} file(s), {skipped} already present, {failed} failed (of {total_files})"
                ),
                reload: ReloadKind::None,
            });
        });
    }
}

fn push_batched(
    specs: &mut Vec<CmdSpec>,
    program: &str,
    base_args: &[&str],
    items: &[String],
    privileged: bool,
) {
    for chunk in items.chunks(100) {
        let mut args: Vec<String> = base_args.iter().map(|arg| arg.to_string()).collect();
        args.extend(chunk.iter().cloned());
        specs.push(if privileged {
            CmdSpec::privileged(program, args)
        } else {
            CmdSpec::new(program, args)
        });
    }
}

fn summary_lines(items: &[String]) -> Vec<String> {
    const MAX: usize = 12;
    let mut lines: Vec<String> = items.iter().take(MAX).cloned().collect();
    if items.len() > MAX {
        lines.push(format!("… and {} more", items.len() - MAX));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_app() -> App {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        App::new(
            tx,
            Paths {
                cache_dir: PathBuf::from("/tmp/ppm-cache"),
                downloads_dir: PathBuf::from("/tmp/ppm-downloads"),
            },
            reqwest::Client::new(),
        )
    }

    #[test]
    fn filter_ranks_prefix_before_substring() {
        let mut state = BrowseState::new(BrowseMode::Install);
        state.rows = vec![
            Row {
                name: "my-linux".to_string(),
                source: Source::Pacman,
                version: "1".to_string(),
                installed: None,
                kind: RowKind::Package,
            },
            Row {
                name: "linux".to_string(),
                source: Source::Pacman,
                version: "2".to_string(),
                installed: None,
                kind: RowKind::Package,
            },
            Row {
                name: "linux-api-headers".to_string(),
                source: Source::Pacman,
                version: "3".to_string(),
                installed: None,
                kind: RowKind::Package,
            },
        ];
        state.filter = "linux".to_string();
        state.apply_filter();
        let names: Vec<&str> = state
            .filtered
            .iter()
            .map(|index| state.rows[*index].name.as_str())
            .collect();
        assert_eq!(names, ["linux", "linux-api-headers", "my-linux"]);
    }

    #[test]
    fn rows_are_built_from_merged_data() {
        let mut app = sample_app();
        app.data.pacman_packages = vec![
            Package::new("bash", "5.3.20-2", Source::Pacman),
            Package::new("linux", "7.3.6-1", Source::Pacman),
        ];
        app.data.aur_names = vec!["spotify".to_string()];
        app.data.aur_names.sort();
        app.data
            .installed
            .insert("bash".to_string(), "5.3.20-1".to_string());
        app.rebuild_packages();
        app.open_browse(BrowseMode::Install);
        assert_eq!(app.browse.rows.len(), 3);
        let bash = app
            .browse
            .rows
            .iter()
            .find(|row| row.name == "bash")
            .unwrap();
        assert_eq!(app.row_status(bash), Status::Upgrade);
    }

    #[test]
    fn status_uses_aur_versions_when_known() {
        let mut app = sample_app();
        app.data.aur_names = vec!["spotify".to_string()];
        app.data
            .installed
            .insert("spotify".to_string(), "1.2.0-1".to_string());
        app.rebuild_packages();
        app.open_browse(BrowseMode::Install);
        let row = app
            .browse
            .rows
            .iter()
            .find(|row| row.name == "spotify")
            .unwrap();
        assert_eq!(app.row_status(row), Status::Unknown);
        app.data
            .aur_versions
            .insert("spotify".to_string(), "1.3.0-1".to_string());
        let row = app
            .browse
            .rows
            .iter()
            .find(|row| row.name == "spotify")
            .unwrap();
        assert_eq!(app.row_status(row), Status::Upgrade);
    }

    #[test]
    fn marked_rows_are_used_as_action_targets() {
        let mut app = sample_app();
        app.data.pacman_packages = vec![
            Package::new("a", "1", Source::Pacman),
            Package::new("b", "1", Source::Pacman),
        ];
        app.rebuild_packages();
        app.open_browse(BrowseMode::Install);
        app.browse.marked.insert("a".to_string());
        app.browse.marked.insert("b".to_string());
        let targets = app.selected_targets();
        assert_eq!(targets.len(), 2);
    }
}
