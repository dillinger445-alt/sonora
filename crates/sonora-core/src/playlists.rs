//! Playlists: a tree of folders, playlists and smart playlists.
//!
//! - A **folder** holds playlists, smart playlists and other folders.
//! - A **playlist** holds tracks in the order the user chose, each once.
//! - A **smart playlist** is a saved library filter and sort: it lists
//!   whatever the library holds that matches, at the time it is read.
//!
//! Playlists only refer to library tracks. Deleting a playlist never
//! touches a track or a file; removing a track from the library removes it
//! from every playlist.
//!
//! [`arrange`] orders tracks for mixing: neighbouring tracks close in tempo
//! and in keys that mix (the Camelot wheel).

use crate::db::now_ms;
use crate::db::sqlite::{Connection, DbError, SqlValue};
use crate::db::StoreError;
use crate::library::{as_params, sort_expr, Filter};
use crate::model::{TrackRow, TRACK_COLUMNS};
use crate::params;
use sonora_engine::analysis::key::{camelot, camelot_distance, format_key, long_name, Notation};
use sonora_json::{num, obj, ToJson, Value};
use std::collections::{HashMap, HashSet};

/// Longest name, in characters.
pub const MAX_NAME: usize = 120;
/// Most tracks a playlist holds, and most a smart playlist lists.
pub const MAX_TRACKS: usize = 10_000;
/// Most playlists one "organize" creates.
const MAX_GROUPS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Playlist,
    Smart,
}

impl Kind {
    pub fn from_id(s: &str) -> Option<Kind> {
        match s {
            "folder" => Some(Kind::Folder),
            "playlist" => Some(Kind::Playlist),
            "smart" => Some(Kind::Smart),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Kind::Folder => "folder",
            Kind::Playlist => "playlist",
            Kind::Smart => "smart",
        }
    }
}

#[derive(Debug)]
pub enum PlaylistError {
    NotFound(String),
    Invalid(String),
    Store(StoreError),
}

impl std::fmt::Display for PlaylistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaylistError::NotFound(m) | PlaylistError::Invalid(m) => f.write_str(m),
            PlaylistError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl From<StoreError> for PlaylistError {
    fn from(e: StoreError) -> Self {
        PlaylistError::Store(e)
    }
}

impl From<DbError> for PlaylistError {
    fn from(e: DbError) -> Self {
        PlaylistError::Store(StoreError::Db(e))
    }
}

type R<T> = Result<T, PlaylistError>;

fn invalid<T>(m: impl Into<String>) -> R<T> {
    Err(PlaylistError::Invalid(m.into()))
}

// ---------------------------------------------------------------- rules

/// What a smart playlist lists: a library filter, its order, and at most
/// `limit` tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct Rules {
    pub filter: Filter,
    pub sort_key: String,
    pub desc: bool,
    pub limit: Option<usize>,
    /// The rules as stored (the filter as the interface wrote it).
    pub json: Value,
}

pub fn parse_rules(v: &Value) -> Result<Rules, String> {
    if v.as_object().is_none() {
        return Err("rules must be an object".into());
    }
    let filter_json = v.get("filter").cloned().unwrap_or_else(Value::object);
    if filter_json.get("playlistId").is_some() {
        return Err("a smart playlist cannot depend on another playlist".into());
    }
    let filter = Filter::from_json(Some(&filter_json))?;
    let sort = v.get("sort");
    let sort_key = sort.and_then(|s| s.get("key")).and_then(Value::as_str).unwrap_or("bpm").to_string();
    if sort_expr(&sort_key).is_none() {
        return Err(format!("unknown sort key '{sort_key}'"));
    }
    let desc = sort.and_then(|s| s.get("desc")).and_then(Value::as_bool).unwrap_or(false);
    let limit = match v.get("limit") {
        None | Some(Value::Null) => None,
        Some(l) => Some(l.as_i64().filter(|n| (1..=MAX_TRACKS as i64).contains(n)).ok_or(format!("limit must be between 1 and {MAX_TRACKS}"))? as usize),
    };
    let json = obj! {
        "filter" => filter_json,
        "sort" => obj! { "key" => sort_key.clone(), "desc" => desc },
        "limit" => limit,
    };
    Ok(Rules { filter, sort_key, desc, limit, json })
}

// ---------------------------------------------------------------- nodes

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub kind: Kind,
    pub name: String,
    pub position: i64,
    pub rules: Option<Value>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Node {
    fn from_row(r: &crate::db::sqlite::Row) -> Node {
        Node {
            id: r.i64(0),
            parent_id: r.opt_i64(1),
            kind: Kind::from_id(&r.text(2)).unwrap_or(Kind::Playlist),
            name: r.text(3),
            position: r.i64(4),
            rules: r.opt_text(5).and_then(|s| sonora_json::parse(&s).ok()),
            created_at: r.i64(6),
            updated_at: r.i64(7),
        }
    }
}

const NODE_COLUMNS: &str = "id, parent_id, kind, name, position, rules, created_at, updated_at";

pub fn get(c: &Connection, id: i64) -> R<Node> {
    c.query_row(&format!("SELECT {NODE_COLUMNS} FROM playlists WHERE id = ?"), params![id], Node::from_row)?
        .ok_or_else(|| PlaylistError::NotFound(format!("playlist {id} not found")))
}

fn clean_name(name: &str) -> R<String> {
    let cleaned: String = name.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        return invalid("a name is needed");
    }
    if cleaned.chars().count() > MAX_NAME {
        return invalid(format!("names are at most {MAX_NAME} characters"));
    }
    Ok(cleaned)
}

