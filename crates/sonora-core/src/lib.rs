//! SONORA core: library database, indexing, background analysis, duplicate
//! detection, export and the JSON API shared by the desktop app and the
//! development bridge.
//!
//! Std-only Rust (plus SQLite through FFI), so it builds offline and is
//! fully testable without the desktop shell.

pub mod api;
pub mod app;
pub mod db;
pub mod discogs;
pub mod dj;
pub mod djexport;
pub mod duplicates;
pub mod events;
pub mod export;
pub mod index;
pub mod jobs;
pub mod library;
pub mod logging;
pub mod model;
pub mod net;
pub mod playback;
pub mod playlists;
pub mod secrets;
pub mod settings;
pub mod sha256;
pub mod slskd;
pub mod slskd_service;
pub mod sources;
pub mod trash;
pub mod usb;
pub mod yaml;

pub use api::ApiError;
pub use app::{App, AppConfig, CoreError};
