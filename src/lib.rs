//! PPM — Powerful Package Manager.
//!
//! A ratatui TUI that presents every package from the configured pacman
//! repositories together with the AUR as one flat, searchable list, and can
//! download or install/remove/upgrade/downgrade packages through pacman and an
//! AUR helper.

pub mod app;
pub mod download;
pub mod event;
pub mod exec;
pub mod model;
pub mod repo;
pub mod snapshot;
pub mod ui;