/// Siblings under `parent`, in order.
fn children(c: &Connection, parent: Option<i64>) -> R<Vec<i64>> {
    Ok(c.query("SELECT id FROM playlists WHERE parent_id IS ? ORDER BY position, id", params![parent], |r| r.i64(0))?)
}

fn renumber(c: &Connection, order: &[i64]) -> R<()> {
    for (i, id) in order.iter().enumerate() {
        c.execute("UPDATE playlists SET position = ? WHERE id = ? AND position != ?", params![i as i64, *id, i as i64])?;
    }
    Ok(())
}

fn check_parent(c: &Connection, parent: Option<i64>) -> R<()> {
    if let Some(p) = parent {
        let node = get(c, p)?;
        if node.kind != Kind::Folder {
            return invalid("playlists go into folders, not into other playlists");
        }
    }
    Ok(())
}

/// Creates a node at `index` among its siblings (the end when `None`).
pub fn create(c: &Connection, kind: Kind, name: &str, parent: Option<i64>, index: Option<usize>, rules: Option<&Value>) -> R<i64> {
    c.transaction(|tx| insert_node(tx, kind, name, parent, index, rules))
}

/// [`create`] inside a transaction the caller holds.
fn insert_node(c: &Connection, kind: Kind, name: &str, parent: Option<i64>, index: Option<usize>, rules: Option<&Value>) -> R<i64> {
    let name = clean_name(name)?;
    let rules_json = match (kind, rules) {
        (Kind::Smart, Some(r)) => Some(parse_rules(r).map_err(PlaylistError::Invalid)?.json.to_string_compact()),
        (Kind::Smart, None) => return invalid("a smart playlist needs rules"),
        (_, Some(_)) => return invalid("only smart playlists have rules"),
        (_, None) => None,
    };
    check_parent(c, parent)?;
    let now = now_ms();
    c.execute(
        "INSERT INTO playlists (parent_id, kind, name, position, rules, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![parent, kind.id(), name, i64::MAX, rules_json, now, now],
    )?;
    let id = c.last_insert_rowid();
    let mut order: Vec<i64> = children(c, parent)?.into_iter().filter(|&x| x != id).collect();
    order.insert(index.unwrap_or(order.len()).min(order.len()), id);
    renumber(c, &order)?;
    Ok(id)
}

pub fn rename(c: &Connection, id: i64, name: &str) -> R<()> {
    let name = clean_name(name)?;
    get(c, id)?;
    c.execute("UPDATE playlists SET name = ?, updated_at = ? WHERE id = ?", params![name, now_ms(), id])?;
    Ok(())
}

pub fn set_rules(c: &Connection, id: i64, rules: &Value) -> R<()> {
    let node = get(c, id)?;
    if node.kind != Kind::Smart {
        return invalid("only smart playlists have rules");
    }
    let json = parse_rules(rules).map_err(PlaylistError::Invalid)?.json.to_string_compact();
    c.execute("UPDATE playlists SET rules = ?, updated_at = ? WHERE id = ?", params![json, now_ms(), id])?;
    Ok(())
}

/// Moves a node into `parent` (a folder, or the top level) at `index`.
pub fn move_node(c: &Connection, id: i64, parent: Option<i64>, index: usize) -> R<()> {
    c.transaction(|tx| {
        let node = get(tx, id)?;
        check_parent(tx, parent)?;
        // A folder cannot go inside itself.
        let mut up = parent;
        while let Some(p) = up {
            if p == id {
                return invalid("a folder cannot go inside itself");
            }
            up = get(tx, p)?.parent_id;
        }
        let old_parent = node.parent_id;
        tx.execute("UPDATE playlists SET parent_id = ?, updated_at = ? WHERE id = ?", params![parent, now_ms(), id])?;
        if old_parent != parent {
            renumber(tx, &children(tx, old_parent)?.into_iter().filter(|&x| x != id).collect::<Vec<_>>())?;
        }
        let mut order: Vec<i64> = children(tx, parent)?.into_iter().filter(|&x| x != id).collect();
        order.insert(index.min(order.len()), id);
        renumber(tx, &order)
    })
}

/// The node and everything under it, parents before children.
fn subtree(c: &Connection, id: i64) -> R<Vec<Node>> {
    let mut out = vec![get(c, id)?];
    let mut i = 0;
    while i < out.len() {
        let parent = out[i].id;
        out.extend(c.query(&format!("SELECT {NODE_COLUMNS} FROM playlists WHERE parent_id = ? ORDER BY position, id"), params![parent], Node::from_row)?);
        i += 1;
    }
    Ok(out)
}

/// Deletes a node and everything under it; returns how many nodes went.
/// Tracks and files are not touched.
pub fn delete(c: &Connection, id: i64) -> R<usize> {
    c.transaction(|tx| {
        let nodes = subtree(tx, id)?;
        let parent = nodes[0].parent_id;
        // Children first is not needed: the foreign key cascades.
        tx.execute("DELETE FROM playlists WHERE id = ?", params![id])?;
        renumber(tx, &children(tx, parent)?)?;
        Ok(nodes.len())
    })
}

