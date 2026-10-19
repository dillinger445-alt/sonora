//! The JSON API used by the interface (through Tauri IPC or the dev bridge).
//!
//! `dispatch(app, method, params)` validates parameters and returns JSON.
//! Errors carry a stable machine `code` and a readable `message`.

use crate::app::App;
use crate::db::StoreError;
use crate::duplicates;
use crate::export;
use crate::index;
use crate::jobs::IndexJob;
use crate::library::{self, Filter};
use crate::model::TrackRow;
use crate::params;
use sonora_json::{obj, ToJson, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(code: &str, message: impl Into<String>) -> ApiError {
        ApiError { code: code.to_string(), message: message.into() }
    }
    fn invalid(message: impl Into<String>) -> ApiError {
        ApiError::new("invalid_params", message)
    }
    fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new("not_found", message)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        ApiError::new("storage_error", e.to_string())
    }
}

impl From<crate::db::sqlite::DbError> for ApiError {
    fn from(e: crate::db::sqlite::DbError) -> Self {
        ApiError::new("storage_error", e.to_string())
    }
}

type R = Result<Value, ApiError>;

fn ids_param(p: &Value, key: &str) -> Result<Vec<i64>, ApiError> {
    match p.get(key) {
        Some(Value::Array(a)) => {
            if a.len() > 1_000_000 {
                return Err(ApiError::invalid(format!("{key}: too many ids")));
            }
            a.iter().map(|v| v.as_i64().ok_or_else(|| ApiError::invalid(format!("{key} must contain integers")))).collect()
        }
        _ => Err(ApiError::invalid(format!("{key} must be an array of track ids"))),
    }
}

fn opt_ids(p: &Value, key: &str) -> Result<Option<Vec<i64>>, ApiError> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => ids_param(p, key).map(Some),
    }
}

fn id_param(p: &Value, key: &str) -> Result<i64, ApiError> {
    p.req_i64(key).map_err(|e| ApiError::invalid(e.to_string()))
}

fn paths_param(p: &Value) -> Result<Vec<PathBuf>, ApiError> {
    let arr = p.req_array("paths").map_err(|e| ApiError::invalid(e.to_string()))?;
    if arr.is_empty() {
        return Err(ApiError::invalid("paths must not be empty"));
    }
    let mut out = Vec::new();
    for v in arr {
        let s = v.as_str().ok_or_else(|| ApiError::invalid("paths must contain strings"))?;
        out.push(index::validate_path(s).map_err(ApiError::invalid)?);
    }
    Ok(out)
}

