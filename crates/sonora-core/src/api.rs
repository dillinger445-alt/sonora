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
                    v.set("outcome", r.text(43));
                    v.set("finishedAt", r.opt_i64(44));
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

        // ------------------------------------------------------ settings
        "settings.get" => Ok(app.settings().to_json()),
        "settings.update" => {
            let values = p.get("values").and_then(Value::as_object).ok_or_else(|| ApiError::invalid("values must be an object"))?;
            let mut s = app.settings();
            for (k, v) in values.iter() {
                s.apply(k, v).map_err(ApiError::invalid)?;
            }
            s.save(&app.db.write())?;
            *app.scans.shared.settings.write().unwrap_or_else(|e| e.into_inner()) = s.clone();
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