/// Copies a node (with its tracks, rules or contents) right after it.
pub fn duplicate(c: &Connection, id: i64) -> R<i64> {
    c.transaction(|tx| {
        let nodes = subtree(tx, id)?;
        let root = &nodes[0];
        let mut map: HashMap<i64, i64> = HashMap::new();
        let now = now_ms();
        for (i, n) in nodes.iter().enumerate() {
            let (parent, name, position) = if i == 0 {
                (n.parent_id, copy_name(&n.name), n.position + 1)
            } else {
                (n.parent_id.and_then(|p| map.get(&p).copied()), n.name.clone(), n.position)
            };
            tx.execute(
                "INSERT INTO playlists (parent_id, kind, name, position, rules, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![parent, n.kind.id(), name, position, n.rules.as_ref().map(|r| r.to_string_compact()), now, now],
            )?;
            let new_id = tx.last_insert_rowid();
            map.insert(n.id, new_id);
            if n.kind == Kind::Playlist {
                tx.execute(
                    "INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at) SELECT ?, track_id, position, ? FROM playlist_tracks WHERE playlist_id = ?",
                    params![new_id, now, n.id],
                )?;
            }
        }
        let copy = map[&root.id];
        let mut order: Vec<i64> = children(tx, root.parent_id)?.into_iter().filter(|&x| x != copy).collect();
        let at = order.iter().position(|&x| x == root.id).map(|p| p + 1).unwrap_or(order.len());
        order.insert(at, copy);
        renumber(tx, &order)?;
        Ok(copy)
    })
}

fn copy_name(name: &str) -> String {
    let n: String = name.chars().take(MAX_NAME - 5).collect();
    format!("{n} copy")
}

// --------------------------------------------------------------- tracks

/// `TRACK_COLUMNS` with a table prefix ("t.id, t.path, …").
fn prefixed_columns(prefix: &str) -> String {
    TRACK_COLUMNS.split(',').map(|col| format!("{prefix}.{}", col.trim())).collect::<Vec<_>>().join(", ")
}

/// A smart playlist's rules as a query: `WHERE` and `ORDER BY … LIMIT`.
fn smart_sql(rules: &Rules) -> (String, Vec<SqlValue>, String) {
    let (w, p) = rules.filter.sql();
    let expr = sort_expr(&rules.sort_key).unwrap_or("dj_bpm");
    let dir = if rules.desc { "DESC" } else { "ASC" };
    let limit = rules.limit.unwrap_or(MAX_TRACKS).min(MAX_TRACKS);
    (w, p, format!("ORDER BY ({expr}) IS NULL, {expr} {dir}, sort_title, id LIMIT {limit}"))
}

fn rules_of(node: &Node) -> R<Rules> {
    let v = node.rules.as_ref().ok_or_else(|| PlaylistError::Invalid(format!("the smart playlist “{}” has no rules", node.name)))?;
    parse_rules(v).map_err(PlaylistError::Invalid)
}

