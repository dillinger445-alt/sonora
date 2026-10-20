//! The application core: owns the database, scan manager and event bus,
//! and exposes the JSON API to the desktop shell and the dev bridge.

use crate::api::{self, ApiError};
use crate::db::Db;
use crate::duplicates;
use crate::events::{EventBus, Listener};
use crate::jobs::{IndexJob, ScanManager};
use crate::logging;
use crate::settings::Settings;
use sonora_engine::decode::ToolPaths;
use sonora_json::{obj, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Where the database and logs live.
    pub data_dir: PathBuf,
    /// Extra directories searched for FFmpeg (e.g. the app's sidecar folder).
    pub tool_dirs: Vec<PathBuf>,
    pub app_version: String,
}

#[derive(Debug)]
pub struct CoreError(pub String);

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CoreError {}

pub struct App {
    pub config: AppConfig,
    pub db: Arc<Db>,
    pub events: Arc<EventBus>,
    pub scans: ScanManager,
    pub dup_running: Arc<AtomicBool>,
    dup_cancel: Arc<AtomicBool>,
    pub exporter: Arc<crate::djexport::Exporter>,
    pub syncer: Arc<crate::usb::Syncer>,
    pub discogs: Arc<crate::discogs::Discogs>,
}

impl App {
    pub fn open(config: AppConfig) -> Result<Arc<App>, CoreError> {
        std::fs::create_dir_all(&config.data_dir).map_err(|e| CoreError(format!("cannot create {}: {e}", config.data_dir.display())))?;
        logging::init(&config.data_dir.join("logs"));
        let db = Arc::new(Db::open(&config.data_dir.join("sonora.db")).map_err(|e| CoreError(format!("cannot open the library database: {e}")))?);
        let recovered = ScanManager::recover(&db).map_err(|e| CoreError(e.to_string()))?;
        let settings = Settings::load(&db.read());
        let tools = ToolPaths::discover(&config.tool_dirs);
        logging::log_fields(
            "info",
            "app",
            "core started",
            obj! {
                "version" => config.app_version,
                "dataDir" => config.data_dir.to_string_lossy().to_string(),
                "ffmpeg" => tools.ffmpeg.as_ref().map(|p| p.to_string_lossy().to_string()),
                "sqlite" => crate::db::sqlite::library_version(),
                "recoveredTracks" => recovered,
            },
        );
        // After the upgrade that added DJ metadata, fill in the values in use
        // (from tags until tracks are analyzed again).
        {
            let c = db.write();
            let pending = c.query_row("SELECT value FROM meta WHERE key = 'dj_refresh'", crate::params![], |r| r.text(0)).ok().flatten().is_some();
            if pending {
                match crate::dj::refresh(&c, None, &settings.dj_config()) {
                    Ok(_) => {
                        let _ = c.execute("DELETE FROM meta WHERE key = 'dj_refresh'", crate::params![]);
                    }
                    Err(e) => logging::log("error", "app", &format!("could not prepare DJ metadata: {e}")),
                }
            }
        }
        let events = Arc::new(EventBus::default());
        let version = config.app_version.clone();
        let rescan = settings.rescan_on_launch == "changed";
        let scans = ScanManager::new(Arc::clone(&db), Arc::clone(&events), tools, settings);
        let app = Arc::new(App {
            config,
            db,
            events,
            scans,
            dup_running: Arc::new(AtomicBool::new(false)),
            dup_cancel: Arc::new(AtomicBool::new(false)),
            exporter: Arc::new(crate::djexport::Exporter::default()),
            syncer: Arc::new(crate::usb::Syncer::default()),
            discogs: Arc::new(crate::discogs::Discogs::new(&version)),
        });
        // Work interrupted by a crash or by quitting mid-scan continues where
        // it stopped. Cancelled work is not resumed.
        match app.interrupted_work() {
            Ok(ids) if !ids.is_empty() => {
                let n = app.scans.enqueue_tracks("resume", &ids).queued;
                logging::log_fields("info", "app", "resumed interrupted analysis", obj! { "tracks" => { n } });
            }
            Ok(_) => {}
            Err(e) => logging::log("error", "app", &format!("could not check for interrupted work: {e}")),
        }
        // Scores from an older scoring model are refreshed from the stored
        // measurements in the background (no audio is decoded).
        {
            let app2 = Arc::clone(&app);
            let _ = std::thread::Builder::new().name("sonora-rescore".into()).spawn(move || {
                match crate::library::rescore_outdated(&app2.db, &sonora_engine::assess::ScoringConfig::default()) {
                    Ok(0) => {}
                    Ok(n) => {
                        logging::log_fields("info", "app", "re-scored analyses with the current scoring model", obj! { "tracks" => n as i64 });
                        app2.events.emit("library.changed", &obj! { "reason" => "rescored", "count" => n as i64 });
                    }
                    Err(e) => logging::log("error", "app", &format!("re-scoring failed: {e}")),
                }
            });
        }
        if rescan {
            let has_folders = app.db.read().query_row("SELECT COUNT(*) FROM folders", crate::params![], |r| r.i64(0)).ok().flatten().unwrap_or(0) > 0;
            if has_folders {
                app.scans.start_indexing("startup", IndexJob::Folders(None));
            }
        }
        Ok(app)
    }