/// Reveals a file in Finder / File Explorer / the Linux file manager.
pub fn reveal(path: &Path) -> Result<(), ApiError> {
    if !path.exists() {
        return Err(ApiError::not_found(format!("{} no longer exists", path.display())));
    }
    let mut cmd;
    #[cfg(target_os = "macos")]
    {
        cmd = std::process::Command::new("open");
        cmd.arg("-R").arg(path);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd = std::process::Command::new("explorer");
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        cmd = std::process::Command::new("xdg-open");
        cmd.arg(path.parent().unwrap_or(path));
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.spawn().map(|_| ()).map_err(|e| ApiError::new("reveal_failed", format!("could not open the file manager: {e}")))
}

/// Response to a request to analyze tracks.
fn queued_json(q: crate::jobs::Queued) -> R {
    if q.refused {
        return Err(ApiError::new("busy", "The cancelled scan is still stopping. Try again in a moment."));
    }
    Ok(obj! { "queued" => q.queued, "alreadyQueued" => q.already })
}

pub fn dispatch(app: &Arc<App>, method: &str, p: &Value) -> R {
    match method {
        "app.info" => Ok(app.info()),
        "app.overview" => Ok(library::overview(&app.db.read())?),

        // ------------------------------------------------------- library
        "library.import" => {
            let paths = paths_param(p)?;
            let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
            let n = paths.len();
            app.scans.start_indexing("import", if force { IndexJob::Force(paths) } else { IndexJob::Paths(paths) });
            Ok(obj! { "accepted" => true, "paths" => n })
        }
        "library.query" => {
            let filter = Filter::from_json(p.get("filter")).map_err(ApiError::invalid)?;
            let sort = p.get("sort");
            let key = sort.and_then(|s| s.get("key")).and_then(Value::as_str).unwrap_or("title");
            if library::sort_expr(key).is_none() {
                return Err(ApiError::invalid(format!("unknown sort key '{key}'")));
            }
            let desc = sort.and_then(|s| s.get("desc")).and_then(Value::as_bool).unwrap_or(false);
            let offset = p.get("offset").and_then(Value::as_i64).unwrap_or(0);
            let limit = p.get("limit").and_then(Value::as_i64).unwrap_or(200);
            let (total, rows) = library::query_tracks(&app.db.read(), &filter, key, desc, offset, limit)?;
            Ok(obj! { "total" => total, "offset" => offset, "rows" => rows })
        }
        "library.ids" => {
            let filter = Filter::from_json(p.get("filter")).map_err(ApiError::invalid)?;
            Ok(obj! { "ids" => library::query_ids(&app.db.read(), &filter)? })
        }
        "library.facets" => Ok(library::facets(&app.db.read())?),

        // -------------------------------------------------------- tracks
        "tracks.get" => {
            let id = id_param(p, "id")?;
            let visuals = p.get("visuals").and_then(Value::as_bool).unwrap_or(true);
            library::track_detail(&app.db.read(), id, visuals)?.ok_or_else(|| ApiError::not_found(format!("track {id} not found")))
        }
        "tracks.rescan" => {
            let ids = ids_param(p, "ids")?;
            let existing: Vec<i64> = library::get_tracks(&app.db.read(), &ids)?.into_iter().filter(|t| t.missing_since.is_none()).map(|t| t.id).collect();
            let missing = (ids.len() - existing.len()) as i64;
            let mut v = queued_json(app.scans.enqueue_tracks("rescan", &existing))?;
            v.set("missing", missing);
            Ok(v)
        }
        "tracks.retryFailed" => {
            let ids = match opt_ids(p, "ids")? {
                Some(ids) => ids,
                None => app.db.read().query("SELECT id FROM tracks WHERE status = 'failed' AND missing_since IS NULL ORDER BY id", params![], |r| r.i64(0))?,
            };
            queued_json(app.scans.enqueue_tracks("retry", &ids))
        }
        "tracks.analyzePending" => {
            let ids = app.db.read().query("SELECT id FROM tracks WHERE status = 'pending' AND missing_since IS NULL ORDER BY id", params![], |r| r.i64(0))?;
            queued_json(app.scans.enqueue_tracks("pending", &ids))
        }
        "tracks.remove" => {
            let ids = ids_param(p, "ids")?;
            // Removing rows a worker is writing would race with it.
            let Some(n) = app.scans.remove_tracks(&ids)? else {
                return Err(ApiError::new("busy", "Some of these tracks are being analyzed right now. Wait for them to finish or cancel the scan, then try again."));
            };
            app.events.emit("library.changed", &obj! { "reason" => "removed", "count" => n });
            Ok(obj! { "removed" => n })
        }
        "tracks.editDj" => {
            let ids = ids_param(p, "ids")?;
            let edit = crate::dj::Edit::from_json(p).map_err(ApiError::invalid)?;
            if edit.is_empty() {
                return Err(ApiError::invalid("nothing to change: give bpm, bpmFactor, key, genre or reset"));
            }
            let cfg = app.settings().dj_config();
            let n = crate::dj::apply_edit(&app.db.write(), &ids, &edit, &cfg)?;
            app.events.emit("library.changed", &obj! { "reason" => "dj_edit", "count" => n as i64 });
            Ok(obj! { "updated" => n as i64 })
        }
        "tracks.analyzeDj" => {
            // Analyses made before tempo and key detection existed.
            let ids = app.db.read().query(
                "SELECT id FROM tracks WHERE status = 'done' AND dj_version IS NULL AND missing_since IS NULL ORDER BY id",
                params![],
                |r| r.i64(0),
            )?;
            queued_json(app.scans.enqueue_tracks("dj", &ids))
        }
        "playback.prepare" => {
            let id = id_param(p, "id")?;
            let t = library::get_track(&app.db.read(), id)?.ok_or_else(|| ApiError::not_found(format!("track {id} not found")))?;
            let playable: Vec<String> = p
                .get("playable")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let decode = p.get("decode").and_then(Value::as_bool).unwrap_or(false);
            let tools = app.scans.shared.tools.read().unwrap_or_else(|e| e.into_inner()).clone();
            let cache = app.config.data_dir.join("cache").join("playback");
            let prepared = crate::playback::prepare(&cache, &t, &playable, decode, &tools).map_err(|m| ApiError::new("playback_failed", m))?;
            let mut v = prepared.to_json();
            v.set("trackId", id);
            v.set("duration", t.duration.map(|d| sonora_json::num(d, 3)).unwrap_or(Value::Null));
            Ok(v)
        }
        "tracks.reveal" => {
            let id = id_param(p, "id")?;
            let t = library::get_track(&app.db.read(), id)?.ok_or_else(|| ApiError::not_found(format!("track {id} not found")))?;
            reveal(Path::new(&t.path))?;
            Ok(obj! { "ok" => true })
        }

        // ------------------------------------------------------- folders
        "folders.list" => Ok(library::folders(&app.db.read())?),
        "folders.rescan" => {
            let ids = opt_ids(p, "ids")?;
            app.scans.start_indexing("folders", IndexJob::Folders(ids));
            Ok(obj! { "accepted" => true })
        }
        "folders.remove" => {
            let id = id_param(p, "id")?;
            let remove_tracks = p.get("removeTracks").and_then(Value::as_bool).unwrap_or(false);
            let Some(n) = app.scans.remove_folder(id, remove_tracks)? else {
                return Err(ApiError::new("busy", "A scan is running. Wait for it to finish or cancel it before removing a folder's tracks."));
            };
            app.events.emit("library.changed", &obj! { "reason" => "folder_removed" });
            Ok(obj! { "removedTracks" => n })
        }

        // --------------------------------------------------------- scans
        "scan.status" => Ok(app.scans.status()),
        "scan.pause" => Ok(obj! { "ok" => app.scans.pause() }),
        "scan.resume" => Ok(obj! { "ok" => app.scans.resume() }),
        "scan.cancel" => Ok(obj! { "ok" => app.scans.cancel() }),
        "scan.history" => {
            let limit = p.get("limit").and_then(Value::as_i64).unwrap_or(50).clamp(1, 500);
            let rows = app.db.read().query(
                "SELECT id, kind, state, started_at, finished_at, total, completed, failed, skipped, roots, cancelled, summary FROM scans ORDER BY id DESC LIMIT ?",
                params![limit],
                |r| {
                    let mut v = library::scan_row_json(r);
                    v.set("cancelled", r.i64(10));
                    v.set("summary", r.opt_text(11).and_then(|s| sonora_json::parse(&s).ok()).unwrap_or(Value::Null));
                    v
                },
            )?;
            Ok(Value::Array(rows))
        }
        "scan.items" => {
            let id = id_param(p, "id")?;
            let filter = p.get("filter").and_then(Value::as_str).unwrap_or("all");
            let cond = match filter {
                "all" => "i.outcome IN ('done', 'failed')",
                "problems" => "(i.outcome = 'failed' OR t.flags LIKE '%,attention,%' OR t.flags LIKE '%,review,%')",
                "failed" => "i.outcome = 'failed'",
                "clipping" => "t.flags LIKE '%,clipping,%'",
                "source" => "t.flags LIKE '%,source_concern,%'",
                other => return Err(ApiError::invalid(format!("unknown filter '{other}'"))),
            };
            let limit = p.get("limit").and_then(Value::as_i64).unwrap_or(200).clamp(1, 2000);
            let offset = p.get("offset").and_then(Value::as_i64).unwrap_or(0).max(0);
            let c = app.db.read();
            let total = c.query_row(&format!("SELECT COUNT(*) FROM scan_items i JOIN tracks t ON t.id = i.track_id WHERE i.scan_id = ? AND {cond}"), params![id], |r| r.i64(0))?.unwrap_or(0);
            let cols: String = crate::model::TRACK_COLUMNS.split(", ").map(|c| format!("t.{}", c.trim())).collect::<Vec<_>>().join(", ");
            let rows = c.query(
                &format!("SELECT {cols}, i.outcome, i.finished_at FROM scan_items i JOIN tracks t ON t.id = i.track_id WHERE i.scan_id = ? AND {cond} ORDER BY i.finished_at DESC, i.seq DESC LIMIT ? OFFSET ?"),
                params![id, limit, offset],
                |r| {
                    let mut v = TrackRow::from_row(r).to_json();
                    v.set("outcome", r.text(crate::model::TRACK_COLUMN_COUNT));
                    v.set("finishedAt", r.opt_i64(crate::model::TRACK_COLUMN_COUNT + 1));
                    v
                },
            )?;
            Ok(obj! { "total" => total, "rows" => Value::Array(rows) })
        }

        // ---------------------------------------------------- duplicates
        "duplicates.list" => {
            let mut v = duplicates::list(&app.db.read())?;
            v.set("scanning", app.dup_running.load(Ordering::Relaxed));
            Ok(v)
        }
        "duplicates.scan" => Ok(obj! { "accepted" => app.start_duplicate_scan() }),

        // ---------------------------------------------------- DJ export
        "djexport.plan" => {
            let ids = ids_param(p, "ids")?;
            let opts = crate::djexport::ExportOptions::from_json(p).map_err(ApiError::invalid)?;
            let items = crate::djexport::plan(&app.db.read(), &ids, &opts)?;
            let tools = app.scans.shared.tools.read().unwrap_or_else(|e| e.into_inner()).clone();
            let problem = crate::djexport::check_tools(&opts, &items, &tools).err();
            let exported = items.iter().filter(|i| i.skip.is_none()).count();
            Ok(obj! {
                "items" => Value::Array(items.iter().map(ToJson::to_json).collect()),
                "exported" => exported,
                "skipped" => items.len() - exported,
                "betterCopies" => items.iter().filter(|i| i.skip.is_none() && i.source.id != i.track.id).count(),
                "withWarnings" => items.iter().filter(|i| i.skip.is_none() && !i.warnings.is_empty()).count(),
                "problem" => problem,
                "running" => app.exporter.is_running(),
            })
        }
        "djexport.start" => {
            let ids = ids_param(p, "ids")?;
            let opts = crate::djexport::ExportOptions::from_json(p).map_err(ApiError::invalid)?;
            let folder = PathBuf::from(p.req_str("folder").map_err(|e| ApiError::invalid(e.to_string()))?);
            if !folder.is_absolute() {
                return Err(ApiError::invalid("folder must be an absolute path"));
            }
            if !folder.is_dir() {
                return Err(ApiError::new("not_found", format!("the folder {} does not exist", folder.display())));
            }
            let items = crate::djexport::plan(&app.db.read(), &ids, &opts)?;
            let tools = app.scans.shared.tools.read().unwrap_or_else(|e| e.into_inner()).clone();
            crate::djexport::check_tools(&opts, &items, &tools).map_err(|m| ApiError::new("tool_missing", m))?;
            let total = items.iter().filter(|i| i.skip.is_none()).count();
            if total == 0 {
                return Err(ApiError::invalid("none of these tracks can be exported (their files are missing)"));
            }
            let playlist = p.get("playlistName").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(120).collect());
            let job = crate::djexport::Job { items, folder, options: opts, app_version: app.config.app_version.clone(), playlist };
            if !app.exporter.start(job, Arc::clone(&app.events), tools) {
                return Err(ApiError::new("busy", "Another export is still running."));
            }
            Ok(obj! { "accepted" => true, "total" => total })
        }
        "djexport.cancel" => Ok(obj! { "ok" => app.exporter.cancel() }),
        "djexport.status" => Ok(obj! { "running" => app.exporter.is_running(), "last" => app.exporter.last() }),

        // ------------------------------------------------------ Discogs
        "discogs.status" => Ok(obj! {
            "configured" => crate::discogs::Discogs::token(&app.db).is_some(),
            "running" => app.discogs.is_running(),
            "last" => app.discogs.last(),
        }),
        "discogs.setToken" => {
            let token = match p.get("token") {
                None | Some(Value::Null) => None,
                Some(v) => Some(v.as_str().ok_or_else(|| ApiError::invalid("token must be text or null"))?),
            };
            crate::discogs::Discogs::set_token(&app.db, token).map_err(ApiError::invalid)?;
            Ok(obj! { "configured" => crate::discogs::Discogs::token(&app.db).is_some() })
        }
        "discogs.test" => {
            let token = crate::discogs::Discogs::token(&app.db).ok_or_else(|| ApiError::invalid("no Discogs token is set"))?;
            let name = app.discogs.identity(&token).map_err(|e| ApiError::new("discogs_failed", e.to_string()))?;
            Ok(obj! { "username" => name })
        }
        "discogs.lookup" => {
            let ids = ids_param(p, "ids")?;
            if ids.is_empty() {
                return Err(ApiError::invalid("choose at least one track"));
            }
            if crate::discogs::Discogs::token(&app.db).is_none() {
                return Err(ApiError::new("not_configured", "Add your Discogs token in Settings → Genres first."));
            }
            let overwrite = p.get("overwrite").and_then(Value::as_bool).unwrap_or(false);
            let dj = app.settings().dj_config();
            if !app.discogs.start(Arc::clone(&app.db), Arc::clone(&app.events), ids.clone(), overwrite, dj) {
                return Err(ApiError::new("busy", "A Discogs lookup is already running."));
            }
            Ok(obj! { "accepted" => true, "total" => ids.len() })
        }
        "discogs.cancel" => Ok(obj! { "ok" => app.discogs.cancel() }),

        // ------------------------------------------------------ settings
        "settings.get" => Ok(app.settings().to_json()),
        "settings.update" => {
            let values = p.get("values").and_then(Value::as_object).ok_or_else(|| ApiError::invalid("values must be an object"))?;
            let mut s = app.settings();
            for (k, v) in values.iter() {
                s.apply(k, v).map_err(ApiError::invalid)?;
            }
            let before = app.settings().dj_config();
            s.save(&app.db.write())?;
            *app.scans.shared.settings.write().unwrap_or_else(|e| e.into_inner()) = s.clone();
            if s.dj_config() != before {
                // The tempo range or folder genres changed what is shown.
                let n = crate::dj::refresh(&app.db.write(), None, &s.dj_config())?;
                app.events.emit("library.changed", &obj! { "reason" => "dj_settings", "count" => n as i64 });
            }
            Ok(s.to_json())
        }
        "tools.refresh" => {
            app.refresh_tools();
            Ok(app.info())
        }

        // ------------------------------------------------- export/cache
        "export.csv" => {
            let rows: Vec<TrackRow> = match opt_ids(p, "ids")? {
                Some(ids) => library::get_tracks(&app.db.read(), &ids)?,
                None => {
                    let filter = Filter::from_json(p.get("filter")).map_err(ApiError::invalid)?;
                    let ids = library::query_ids(&app.db.read(), &filter)?;
                    library::get_tracks(&app.db.read(), &ids)?
                }
            };
            let csv = export::to_csv(&rows);
            match p.get("path").and_then(Value::as_str) {
                Some(path) => {
                    let dest = PathBuf::from(path);
                    if !dest.is_absolute() || dest.file_name().is_none() {
                        return Err(ApiError::invalid("path must be an absolute file path"));
                    }
                    if dest.is_dir() || !dest.parent().map(Path::is_dir).unwrap_or(false) {
                        return Err(ApiError::invalid("the destination folder does not exist"));
                    }
                    std::fs::write(&dest, csv.as_bytes()).map_err(|e| ApiError::new("io_error", format!("could not write {}: {e}", dest.display())))?;
                    Ok(obj! { "path" => dest.to_string_lossy().to_string(), "rows" => rows.len() })
                }
                None => Ok(obj! { "csv" => csv, "rows" => rows.len() }),
            }
        }
        "cache.stats" => {
            let c = app.db.read();
            let (analyses, visuals) = c
                .query_row("SELECT COUNT(*), COALESCE(SUM(LENGTH(visuals)), 0) FROM analyses", params![], |r| (r.i64(0), r.i64(1)))?
                .unwrap_or((0, 0));
            Ok(obj! { "dbBytes" => app.db.size_bytes(), "visualsBytes" => visuals, "analyses" => analyses, "dbPath" => app.db.path().to_string_lossy().to_string() })
        }
        "cache.clearVisuals" => {
            if app.scans.is_busy() {
                return Err(ApiError::new("busy", "Wait for the current scan to finish before clearing cached visualizations."));
            }
            let c = app.db.write();
            let n = c.execute("UPDATE analyses SET visuals = NULL WHERE visuals IS NOT NULL", params![])?;
            c.execute_batch("VACUUM")?;
            Ok(obj! { "cleared" => n })
        }
        "db.backup" => {
            let path = p.req_str("path").map_err(|e| ApiError::invalid(e.to_string()))?;
            let dest = PathBuf::from(path);
            if !dest.is_absolute() || !dest.parent().map(Path::is_dir).unwrap_or(false) {
                return Err(ApiError::invalid("path must be an absolute file path in an existing folder"));
            }
            app.db.backup_to(&dest).map_err(|e| ApiError::new("backup_failed", e.to_string()))?;
            Ok(obj! { "path" => path })
        }
        "system.reveal" => {
            let path = p.req_str("path").map_err(|e| ApiError::invalid(e.to_string()))?;
            let pb = PathBuf::from(path);
            if !pb.is_absolute() {
                return Err(ApiError::invalid("path must be absolute"));
            }
            reveal(&pb)?;
            Ok(obj! { "ok" => true })
        }
        other => Err(ApiError::new("unknown_method", format!("unknown method '{other}'"))),
    }
}