/// The tracks of a playlist or smart playlist, in order.
pub fn tracks(c: &Connection, id: i64) -> R<Vec<TrackRow>> {
    let node = get(c, id)?;
    match node.kind {
        Kind::Folder => invalid("a folder holds playlists, not tracks"),
        Kind::Playlist => Ok(c.query(
            &format!(
                "SELECT {} FROM playlist_tracks pt JOIN tracks t ON t.id = pt.track_id WHERE pt.playlist_id = ? ORDER BY pt.position LIMIT {MAX_TRACKS}",
                prefixed_columns("t")
            ),
            params![id],
            TrackRow::from_row,
        )?),
        Kind::Smart => {
            let (w, p, order) = smart_sql(&rules_of(&node)?);
            Ok(c.query(&format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE {w} {order}"), &as_params(&p), TrackRow::from_row)?)
        }
    }
}

/// Track ids of a playlist or smart playlist, in order.
pub fn track_ids(c: &Connection, id: i64) -> R<Vec<i64>> {
    let node = get(c, id)?;
    match node.kind {
        Kind::Folder => invalid("a folder holds playlists, not tracks"),
        Kind::Playlist => Ok(c.query("SELECT track_id FROM playlist_tracks WHERE playlist_id = ? ORDER BY position", params![id], |r| r.i64(0))?),
        Kind::Smart => {
            let (w, p, order) = smart_sql(&rules_of(&node)?);
            Ok(c.query(&format!("SELECT id FROM tracks WHERE {w} {order}"), &as_params(&p), |r| r.i64(0))?)
        }
    }
}

fn require_playlist(c: &Connection, id: i64) -> R<Node> {
    let node = get(c, id)?;
    match node.kind {
        Kind::Playlist => Ok(node),
        Kind::Smart => invalid("a smart playlist lists what matches its rules; convert it to a playlist to change its tracks by hand"),
        Kind::Folder => invalid("tracks go into playlists, not folders"),
    }
}

/// A playlist member: the track and when it was added.
type Member = (i64, i64);

/// Members with the time each was added, in order.
fn members(c: &Connection, id: i64) -> R<Vec<Member>> {
    Ok(c.query("SELECT track_id, added_at FROM playlist_tracks WHERE playlist_id = ? ORDER BY position", params![id], |r| (r.i64(0), r.i64(1)))?)
}

fn write_members(c: &Connection, id: i64, order: &[(i64, i64)]) -> R<()> {
    c.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?", params![id])?;
    for (i, (track, added)) in order.iter().enumerate() {
        c.execute("INSERT INTO playlist_tracks (playlist_id, track_id, position, added_at) VALUES (?, ?, ?, ?)", params![id, *track, i as i64, *added])?;
    }
    c.execute("UPDATE playlists SET updated_at = ? WHERE id = ?", params![now_ms(), id])?;
    Ok(())
}

fn existing_tracks(c: &Connection, ids: &[i64]) -> R<HashSet<i64>> {
    let mut out = HashSet::new();
    for chunk in ids.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let p: Vec<SqlValue> = chunk.iter().map(|&i| SqlValue::Int(i)).collect();
        out.extend(c.query(&format!("SELECT id FROM tracks WHERE id IN ({marks})"), &as_params(&p), |r| r.i64(0))?);
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Added {
    pub added: usize,
    /// Already in the playlist (each track is in a playlist once).
    pub already: usize,
}

/// Adds tracks at `index` (the end when `None`), in the order given.
pub fn add_tracks(c: &Connection, id: i64, ids: &[i64], index: Option<usize>) -> R<Added> {
    c.transaction(|tx| {
        require_playlist(tx, id)?;
        let mut order = members(tx, id)?;
        let present: HashSet<i64> = order.iter().map(|m| m.0).collect();
        let known = existing_tracks(tx, ids)?;
        let mut seen = HashSet::new();
        let now = now_ms();
        let new: Vec<(i64, i64)> = ids.iter().filter(|t| known.contains(t) && !present.contains(t) && seen.insert(**t)).map(|&t| (t, now)).collect();
        let already = ids.iter().filter(|t| present.contains(t)).collect::<HashSet<_>>().len();
        if order.len() + new.len() > MAX_TRACKS {
            return invalid(format!("a playlist holds at most {MAX_TRACKS} tracks"));
        }
        let added = new.len();
        if added > 0 {
            let at = index.unwrap_or(order.len()).min(order.len());
            order.splice(at..at, new);
            write_members(tx, id, &order)?;
        }
        Ok(Added { added, already })
    })
}

pub fn remove_tracks(c: &Connection, id: i64, ids: &[i64]) -> R<usize> {
    c.transaction(|tx| {
        require_playlist(tx, id)?;
        let gone: HashSet<i64> = ids.iter().copied().collect();
        let order = members(tx, id)?;
        let kept: Vec<(i64, i64)> = order.iter().copied().filter(|m| !gone.contains(&m.0)).collect();
        let removed = order.len() - kept.len();
        if removed > 0 {
            write_members(tx, id, &kept)?;
        }
        Ok(removed)
    })
}

/// Puts the tracks in the order given, which must list every track once.
pub fn reorder(c: &Connection, id: i64, ids: &[i64]) -> R<()> {
    c.transaction(|tx| {
        require_playlist(tx, id)?;
        let current = members(tx, id)?;
        let added: HashMap<i64, i64> = current.iter().copied().collect();
        let unique: HashSet<i64> = ids.iter().copied().collect();
        if ids.len() != current.len() || unique.len() != ids.len() || !ids.iter().all(|t| added.contains_key(t)) {
            return invalid("the playlist changed meanwhile: the new order must list each of its tracks once");
        }
        let order: Vec<(i64, i64)> = ids.iter().map(|t| (*t, added[t])).collect();
        if order != current {
            write_members(tx, id, &order)?;
        }
        Ok(())
    })
}

/// Moves some tracks, in their current order, to `index` in the list of
/// the tracks that stay.
pub fn move_tracks(c: &Connection, id: i64, ids: &[i64], index: usize) -> R<()> {
    c.transaction(|tx| {
        require_playlist(tx, id)?;
        let moving: HashSet<i64> = ids.iter().copied().collect();
        let current = members(tx, id)?;
        let (moved, mut rest): (Vec<Member>, Vec<Member>) = current.iter().copied().partition(|m| moving.contains(&m.0));
        if moved.is_empty() {
            return Ok(());
        }
        let at = index.min(rest.len());
        rest.splice(at..at, moved);
        if rest != current {
            write_members(tx, id, &rest)?;
        }
        Ok(())
    })
}

/// Turns a smart playlist into a playlist holding what it lists now.
pub fn freeze(c: &Connection, id: i64) -> R<usize> {
    let ids = {
        let node = get(c, id)?;
        if node.kind != Kind::Smart {
            return invalid("only a smart playlist can be converted");
        }
        track_ids(c, id)?
    };
    c.transaction(|tx| {
        tx.execute("UPDATE playlists SET kind = 'playlist', rules = NULL, updated_at = ? WHERE id = ?", params![now_ms(), id])?;
        let now = now_ms();
        write_members(tx, id, &ids.iter().map(|&t| (t, now)).collect::<Vec<_>>())?;
        Ok(ids.len())
    })
}

/// Playlists (not smart ones) that hold `track`.
pub fn containing(c: &Connection, track: i64) -> R<Vec<i64>> {
    Ok(c.query("SELECT playlist_id FROM playlist_tracks WHERE track_id = ? ORDER BY playlist_id", params![track], |r| r.i64(0))?)
}

// ----------------------------------------------------------------- tree

/// Every node with its track count and total duration. Folders count the
/// tracks of everything under them (a track in two playlists counts twice).
pub fn list(c: &Connection) -> R<Value> {
    let nodes = c.query(&format!("SELECT {NODE_COLUMNS} FROM playlists ORDER BY parent_id IS NOT NULL, parent_id, position, id"), params![], Node::from_row)?;
    let mut stats: HashMap<i64, (i64, f64)> = c
        .query(
            "SELECT pt.playlist_id, COUNT(*), COALESCE(SUM(t.duration), 0) FROM playlist_tracks pt JOIN tracks t ON t.id = pt.track_id GROUP BY pt.playlist_id",
            params![],
            |r| (r.i64(0), (r.i64(1), r.f64(2))),
        )?
        .into_iter()
        .collect();
    let mut problems: HashMap<i64, String> = HashMap::new();
    for n in nodes.iter().filter(|n| n.kind == Kind::Smart) {
        match rules_of(n) {
            Ok(rules) => {
                let (w, p, order) = smart_sql(&rules);
                let row = c.query_row(
                    &format!("SELECT COUNT(*), COALESCE(SUM(duration), 0) FROM (SELECT duration FROM tracks WHERE {w} {order})"),
                    &as_params(&p),
                    |r| (r.i64(0), r.f64(1)),
                )?;
                stats.insert(n.id, row.unwrap_or((0, 0.0)));
            }
            Err(e) => {
                problems.insert(n.id, e.to_string());
            }
        }
    }
    // Folders: sum what is under them, deepest first.
    let parent_of: HashMap<i64, Option<i64>> = nodes.iter().map(|n| (n.id, n.parent_id)).collect();
    let depth = |mut id: i64| {
        let mut d = 0;
        while let Some(Some(p)) = parent_of.get(&id) {
            d += 1;
            id = *p;
            if d > 64 {
                break;
            }
        }
        d
    };
    let mut by_depth: Vec<&Node> = nodes.iter().collect();
    by_depth.sort_by_key(|n| std::cmp::Reverse(depth(n.id)));
    let mut totals: HashMap<i64, (i64, f64)> = HashMap::new();
    let mut kids: HashMap<i64, i64> = HashMap::new();
    for n in by_depth {
        let own = if n.kind == Kind::Folder { totals.get(&n.id).copied().unwrap_or((0, 0.0)) } else { stats.get(&n.id).copied().unwrap_or((0, 0.0)) };
        totals.insert(n.id, own);
        if let Some(p) = n.parent_id {
            let e = totals.entry(p).or_insert((0, 0.0));
            e.0 += own.0;
            e.1 += own.1;
            *kids.entry(p).or_insert(0) += 1;
        }
    }
    let out: Vec<Value> = nodes
        .iter()
        .map(|n| {
            let (count, secs) = totals.get(&n.id).copied().unwrap_or((0, 0.0));
            obj! {
                "id" => n.id,
                "parentId" => n.parent_id,
                "kind" => n.kind.id(),
                "name" => n.name.clone(),
                "position" => n.position,
                "rules" => n.rules.clone(),
                "count" => count,
                "duration" => num(secs, 0),
                "children" => kids.get(&n.id).copied().unwrap_or(0),
                "problem" => problems.get(&n.id).cloned(),
                "createdAt" => n.created_at,
                "updatedAt" => n.updated_at,
            }
        })
        .collect();
    Ok(Value::Array(out))
}

pub fn node_json(c: &Connection, id: i64) -> R<Value> {
    let all = list(c)?;
    all.as_array()
        .and_then(|a| a.iter().find(|v| v.get("id").and_then(Value::as_i64) == Some(id)).cloned())
        .ok_or_else(|| PlaylistError::NotFound(format!("playlist {id} not found")))
}

// ------------------------------------------------------------- organize

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrganizeBy {
    Genre,
    Key,
    Tempo,
}

impl OrganizeBy {
    pub fn from_id(s: &str) -> Option<OrganizeBy> {
        match s {
            "genre" => Some(OrganizeBy::Genre),
            "key" => Some(OrganizeBy::Key),
            "tempo" => Some(OrganizeBy::Tempo),
            _ => None,
        }
    }

    fn folder_name(self) -> &'static str {
        match self {
            OrganizeBy::Genre => "By genre",
            OrganizeBy::Key => "By key",
            OrganizeBy::Tempo => "By tempo",
        }
    }
}

/// Width of the tempo bands "By tempo" creates.
pub const TEMPO_BAND: f64 = 5.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Organized {
    pub folder_id: i64,
    pub playlists: usize,
    pub tracks: usize,
    /// Tracks without a genre, key or tempo to sort them by.
    pub without: usize,
    /// Groups left out past the most one folder holds.
    pub left_out: usize,
}

impl ToJson for Organized {
    fn to_json(&self) -> Value {
        obj! { "folderId" => self.folder_id, "playlists" => self.playlists, "tracks" => self.tracks, "without" => self.without, "leftOut" => self.left_out }
    }
}

/// The name of a key playlist: "8A (A minor)" in Camelot or Open Key, "Am" in note names.
fn key_name(key: u8, notation: Notation) -> String {
    match notation {
        Notation::Camelot | Notation::OpenKey => format!("{} ({})", format_key(key, notation, false), long_name(key)),
        _ => format_key(key, notation, false),
    }
}

fn band_of(bpm: f64) -> i64 {
    ((bpm + 1e-9) / TEMPO_BAND).floor() as i64 * TEMPO_BAND as i64
}

/// Sorts tracks into a new folder of playlists, one per genre, key or tempo
/// band. With `ids`, only those tracks, as fixed playlists; without, the
/// whole library, as smart playlists (`smart`) that stay current, or fixed
/// ones.
pub fn organize(c: &Connection, by: OrganizeBy, ids: Option<&[i64]>, smart: bool, notation: Notation) -> R<Organized> {
    let smart = smart && ids.is_none();
    // (id, genre, key, tempo)
    type Facts = (i64, Option<String>, Option<i64>, Option<f64>);
    let rows: Vec<Facts> = match ids {
        Some(ids) => {
            let mut v = Vec::new();
            for chunk in ids.chunks(500) {
                let marks = vec!["?"; chunk.len()].join(",");
                let p: Vec<SqlValue> = chunk.iter().map(|&i| SqlValue::Int(i)).collect();
                v.extend(c.query(
                    &format!("SELECT id, dj_genre, dj_key, dj_bpm FROM tracks WHERE id IN ({marks}) AND missing_since IS NULL ORDER BY dj_bpm IS NULL, dj_bpm, dj_key_order, sort_title"),
                    &as_params(&p),
                    |r| (r.i64(0), r.opt_text(1), r.opt_i64(2), r.opt_f64(3)),
                )?);
            }
            v
        }
        None => c.query(
            "SELECT id, dj_genre, dj_key, dj_bpm FROM tracks WHERE missing_since IS NULL ORDER BY dj_bpm IS NULL, dj_bpm, dj_key_order, sort_title",
            params![],
            |r| (r.i64(0), r.opt_text(1), r.opt_i64(2), r.opt_f64(3)),
        )?,
    };
    // Group key → (sort key, name, rules filter, members).
    let mut groups: HashMap<String, (String, String, Value, Vec<i64>)> = HashMap::new();
    let mut without = 0usize;
    for (id, genre, key, bpm) in rows {
        let g = match by {
            OrganizeBy::Genre => genre.map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).map(|g| {
                let k = g.to_lowercase();
                (k.clone(), k, g.clone(), obj! { "genres" => vec![g] })
            }),
            OrganizeBy::Key => key.filter(|k| (0..24).contains(k)).map(|k| {
                let k = k as u8;
                let (n, l) = camelot(k);
                (format!("k{k}"), format!("{n:02}{l}"), key_name(k, notation), obj! { "keys" => vec![k as i64] })
            }),
            OrganizeBy::Tempo => bpm.filter(|b| *b > 0.0).map(|b| {
                let lo = band_of(b);
                let hi = lo + TEMPO_BAND as i64 - 1;
                (format!("b{lo}"), format!("{lo:05}"), format!("{lo}–{hi} BPM"), obj! { "bpmMin" => lo, "bpmMax" => num(lo as f64 + TEMPO_BAND - 0.001, 3) })
            }),
        };
        match g {
            Some((k, sort, name, filter)) => {
                // The first spelling of a genre seen names its playlist.
                groups.entry(k).or_insert_with(|| (sort, name, filter, Vec::new())).3.push(id);
            }
            None => without += 1,
        }
    }
    let mut list: Vec<(String, String, Value, Vec<i64>)> = groups.into_values().collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    let left_out = list.len().saturating_sub(MAX_GROUPS);
    list.truncate(MAX_GROUPS);
    if list.is_empty() {
        return invalid(match by {
            OrganizeBy::Genre => "none of these tracks has a genre yet",
            OrganizeBy::Key => "none of these tracks has a key yet: analyze them first",
            OrganizeBy::Tempo => "none of these tracks has a tempo yet: analyze them first",
        });
    }
    c.transaction(|tx| {
        let taken: HashSet<String> = tx.query("SELECT name FROM playlists WHERE parent_id IS NULL", params![], |r| r.text(0).to_lowercase())?.into_iter().collect();
        let base = by.folder_name();
        let name = (1..).map(|n| if n == 1 { base.to_string() } else { format!("{base} {n}") }).find(|n| !taken.contains(&n.to_lowercase())).unwrap_or(base.to_string());
        let folder = insert_node(tx, Kind::Folder, &name, None, None, None)?;
        let mut tracks = 0;
        let sort = match by {
            OrganizeBy::Tempo => "key",
            _ => "bpm",
        };
        for (_, title, filter, members) in &list {
            if smart {
                let rules = obj! { "filter" => filter.clone(), "sort" => obj! { "key" => sort, "desc" => false } };
                insert_node(tx, Kind::Smart, title, Some(folder), None, Some(&rules))?;
            } else {
                let p = insert_node(tx, Kind::Playlist, title, Some(folder), None, None)?;
                let now = now_ms();
                write_members(tx, p, &members.iter().map(|&t| (t, now)).collect::<Vec<_>>())?;
            }
            tracks += members.len();
        }
        Ok(Organized { folder_id: folder, playlists: list.len(), tracks, without, left_out })
    })
}

// --------------------------------------------------------------- mixing

/// How the keys of two neighbouring tracks mix (Camelot wheel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMatch {
    Same,
    /// Same number, other letter: the relative major or minor.
    Relative,
    /// One step round the wheel, same letter.
    Adjacent,
    /// One step round and the other letter.
    Diagonal,
    /// Two steps round, same letter (an energy boost).
    Boost,
    Clash,
    Unknown,
}

