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
use crate::playlists::{self, Kind, OrganizeBy};
use sonora_engine::analysis::key::Notation;
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

impl From<crate::slskd::SlskdError> for ApiError {
    fn from(e: crate::slskd::SlskdError) -> Self {
        ApiError::new(e.code, e.message)
    }
}

impl From<crate::playlists::PlaylistError> for ApiError {
    fn from(e: crate::playlists::PlaylistError) -> Self {
        use crate::playlists::PlaylistError as E;
        match e {
            E::NotFound(m) => ApiError::not_found(m),
            E::Invalid(m) => ApiError::invalid(m),
            E::Store(s) => s.into(),
        }
    }
}

impl From<crate::db::sqlite::DbError> for ApiError {
    fn from(e: crate::db::sqlite::DbError) -> Self {
        ApiError::new("storage_error", e.to_string())
    }
}

type R = Result<Value, ApiError>;

fn busy_message(n: usize) -> String {
    if n == 1 {
        "This track is being analyzed right now. Try again once it is done, or cancel the scan.".into()
    } else {
        format!("These {n} tracks are being analyzed right now. Try again once they are done, or cancel the scan.")
    }
}

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

/// Where to send playlists, which ones, and how.
fn usb_params(p: &Value) -> Result<(PathBuf, Vec<i64>, crate::usb::SyncOptions), ApiError> {
    let root = PathBuf::from(p.req_str("root").map_err(|e| ApiError::invalid(e.to_string()))?);
    if !root.is_absolute() {
        return Err(ApiError::invalid("root must be an absolute path"));
    }
    if !root.is_dir() {
        return Err(ApiError::not_found(format!("{} is not available", root.display())));
    }
    let ids = ids_param(p, "playlistIds")?;
    let opts = crate::usb::SyncOptions::from_json(p).map_err(ApiError::invalid)?;
    Ok((root, ids, opts))
}