    /// Tracks still waiting from the most recent interrupted scan. They are
    /// marked as resumed so a later cancel is respected on the next launch.
    fn interrupted_work(&self) -> Result<Vec<i64>, crate::db::StoreError> {
        let c = self.db.write();
        let Some(scan) = c.query_row(
            "SELECT MAX(scan_id) FROM scan_items WHERE outcome = 'interrupted'",
            crate::params![],
            |r| r.opt_i64(0),
        )?
        .flatten() else {
            return Ok(Vec::new());
        };
        let ids = c.query(
            "SELECT i.track_id FROM scan_items i JOIN tracks t ON t.id = i.track_id \
             WHERE i.scan_id = ? AND i.outcome = 'interrupted' AND t.status IN ('pending', 'done') AND t.missing_since IS NULL ORDER BY i.seq",
            crate::params![scan],
            |r| r.i64(0),
        )?;
        c.execute("UPDATE scan_items SET outcome = 'resumed' WHERE outcome = 'interrupted'", crate::params![])?;
        Ok(ids)
    }

    /// JSON in, JSON out.
    pub fn call(self: &Arc<Self>, method: &str, params_json: &str) -> Result<String, ApiError> {
        let params = if params_json.trim().is_empty() {
            Value::object()
        } else {
            sonora_json::parse(params_json).map_err(|e| ApiError::new("invalid_params", format!("params are not valid JSON: {e}")))?
        };
        let started = std::time::Instant::now();
        let result = api::dispatch(self, method, &params);
        if let Err(e) = &result {
            if e.code != "not_found" && e.code != "invalid_params" && e.code != "busy" {
                logging::log_fields("warn", "api", &e.message, obj! { "method" => method, "code" => e.code.clone() });
            }
        }
        let elapsed = started.elapsed();
        if elapsed > Duration::from_millis(500) {
            logging::log_fields("info", "api", "slow call", obj! { "method" => method, "ms" => elapsed.as_millis() as i64 });
        }
        result.map(|v| v.to_string_compact())
    }

    pub fn subscribe(&self, listener: Listener) -> u64 {
        self.events.subscribe(listener)
    }

    pub fn unsubscribe(&self, id: u64) {
        self.events.unsubscribe(id)
    }

    /// Cancels running work and waits briefly for workers to stop.
    pub fn shutdown(&self) {
        self.dup_cancel.store(true, Ordering::Relaxed);
        self.exporter.cancel();
        self.syncer.cancel();
        self.discogs.cancel();
        self.scans.shutdown(Duration::from_secs(5));
        logging::log("info", "app", "core stopped");
    }

    pub fn settings(&self) -> Settings {
        self.scans.shared.settings.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn refresh_tools(&self) {
        let tools = ToolPaths::discover(&self.config.tool_dirs);
        *self.scans.shared.tools.write().unwrap_or_else(|p| p.into_inner()) = tools;
    }

    pub fn info(&self) -> Value {
        let tools = self.scans.shared.tools.read().unwrap_or_else(|p| p.into_inner()).clone();
        obj! {
            "version" => self.config.app_version,
            "engineVersion" => sonora_engine::analysis::ENGINE_VERSION,
            "modelVersion" => sonora_engine::assess::SCORE_MODEL_VERSION,
            "measurementSchema" => sonora_engine::analysis::measure::MEASUREMENT_SCHEMA,
            "dataDir" => self.config.data_dir.to_string_lossy().to_string(),
            "dbPath" => self.db.path().to_string_lossy().to_string(),
            "sqlite" => crate::db::sqlite::library_version(),
            "ffmpeg" => obj! {
                "available" => tools.ffmpeg.is_some(),
                "path" => tools.ffmpeg.as_ref().map(|p| p.to_string_lossy().to_string()),
                "version" => tools.ffmpeg_version.clone(),
            },
            "platform" => std::env::consts::OS,
            "arch" => std::env::consts::ARCH,
            "cpus" => std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            "nativeFormats" => vec!["WAV", "AIFF", "FLAC"],
            "ffmpegFormats" => vec!["MP3", "AAC/M4A", "ALAC", "Ogg Vorbis", "Opus", "WMA", "WavPack", "APE"],
        }
    }

    /// Runs fingerprint matching across the library in the background.
    /// Starts comparing fingerprints in the background. Returns whether a
    /// comparison is now running (false only if it could not be started).
    pub fn start_duplicate_scan(self: &Arc<Self>) -> bool {
        if self.dup_running.swap(true, Ordering::SeqCst) {
            return true;
        }
        self.dup_cancel.store(false, Ordering::Relaxed);
        let app = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("sonora-duplicates".into()).spawn(move || {
            let events = Arc::clone(&app.events);
            let mut last = std::time::Instant::now();
            let result = duplicates::compute_matches(&app.db, &app.dup_cancel, |f| {
                if last.elapsed() > Duration::from_millis(250) {
                    last = std::time::Instant::now();
                    events.emit("duplicates.progress", &obj! { "progress" => sonora_json::num(f, 3) });
                }
            });
            let error = match result {
                Ok(matches) if !app.dup_cancel.load(Ordering::Relaxed) => duplicates::store_matches(&app.db.write(), &matches).err().map(|e| e.to_string()),
                Ok(_) => None,
                Err(e) => Some(e.to_string()),
            };
            if let Some(e) = &error {
                logging::log("error", "duplicates", e);
            }
            app.dup_running.store(false, Ordering::SeqCst);
            // The view says when the comparison did not complete.
            app.events.emit("duplicates.updated", &match error {
                Some(e) => obj! { "error" => e },
                None => obj! {},
            });
        });
        if spawned.is_err() {
            self.dup_running.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }
}