impl KeyMatch {
    pub fn id(self) -> &'static str {
        match self {
            KeyMatch::Same => "same",
            KeyMatch::Relative => "relative",
            KeyMatch::Adjacent => "adjacent",
            KeyMatch::Diagonal => "diagonal",
            KeyMatch::Boost => "boost",
            KeyMatch::Clash => "clash",
            KeyMatch::Unknown => "unknown",
        }
    }

    /// Mixes without a key clash.
    pub fn harmonic(self) -> bool {
        matches!(self, KeyMatch::Same | KeyMatch::Relative | KeyMatch::Adjacent)
    }

    fn cost(self) -> f64 {
        match self {
            KeyMatch::Same => 0.0,
            KeyMatch::Relative | KeyMatch::Adjacent => 1.0,
            KeyMatch::Diagonal => 2.2,
            KeyMatch::Boost => 2.6,
            KeyMatch::Unknown => 2.5,
            KeyMatch::Clash => 6.0,
        }
    }
}

pub fn key_match(a: Option<i64>, b: Option<i64>) -> KeyMatch {
    let (Some(a), Some(b)) = (a.filter(|k| (0..24).contains(k)), b.filter(|k| (0..24).contains(k))) else { return KeyMatch::Unknown };
    let (a, b) = (a as u8, b as u8);
    let (na, la) = camelot(a);
    let (nb, lb) = camelot(b);
    let d = (na as i32 - nb as i32).rem_euclid(12);
    let around = d.min(12 - d);
    match (around, la == lb) {
        (0, true) => KeyMatch::Same,
        (0, false) => KeyMatch::Relative,
        (1, true) => KeyMatch::Adjacent,
        (1, false) => KeyMatch::Diagonal,
        (2, true) => KeyMatch::Boost,
        _ => {
            debug_assert!(camelot_distance(a, b) >= 2);
            KeyMatch::Clash
        }
    }
}