fn usb_tools_problem(plan: &crate::usb::SyncPlan, tools: &sonora_engine::decode::ToolPaths) -> Option<String> {
    let items: Vec<crate::djexport::PlanItem> = plan.files.iter().map(|f| plan.leaves[f.leaf].items[f.item].clone()).collect();
    crate::djexport::check_tools(&plan.options.export, &items, tools).err()
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
            // Tracks no longer in the library are left out; only files gone from disk count as missing.
            let (gone, existing): (Vec<_>, Vec<_>) = library::get_tracks(&app.db.read(), &ids)?.into_iter().partition(|t| t.missing_since.is_some());
            let existing: Vec<i64> = existing.into_iter().map(|t| t.id).collect();
            let missing = gone.len() as i64;
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
            // Removing rows a worker is writing would race with it: tracks
            // being analysed right now stay, and the answer counts them.
            let r = app.scans.remove_tracks(&ids)?;
            if r.removed == 0 && r.busy > 0 {
                return Err(ApiError::new("busy", busy_message(r.busy)));
            }
            if r.removed > 0 {
                app.events.emit("library.changed", &obj! { "reason" => "removed", "count" => r.removed });
            }
            Ok(obj! { "removed" => r.removed, "busy" => r.busy })
        }
        "tracks.trash" => {
            // Moves the files to the system's Trash (where the user can put
            // them back), then removes their tracks from the library.
            let ids = ids_param(p, "ids")?;
            let r = app.scans.trash_tracks(&ids)?;
            if r.removed == 0 && r.failed.is_empty() && r.busy > 0 {
                return Err(ApiError::new("busy", busy_message(r.busy)));
            }
            crate::logging::log_fields(
                "info",
                "library",
                "moved files to the trash",
                obj! { "trashed" => r.trashed, "removed" => r.removed, "failed" => r.failed.len() },
            );
            if r.removed > 0 {
                app.events.emit("library.changed", &obj! { "reason" => "removed", "count" => r.removed });
            }
            let failed: Vec<Value> = r.failed.iter().map(|(id, title, error)| obj! { "id" => *id, "title" => title.clone(), "error" => error.clone() }).collect();
            Ok(obj! { "trashed" => r.trashed, "removed" => r.removed, "failed" => Value::Array(failed), "busy" => r.busy, "trash" => crate::trash::trash_name() })
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
                return Err(ApiError::new(
                    "busy",
                    "Some of this folder's files are being indexed or analyzed right now. Try again in a moment, or cancel the scan first.",
                ));
            };
            app.events.emit("library.changed", &obj! { "reason" => "folder_removed" });
            Ok(obj! { "removedTracks" => n })
        }
        "folders.bringBack" => {
            // Files removed from the library earlier are welcome again: the
            // folder is rescanned to add them.
            let id = id_param(p, "id")?;
            let n = library::bring_back(&app.db.write(), id)?;
            if n > 0 {
                app.scans.start_indexing("folders", IndexJob::Folders(Some(vec![id])));
            }
            app.events.emit("library.changed", &obj! { "reason" => "brought_back", "count" => n });
            Ok(obj! { "count" => n })
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
                // Only what this scan finished: queued or cancelled tracks still
                // carry the flags of an earlier analysis (B31).
                "problems" => "(i.outcome = 'failed' OR (i.outcome = 'done' AND (t.flags LIKE '%,attention,%' OR t.flags LIKE '%,review,%')))",
                "failed" => "i.outcome = 'failed'",
                "clipping" => "i.outcome = 'done' AND t.flags LIKE '%,clipping,%'",
                "source" => "i.outcome = 'done' AND t.flags LIKE '%,source_concern,%'",
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

        // ----------------------------------------------------- playlists
        "playlists.list" => Ok(obj! { "nodes" => playlists::list(&app.db.read())? }),
        "playlists.create" => {
            let kind = Kind::from_id(p.req_str("kind").map_err(|e| ApiError::invalid(e.to_string()))?).ok_or_else(|| ApiError::invalid("kind must be folder, playlist or smart"))?;
            let name = p.req_str("name").map_err(|e| ApiError::invalid(e.to_string()))?;
            let parent = p.get("parentId").and_then(Value::as_i64);
            let index = p.get("index").and_then(Value::as_i64).map(|i| i.max(0) as usize);
            let rules = p.get("rules").filter(|v| !v.is_null());
            let tracks = opt_ids(p, "trackIds")?;
            let c = app.db.write();
            let id = playlists::create(&c, kind, name, parent, index, rules)?;
            if let Some(ids) = tracks {
                if kind == Kind::Playlist && !ids.is_empty() {
                    playlists::add_tracks(&c, id, &ids, None)?;
                }
            }
            let node = playlists::node_json(&c, id)?;
            drop(c);
            app.events.emit("playlists.changed", &obj! { "reason" => "created", "id" => id });
            Ok(node)
        }
        "playlists.update" => {
            let id = id_param(p, "id")?;
            let c = app.db.write();
            if let Some(name) = p.get("name").and_then(Value::as_str) {
                playlists::rename(&c, id, name)?;
            }
            if let Some(rules) = p.get("rules").filter(|v| !v.is_null()) {
                playlists::set_rules(&c, id, rules)?;
            }
            let node = playlists::node_json(&c, id)?;
            drop(c);
            app.events.emit("playlists.changed", &obj! { "reason" => "updated", "id" => id });
            Ok(node)
        }
        "playlists.move" => {
            let id = id_param(p, "id")?;
            let parent = p.get("parentId").and_then(Value::as_i64);
            let index = p.get("index").and_then(Value::as_i64).unwrap_or(i64::MAX).max(0) as usize;
            playlists::move_node(&app.db.write(), id, parent, index)?;
            app.events.emit("playlists.changed", &obj! { "reason" => "moved", "id" => id });
            Ok(obj! { "ok" => true })
        }
        "playlists.delete" => {
            let id = id_param(p, "id")?;
            let n = playlists::delete(&app.db.write(), id)?;
            app.events.emit("playlists.changed", &obj! { "reason" => "deleted", "id" => id });
            Ok(obj! { "deleted" => n })
        }
        "playlists.duplicate" => {
            let id = id_param(p, "id")?;
            let c = app.db.write();
            let copy = playlists::duplicate(&c, id)?;
            let node = playlists::node_json(&c, copy)?;
            drop(c);
            app.events.emit("playlists.changed", &obj! { "reason" => "created", "id" => copy });
            Ok(node)
        }
        "playlists.tracks" => {
            let id = id_param(p, "id")?;
            let c = app.db.read();
            let rows = playlists::tracks(&c, id)?;
            Ok(obj! { "id" => id, "rows" => Value::Array(rows.iter().map(ToJson::to_json).collect()), "total" => rows.len() })
        }
        "playlists.addTracks" => {
            let id = id_param(p, "id")?;
            let ids = ids_param(p, "trackIds")?;
            let index = p.get("index").and_then(Value::as_i64).map(|i| i.max(0) as usize);
            let r = playlists::add_tracks(&app.db.write(), id, &ids, index)?;
            if r.added > 0 {
                app.events.emit("playlists.changed", &obj! { "reason" => "tracks", "id" => id });
            }
            Ok(obj! { "added" => r.added, "already" => r.already })
        }
        "playlists.removeTracks" => {
            let id = id_param(p, "id")?;
            let ids = ids_param(p, "trackIds")?;
            let n = playlists::remove_tracks(&app.db.write(), id, &ids)?;
            app.events.emit("playlists.changed", &obj! { "reason" => "tracks", "id" => id });
            Ok(obj! { "removed" => n })
        }
        "playlists.reorder" => {
            let id = id_param(p, "id")?;
            let ids = ids_param(p, "trackIds")?;
            playlists::reorder(&app.db.write(), id, &ids)?;
            app.events.emit("playlists.changed", &obj! { "reason" => "order", "id" => id });
            Ok(obj! { "ok" => true })
        }
        "playlists.moveTracks" => {
            let id = id_param(p, "id")?;
            let ids = ids_param(p, "trackIds")?;
            let index = p.get("index").and_then(Value::as_i64).unwrap_or(0).max(0) as usize;
            playlists::move_tracks(&app.db.write(), id, &ids, index)?;
            app.events.emit("playlists.changed", &obj! { "reason" => "order", "id" => id });
            Ok(obj! { "ok" => true })
        }
        "playlists.arrange" => {
            let id = id_param(p, "id")?;
            let rising = match p.get("tempo").and_then(Value::as_str).unwrap_or("any") {
                "any" => false,
                "rising" => true,
                _ => return Err(ApiError::invalid("tempo must be any or rising")),
            };
            let start = p.get("start").and_then(Value::as_i64);
            let c = app.db.read();
            if playlists::get(&c, id)?.kind != Kind::Playlist {
                return Err(ApiError::invalid("only a playlist can be arranged: a smart playlist keeps the order of its rules"));
            }
            let rows = playlists::tracks(&c, id)?;
            drop(c);
            if rows.len() > playlists::MAX_ARRANGE {
                return Err(ApiError::invalid(format!("arranging works on playlists of up to {} tracks", playlists::MAX_ARRANGE)));
            }
            let order = playlists::arrange(&rows, start, rising);
            let by_id: std::collections::HashMap<i64, &TrackRow> = rows.iter().map(|r| (r.id, r)).collect();
            let before: Vec<&TrackRow> = rows.iter().collect();
            let after: Vec<&TrackRow> = order.iter().filter_map(|t| by_id.get(t).copied()).collect();
            Ok(obj! { "order" => order, "before" => playlists::summarize(&before), "after" => playlists::summarize(&after) })
        }
        "playlists.freeze" => {
            let id = id_param(p, "id")?;
            let n = playlists::freeze(&app.db.write(), id)?;
            app.events.emit("playlists.changed", &obj! { "reason" => "updated", "id" => id });
            Ok(obj! { "tracks" => n })
        }
        "playlists.organize" => {
            let by = OrganizeBy::from_id(p.req_str("by").map_err(|e| ApiError::invalid(e.to_string()))?).ok_or_else(|| ApiError::invalid("by must be genre, key or tempo"))?;
            let ids = opt_ids(p, "trackIds")?;
            let smart = p.get("smart").and_then(Value::as_bool).unwrap_or(true);
            let notation = match p.get("keyNotation").and_then(Value::as_str) {
                Some(n) => Notation::from_id(n).ok_or_else(|| ApiError::invalid("unknown keyNotation"))?,
                None => Notation::from_id(&app.settings().key_notation).unwrap_or(Notation::Camelot),
            };
            let r = playlists::organize(&app.db.write(), by, ids.as_deref(), smart, notation)?;
            app.events.emit("playlists.changed", &obj! { "reason" => "created", "id" => r.folder_id });
            Ok(r.to_json())
        }
        "playlists.containing" => {
            // Playlists holding one track, or any of several (`trackIds`).
            if p.get("trackIds").is_some() {
                let ids = ids_param(p, "trackIds")?;
                return Ok(obj! { "ids" => playlists::containing_any(&app.db.read(), &ids)? });
            }
            let id = id_param(p, "trackId")?;
            Ok(obj! { "ids" => playlists::containing(&app.db.read(), id)? })
        }

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
                // Either job blocks a new export (B48).
                "running" => app.exporter.is_running() || app.syncer.is_running(),
            })
        }
        "djexport.start" => {
            if app.syncer.is_running() {
                return Err(ApiError::new("busy", "Playlists are being sent to a drive; wait for that to finish."));
            }
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

        // --------------------------------------------------- USB drives
        "usb.drives" => Ok(obj! { "drives" => Value::Array(crate::usb::drives().iter().map(ToJson::to_json).collect()) }),
        "usb.plan" => {
            let (root, ids, opts) = usb_params(p)?;
            let mut plan = crate::usb::plan(&app.db.read(), &root, &ids, &opts)?;
            let tools = app.scans.shared.tools.read().unwrap_or_else(|e| e.into_inner()).clone();
            if plan.problem.is_none() {
                plan.problem = usb_tools_problem(&plan, &tools);
            }
            Ok(plan.to_json(app.syncer.is_running() || app.exporter.is_running()))
        }
        "usb.start" => {
            if app.exporter.is_running() {
                return Err(ApiError::new("busy", "A DJ export is running; wait for it to finish."));
            }
            let (root, ids, opts) = usb_params(p)?;
            let plan = crate::usb::plan(&app.db.read(), &root, &ids, &opts)?;
            let tools = app.scans.shared.tools.read().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(problem) = plan.problem.clone().or_else(|| usb_tools_problem(&plan, &tools)) {
                return Err(ApiError::new("cannot_send", problem));
            }
            let total = plan.files.len();
            if !app.syncer.start(plan, Arc::clone(&app.events), tools, app.config.app_version.clone()) {
                return Err(ApiError::new("busy", "Playlists are already being sent to a drive."));
            }
            Ok(obj! { "accepted" => true, "total" => total })
        }
        "usb.cancel" => Ok(obj! { "ok" => app.syncer.cancel() }),
        "usb.status" => Ok(obj! { "running" => app.syncer.is_running(), "last" => app.syncer.last() }),

        // -------------------------------- Soulseek downloads (sources.rs)
        // The person's own Soulseek client downloads; SONORA watches its
        // download folder and adds finished files. Nothing here talks to
        // the network.
        "sources.status" => Ok(crate::sources::status_json(app)?),
        "sources.accept" => {
            crate::sources::accept(&app.db.write())?;
            crate::logging::log("info", "sources", "lawful-use notice accepted");
            app.sources.wake();
            Ok(crate::sources::status_json(app)?)
        }
        "sources.configure" => {
            let _change = app.settings_change();
            let mut s = app.settings();
            if let Some(v) = p.get("folder") {
                if v.is_null() {
                    s.soulseek_folder = None;
                } else {
                    if crate::sources::accepted_at(&app.db.read()).is_none() {
                        return Err(ApiError::new("not_accepted", "Read and accept the lawful-use notice before choosing a folder."));
                    }
                    let path = v.as_str().ok_or_else(|| ApiError::invalid("folder must be a folder path"))?;
                    let dir = PathBuf::from(path);
                    if !dir.is_absolute() || !dir.is_dir() {
                        return Err(ApiError::invalid("folder must be the full path of an existing folder"));
                    }
                    if let (Ok(chosen), Ok(own)) = (dir.canonicalize(), app.config.data_dir.canonicalize()) {
                        if chosen.starts_with(&own) {
                            return Err(ApiError::invalid("Choose the folder your Soulseek client saves downloads in, not SONORA's own data folder."));
                        }
                    }
                    if s.soulseek_folder.as_deref() != Some(path) {
                        // What is there already was not downloaded since: it stays
                        // out unless the person adds it.
                        crate::sources::note_existing(app, &dir)?;
                    }
                    s.soulseek_folder = Some(path.to_string());
                }
            }
            if let Some(v) = p.get("review") {
                s.soulseek_review = v.as_bool().ok_or_else(|| ApiError::invalid("review must be true or false"))?;
            }
            s.save(&app.db.write())?;
            *app.scans.shared.settings.write().unwrap_or_else(|e| e.into_inner()) = s;
            app.sources.wake();
            Ok(crate::sources::status_json(app)?)
        }
        "sources.addExisting" => {
            let n = crate::sources::add_existing(app)?;
            app.sources.wake();
            Ok(obj! { "added" => n })
        }
        "sources.check" => {
            let r = crate::sources::check(app);
            let mut v = crate::sources::status_json(app)?;
            v.set("found", obj! { "imported" => r.imported as i64, "held" => r.held as i64, "settling" => r.settling as i64 });
            Ok(v)
        }
        "sources.review" => Ok(crate::sources::review_list(&app.db.read(), 1000)?),
        "sources.decide" => {
            let paths: Vec<String> = p
                .get("paths")
                .and_then(Value::as_array)
                .ok_or_else(|| ApiError::invalid("paths must be a list"))?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let add = p.get("add").and_then(Value::as_bool).ok_or_else(|| ApiError::invalid("add must be true or false"))?;
            let (added, declined) = crate::sources::decide(app, &paths, add)?;
            Ok(obj! { "added" => added as i64, "declined" => declined as i64 })
        }

        // ------------------------------------------- slskd (slskd.rs)
        // Searching and downloading through slskd, the Soulseek client the
        // person runs. SONORA sends slskd instructions; slskd does the
        // networking. Downloads arrive through the folder watcher above.
        "slskd.status" => Ok(app.slskd.status_json(app)?),
        "slskd.configure" => {
            if let Some(v) = p.get("url") {
                let url = v.as_str().ok_or_else(|| ApiError::invalid("url must be a web address"))?;
                let url = crate::slskd::normalize_url(url).map_err(ApiError::invalid)?;
                let _change = app.settings_change();
                let mut s = app.settings();
                s.slskd_url = url;
                s.save(&app.db.write())?;
                *app.scans.shared.settings.write().unwrap_or_else(|e| e.into_inner()) = s;
            }
            match p.get("apiKey") {
                None => {}
                Some(Value::Null) => app.slskd.set_key(app, None)?,
                Some(v) => {
                    let key = v.as_str().map(str::trim).ok_or_else(|| ApiError::invalid("apiKey must be text or null"))?;
                    crate::slskd::check_key(key).map_err(ApiError::invalid)?;
                    app.slskd.set_key(app, Some(key))?;
                }
            }
            Ok(app.slskd.status_json(app)?)
        }
        "slskd.newKey" => {
            // A key for the person to add to slskd.yml; saved here at once.
            let key = crate::secrets::random_key(32).map_err(|e| ApiError::new("internal", e))?;
            app.slskd.set_key(app, Some(&key))?;
            Ok(obj! { "key" => key })
        }
        "slskd.test" => match app.slskd.test(app) {
            // SONORA's own slskd has a different key in slskd.yml (the setup
            // ran twice, say): SONORA takes the one slskd uses and asks again.
            Err(e) if e.code == "slskd_unauthorized" && app.service.adopt_key(app) => Ok(app.slskd.test(app)?),
            r => Ok(r?),
        },
        "slskd.reconnect" => {
            app.slskd.connect(app)?;
            Ok(obj! { "ok" => true })
        }

        // ------------------------------- slskd set up by SONORA (slskd_service.rs)
        // macOS: SONORA downloads slskd, writes its settings, runs it with a
        // launch agent and keeps it healthy. The Soulseek password only
        // passes through to slskd.yml; it is never logged or returned.
        "slskd.managed" => {
            if p.get("ensure").and_then(Value::as_bool) == Some(true) {
                if let Err(e) = app.service.ensure(app) {
                    crate::logging::log_fields("warn", "slskd", "could not start slskd", obj! { "error" => e });
                }
            }
            Ok(app.service.status(app))
        }
        "slskd.setupPlan" => Ok(app.service.plan(app)),
        "slskd.setupStart" => {
            let req = crate::slskd_service::Request::from_json(p).map_err(ApiError::invalid)?;
            app.service.start(app, req).map_err(|e| ApiError::new("setup_failed", e))
        }
        "slskd.setupRetry" => {
            let password = p.get("password").and_then(Value::as_str).map(str::to_string);
            app.service.retry(app, password).map_err(|e| ApiError::new("setup_failed", e))
        }
        "slskd.setupCancel" => {
            app.service.cancel();
            Ok(obj! { "ok" => true })
        }
        "slskd.setupDiscard" => app.service.discard(app).map_err(|e| ApiError::new("setup_failed", e)),
        "slskd.service" => {
            let action = p.req_str("action").map_err(|e| ApiError::invalid(e.to_string()))?;
            if !["start", "stop", "restart"].contains(&action) {
                return Err(ApiError::invalid("action must be start, stop or restart"));
            }
            app.service.action(app, action).map_err(|e| ApiError::new("slskd_service", e))?;
            Ok(app.service.status(app))
        }
        "slskd.background" => {
            let on = p.req_bool("enabled").map_err(|e| ApiError::invalid(e.to_string()))?;
            app.service.set_background(app, on).map_err(|e| ApiError::new("slskd_service", e))?;
            Ok(app.service.status(app))
        }
        "slskd.removeAgent" => {
            let r = app.service.remove_agent(app).map_err(|e| ApiError::new("slskd_service", e))?;
            let mut v = app.service.status(app);
            v.set("kept", r.get("kept").cloned().unwrap_or(Value::Null));
            Ok(v)
        }
        "slskd.repair" => {
            app.service.repair(app).map_err(|e| ApiError::new("slskd_service", e))?;
            Ok(app.service.status(app))
        }
        "slskd.password" => {
            let pw = p.req_str("password").map_err(|e| ApiError::invalid(e.to_string()))?;
            app.service.change_password(app, pw).map_err(|e| ApiError::new("slskd_service", e))?;
            Ok(app.service.status(app))
        }
        "slskd.stopOther" => {
            let pid = p.req_i64("pid").map_err(|e| ApiError::invalid(e.to_string()))?;
            let pid = u32::try_from(pid).map_err(|_| ApiError::invalid("pid must be a process id"))?;
            app.service.stop_other(app, pid).map_err(|e| ApiError::new("slskd_service", e))?;
            Ok(app.service.status(app))
        }
        "slskd.quitSoulseekQt" => {
            app.service.quit_soulseek_qt().map_err(|e| ApiError::new("slskd_service", e))?;
            Ok(app.service.status(app))
        }
        "slskd.webPassword" => Ok(obj! { "password" => app.service.web_password(app) }),
        "slskd.search" => {
            if crate::sources::accepted_at(&app.db.read()).is_none() {
                return Err(ApiError::new("not_accepted", "Read and accept the notice on the Soulseek page before searching."));
            }
            let text = p.get("text").and_then(Value::as_str).map(str::trim).unwrap_or("");
            if text.is_empty() || text.chars().count() > 200 || text.chars().any(char::is_control) {
                return Err(ApiError::invalid("Type between 1 and 200 characters to search for."));
            }
            Ok(app.slskd.search(app, text)?)
        }
        "slskd.results" => {
            let id = p.get("id").and_then(Value::as_str).ok_or_else(|| ApiError::invalid("id must be a search id"))?;
            let lossless = p.get("lossless").and_then(Value::as_bool).unwrap_or(true);
            Ok(app.slskd.results(app, id, lossless)?)
        }
        "slskd.stop" => {
            let id = p.get("id").and_then(Value::as_str).ok_or_else(|| ApiError::invalid("id must be a search id"))?;
            app.slskd.stop(app, id)?;
            Ok(obj! { "ok" => true })
        }
        "slskd.download" => {
            if crate::sources::accepted_at(&app.db.read()).is_none() {
                return Err(ApiError::new("not_accepted", "Read and accept the notice on the Soulseek page before downloading."));
            }
            let username = p.get("username").and_then(Value::as_str).filter(|u| !u.trim().is_empty()).ok_or_else(|| ApiError::invalid("username must be a Soulseek user name"))?;
            let files = p.get("files").and_then(Value::as_array).ok_or_else(|| ApiError::invalid("files must be a list"))?;
            if files.is_empty() || files.len() > 1000 {
                return Err(ApiError::invalid("choose between 1 and 1000 files"));
            }
            let mut list = Vec::with_capacity(files.len());
            for f in files {
                let name = f.get("filename").and_then(Value::as_str).filter(|n| !n.is_empty()).ok_or_else(|| ApiError::invalid("each file needs its filename"))?;
                let size = f.get("size").and_then(Value::as_i64).filter(|s| *s >= 0).ok_or_else(|| ApiError::invalid("each file needs its size"))?;
                list.push((name.to_string(), size));
            }
            // Files being previewed: their preview hands over to the download,
            // which resumes from what the preview already fetched. If slskd
            // does not queue a file, its preview fetches on its own again.
            let names: Vec<String> = list.iter().map(|(f, _)| f.clone()).collect();
            app.previews.hand_over(app, username, &names);
            match app.slskd.download(app, username, &list) {
                Ok(v) => {
                    let failed: Vec<String> = v.get("failedFiles").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                    if !failed.is_empty() {
                        app.previews.take_back(username, &failed);
                    }
                    Ok(v)
                }
                Err(e) => {
                    app.previews.take_back(username, &names);
                    Err(e.into())
                }
            }
        }

        // ---------------------------------------- previews (preview.rs)
        // Listening to a Soulseek file before downloading it: never added to
        // the library, never kept.
        "preview.start" => {
            if crate::sources::accepted_at(&app.db.read()).is_none() {
                return Err(ApiError::new("not_accepted", "Read and accept the notice on the Soulseek page before listening."));
            }
            let username = p.get("username").and_then(Value::as_str).filter(|u| !u.trim().is_empty()).ok_or_else(|| ApiError::invalid("username must be a Soulseek user name"))?;
            let filename = p.get("filename").and_then(Value::as_str).filter(|f| !f.is_empty()).ok_or_else(|| ApiError::invalid("filename must be the file's path"))?;
            let size = p.get("size").and_then(Value::as_i64).filter(|s| *s >= 0).ok_or_else(|| ApiError::invalid("size must be the file's size"))?;
            let length = p.get("length").and_then(Value::as_f64);
            Ok(app.previews.start(app, username, filename, size as u64, length)?)
        }
        "preview.status" => {
            let id = id_param(p, "id")?;
            app.previews.get(id as u64).map(|s| s.to_json()).ok_or_else(|| ApiError::not_found("That preview ended."))
        }
        "preview.position" => {
            let id = id_param(p, "id")?;
            let seconds = p.get("seconds").and_then(Value::as_f64).ok_or_else(|| ApiError::invalid("seconds must be a number"))?;
            Ok(obj! { "ok" => app.previews.position(id as u64, seconds) })
        }
        "preview.stop" => {
            let id = id_param(p, "id")?;
            Ok(obj! { "ok" => app.previews.stop(app, id as u64) })
        }
        "preview.list" => Ok(Value::Array(app.previews.list())),
        "slskd.downloads" => Ok(app.slskd.downloads(app)?),
        "slskd.cancel" => {
            let username = p.get("username").and_then(Value::as_str).ok_or_else(|| ApiError::invalid("username must be a Soulseek user name"))?;
            let id = p.get("id").and_then(Value::as_str).ok_or_else(|| ApiError::invalid("id must be a transfer id"))?;
            app.slskd.cancel(app, username, id)?;
            Ok(obj! { "ok" => true })
        }
        "slskd.clearFinished" => {
            app.slskd.clear_finished(app)?;
            Ok(obj! { "ok" => true })
        }

        // ---------------------------- YouTube playlists from Soulseek (jobs.rs)
        // The person pastes a playlist link; SONORA reads the list from
        // YouTube's page, looks for each track through slskd and downloads
        // the pick by the rules in Settings.
        "playlist.fetch" => {
            if crate::sources::accepted_at(&app.db.read()).is_none() {
                return Err(ApiError::new("not_accepted", "Read and accept the notice on the Soulseek page first."));
            }
            let source = p.get("source").and_then(Value::as_str).map(str::trim).unwrap_or("");
            if source.is_empty() || source.chars().count() > 500 {
                return Err(ApiError::invalid("Paste the link of a YouTube playlist."));
            }
            let id = app.playlist.fetch(app, source)?;
            Ok(obj! { "id" => id })
        }
        "playlist.list" => Ok(obj! { "runs" => app.playlist.list(app)? }),
        "playlist.get" => app.playlist.get(app, id_param(p, "id")?),
        "playlist.delete" => {
            app.playlist.delete(app, id_param(p, "id")?)?;
            Ok(obj! { "ok" => true })
        }
        "playlist.include" => {
            let id = id_param(p, "id")?;
            let items = ids_param(p, "itemIds")?;
            let included = p.req_bool("included").map_err(|e| ApiError::invalid(e.to_string()))?;
            app.playlist.include(app, id, &items, included)
        }
        "playlist.start" => {
            if app.slskd.key(app)?.is_none() {
                return Err(ApiError::new("slskd_not_set_up", "Set slskd up on the Soulseek page first."));
            }
            app.playlist.start(app, id_param(p, "id")?)
        }
        "playlist.pause" => app.playlist.pause(app, id_param(p, "id")?),
        "playlist.resume" => app.playlist.resume_run(app, id_param(p, "id")?),
        "playlist.cancel" => app.playlist.cancel(app, id_param(p, "id")?),
        "playlist.confirm" => app.playlist.confirm(app, id_param(p, "id")?),
        "playlist.choose" => {
            let id = id_param(p, "id")?;
            let item = id_param(p, "itemId")?;
            let candidate = id_param(p, "candidate")?;
            app.playlist.choose(app, id, item, candidate)
        }
        "playlist.retry" => {
            let id = id_param(p, "id")?;
            let items = opt_ids(p, "itemIds")?;
            app.playlist.retry(app, id, items.as_deref())
        }

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
            let _change = app.settings_change();
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
            // `replace`: the person chose an existing file in a save dialog that asked first.
            let replace = p.get("replace").and_then(Value::as_bool).unwrap_or(false);
            app.db.backup_to(&dest, replace).map_err(|e| ApiError::new("backup_failed", e.to_string()))?;
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