/// Tempo change from `a` to `b` in percent, mixing at half or double time
/// when that is closer (then `true`). `None` when a tempo is unknown.
pub fn tempo_change(a: Option<f64>, b: Option<f64>) -> Option<(f64, bool)> {
    let (a, b) = (a.filter(|x| *x > 0.0)?, b.filter(|x| *x > 0.0)?);
    let direct = (b / a).ln();
    let best = [(direct, false), (direct + std::f64::consts::LN_2, true), (direct - std::f64::consts::LN_2, true)]
        .into_iter()
        .min_by(|x, y| x.0.abs().total_cmp(&y.0.abs()))
        .unwrap_or((direct, false));
    Some(((best.0.exp() - 1.0) * 100.0, best.1))
}

fn tempo_cost(change: Option<(f64, bool)>) -> f64 {
    match change {
        None => 3.0,
        Some((pct, half_double)) => {
            let p = pct.abs();
            let base = if p <= 1.0 {
                p * 0.3
            } else if p <= 6.0 {
                0.3 + (p - 1.0) * 0.8
            } else {
                4.3 + (p - 6.0) * 1.5
            };
            base + if half_double { 1.5 } else { 0.0 }
        }
    }
}

/// What arranging optimises: the key and the tempo of each transition.
fn transition_cost(a: &TrackRow, b: &TrackRow, rising: bool) -> f64 {
    let change = tempo_change(a.dj_bpm, b.dj_bpm);
    let mut cost = key_match(a.dj_key, b.dj_key).cost() + tempo_cost(change);
    if rising {
        if let (Some(x), Some(y)) = (a.dj_bpm, b.dj_bpm) {
            // Building up: slowing down costs, by how much.
            if y < x * 0.995 {
                cost += 2.0 + (x - y) / x * 100.0 * 0.5;
            }
        }
    }
    cost
}

/// Quality of a running order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MixSummary {
    pub transitions: usize,
    pub harmonic: usize,
    pub clashes: usize,
    pub unknown: usize,
    /// Mean absolute tempo change between neighbours with known tempos, in percent.
    pub tempo_change: Option<f64>,
}

impl ToJson for MixSummary {
    fn to_json(&self) -> Value {
        obj! {
            "transitions" => self.transitions,
            "harmonic" => self.harmonic,
            "clashes" => self.clashes,
            "unknown" => self.unknown,
            "tempoChange" => self.tempo_change.map(|v| num(v, 2)),
        }
    }
}

pub fn summarize(rows: &[&TrackRow]) -> MixSummary {
    let mut s = MixSummary { transitions: rows.len().saturating_sub(1), ..Default::default() };
    let mut sum = 0.0;
    let mut n = 0;
    for w in rows.windows(2) {
        match key_match(w[0].dj_key, w[1].dj_key) {
            KeyMatch::Unknown => s.unknown += 1,
            KeyMatch::Clash => s.clashes += 1,
            m if m.harmonic() => s.harmonic += 1,
            _ => {}
        }
        if let Some((pct, _)) = tempo_change(w[0].dj_bpm, w[1].dj_bpm) {
            sum += pct.abs();
            n += 1;
        }
    }
    s.tempo_change = (n > 0).then(|| sum / n as f64);
    s
}

/// A running order for mixing: neighbours close in tempo and in keys that
/// mix. Starts with `start` when given; with `rising`, tempos build up.
/// Returns the track ids in the new order.
pub fn arrange(rows: &[TrackRow], start: Option<i64>, rising: bool) -> Vec<i64> {
    let n = rows.len();
    if n < 3 {
        let mut ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        if let Some(s) = start {
            if let Some(i) = ids.iter().position(|&x| x == s) {
                let v = ids.remove(i);
                ids.insert(0, v);
            }
        }
        if rising && start.is_none() && n == 2 && rows[1].dj_bpm.unwrap_or(0.0) < rows[0].dj_bpm.unwrap_or(0.0) {
            ids.reverse();
        }
        return ids;
    }
    let cost: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| if i == j { 0.0 } else { transition_cost(&rows[i], &rows[j], rising) }).collect()).collect();
    let path_cost = |p: &[usize]| p.windows(2).map(|w| cost[w[0]][w[1]]).sum::<f64>();
    let greedy = |first: usize| {
        let mut used = vec![false; n];
        let mut p = Vec::with_capacity(n);
        let mut cur = first;
        used[cur] = true;
        p.push(cur);
        for _ in 1..n {
            let next = (0..n).filter(|&j| !used[j]).min_by(|&x, &y| cost[cur][x].total_cmp(&cost[cur][y]).then(x.cmp(&y))).unwrap_or(cur);
            used[next] = true;
            p.push(next);
            cur = next;
        }
        p
    };
    let fixed = start.and_then(|s| rows.iter().position(|r| r.id == s));
    let starts: Vec<usize> = match fixed {
        Some(i) => vec![i],
        // Small sets: try every opener. Larger ones: the slowest (rising) or the first.
        None if n <= 150 => (0..n).collect(),
        None if rising => vec![(0..n).min_by(|&a, &b| rows[a].dj_bpm.unwrap_or(f64::MAX).total_cmp(&rows[b].dj_bpm.unwrap_or(f64::MAX))).unwrap_or(0)],
        None => vec![0],
    };
    let mut best = starts.iter().map(|&s| greedy(s)).min_by(|a, b| path_cost(a).total_cmp(&path_cost(b))).unwrap_or_else(|| (0..n).collect());
    let pinned = fixed.is_some() || n > 150;
    // Improve: move one track elsewhere (or-opt), which keeps every other
    // transition's direction, so it suits the direction-dependent costs of a
    // build-up too; then reverse stretches (2-opt) when costs are symmetric.
    let budget = std::time::Instant::now();
    let edge = |a: Option<usize>, b: Option<usize>| match (a, b) {
        (Some(a), Some(b)) => cost[a][b],
        _ => 0.0,
    };
    let mut improved = true;
    let mut rounds = 0;
    while improved && rounds < 50 && budget.elapsed() < std::time::Duration::from_millis(1500) {
        improved = false;
        rounds += 1;
        let first_movable = usize::from(pinned);
        for i in first_movable..n {
            let v = best[i];
            let prev = i.checked_sub(1).map(|k| best[k]);
            let next = best.get(i + 1).copied();
            let gain = edge(prev, Some(v)) + edge(Some(v), next) - edge(prev, next);
            // Without `v`, slot k sits between q[k-1] and q[k].
            let q = |k: usize| if k < i { best[k] } else { best[k + 1] };
            let mut choice: Option<(usize, f64)> = None;
            for k in first_movable..n {
                if k == i {
                    continue;
                }
                let left = k.checked_sub(1).map(q);
                let right = (k < n - 1).then(|| q(k));
                let delta = edge(left, Some(v)) + edge(Some(v), right) - edge(left, right) - gain;
                if delta < -1e-9 && choice.is_none_or(|c| delta < c.1) {
                    choice = Some((k, delta));
                }
            }
            if let Some((k, _)) = choice {
                best.remove(i);
                best.insert(k, v);
                improved = true;
            }
        }
        if !rising {
            for i in first_movable.max(1)..n.saturating_sub(1) {
                for j in i + 1..n {
                    let before = cost[best[i - 1]][best[i]] + if j + 1 < n { cost[best[j]][best[j + 1]] } else { 0.0 };
                    let after = cost[best[i - 1]][best[j]] + if j + 1 < n { cost[best[i]][best[j + 1]] } else { 0.0 };
                    if after + 1e-9 < before {
                        best[i..=j].reverse();
                        improved = true;
                    }
                }
            }
            // The whole order backwards may suit a free start better.
            if !pinned {
                let mut rev = best.clone();
                rev.reverse();
                if path_cost(&rev) + 1e-9 < path_cost(&best) {
                    best = rev;
                    improved = true;
                }
            }
        }
    }
    best.into_iter().map(|i| rows[i].id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_mix_by_the_wheel() {
        // 8A = A minor (21), 8B = C major (0), 9A = E minor (16), 7A = D minor (14), 10A = B minor (23).
        assert_eq!(key_match(Some(21), Some(21)), KeyMatch::Same);
        assert_eq!(key_match(Some(21), Some(0)), KeyMatch::Relative);
        assert_eq!(key_match(Some(21), Some(16)), KeyMatch::Adjacent);
        assert_eq!(key_match(Some(21), Some(14)), KeyMatch::Adjacent);
        assert_eq!(key_match(Some(21), Some(7)), KeyMatch::Diagonal, "8A → 9B (G major)");
        assert_eq!(key_match(Some(21), Some(23)), KeyMatch::Boost, "8A → 10A");
        assert_eq!(key_match(Some(21), Some(3)), KeyMatch::Clash, "8A → 5B (E♭ major)");
        assert_eq!(key_match(None, Some(3)), KeyMatch::Unknown);
        assert!(KeyMatch::Adjacent.harmonic() && !KeyMatch::Diagonal.harmonic());
    }

    #[test]
    fn tempo_changes_consider_half_and_double_time() {
        let (pct, hd) = tempo_change(Some(124.0), Some(126.0)).unwrap();
        assert!((pct - 1.6129).abs() < 0.01 && !hd);
        let (pct, hd) = tempo_change(Some(87.0), Some(174.0)).unwrap();
        assert!(pct.abs() < 1e-9 && hd, "174 is double 87");
        assert!(tempo_change(None, Some(120.0)).is_none());
    }

    fn row(id: i64, bpm: f64, key: i64) -> TrackRow {
        TrackRow { id, dj_bpm: Some(bpm), dj_key: Some(key), ..Default::default() }
    }

    #[test]
    fn arranging_follows_the_wheel_and_the_tempo() {
        // A minor family around 124–128 BPM, shuffled; one clashing outlier.
        let rows = vec![row(1, 128.0, 23), row(2, 124.0, 21), row(3, 126.0, 16), row(4, 125.0, 0), row(5, 100.0, 3), row(6, 127.0, 4)];
        let order = arrange(&rows, None, false);
        assert_eq!(order.len(), 6);
        let by_id: HashMap<i64, &TrackRow> = rows.iter().map(|r| (r.id, r)).collect();
        let arranged: Vec<&TrackRow> = order.iter().map(|id| by_id[id]).collect();
        let shuffled: Vec<&TrackRow> = rows.iter().collect();
        let (a, s) = (summarize(&arranged), summarize(&shuffled));
        assert!(a.harmonic >= s.harmonic, "{a:?} vs {s:?}");
        assert!(a.tempo_change.unwrap() <= s.tempo_change.unwrap());
        // The outlier ends up at an edge, not between two good transitions.
        let at = order.iter().position(|&x| x == 5).unwrap();
        assert!(at == 0 || at == 5, "{order:?}");
        // A fixed opener stays first.
        assert_eq!(arrange(&rows, Some(6), false)[0], 6);
    }

    #[test]
    fn building_up_rises_in_tempo() {
        let rows = vec![row(1, 128.0, 21), row(2, 120.0, 21), row(3, 124.0, 21), row(4, 122.0, 21), row(5, 126.0, 21)];
        let order = arrange(&rows, None, true);
        assert_eq!(order, vec![2, 4, 3, 5, 1]);
    }
}
