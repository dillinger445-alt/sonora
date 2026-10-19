//! Sending playlists to a USB drive (or any folder).
//!
//! Each chosen playlist becomes a folder of DJ-ready files under `SONORA/`
//! on the drive, mirroring the playlist tree, numbered in playlist order
//! ("001 - Artist - Title.aiff") so every player lists them as the set
//! runs. Beside them: an M3U8 playlist per folder, a rekordbox.xml with the
//! whole tree, and Serato crates in the drive's `_Serato_` folder.
//!
//! Sending again brings the drive up to date: unchanged files stay,
//! reordered ones are renamed, changed ones are written again. SONORA keeps
//! a record of what it wrote (`SONORA/.sonora-sync.json`) and only ever
//! renames, replaces or removes those files, and only while they are
//! exactly as it left them. Anything else on the drive is never touched,
//! and files that left the playlists are only removed when asked.

use crate::db::sqlite::Connection;
use crate::djexport::{self, base_name, clean_name, export_file, file_url, unique_path, xml_escape, ExportError, ExportOptions, PlanItem};
use crate::events::EventBus;
use crate::library::get_tracks;
use crate::model::TrackRow;
use crate::playlists::{self, Kind, Node, PlaylistError};
use sonora_engine::analysis::key::format_key;
use sonora_engine::decode::ToolPaths;
use sonora_engine::export::convert::{Quality, Target};
use sonora_json::{num, obj, ToJson, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The folder SONORA writes into, at the top of the drive.
pub const SYNC_DIR: &str = "SONORA";
/// SONORA's record of the files it wrote, inside [`SYNC_DIR`].
pub const MANIFEST: &str = ".sonora-sync.json";
const SERATO_DIR: &str = "_Serato_/Subcrates";
/// Room kept free on the drive beyond what the files need.
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;
/// FAT32 cannot hold files of 4 GiB or more.
const FAT32_LIMIT: u64 = 4 * 1024 * 1024 * 1024 - 1;
/// Longest folder and file names (characters), so paths stay short enough
/// for every system and player.
const MAX_FOLDER_NAME: usize = 60;
const MAX_FILE_BASE: usize = 100;

// ---------------------------------------------------------------- drives

#[derive(Clone, Debug, PartialEq)]
pub struct Drive {
    pub path: PathBuf,
    pub name: String,
    /// File system as the system names it ("msdos", "exfat", "FAT32", "NTFS", …).
    pub fs: Option<String>,
    pub total: Option<u64>,
    pub free: Option<u64>,
    /// A removable drive (USB stick, SD card); otherwise another external
    /// or secondary drive.
    pub removable: bool,
}

impl ToJson for Drive {
    fn to_json(&self) -> Value {
        obj! {
            "path" => self.path.to_string_lossy().to_string(),
            "name" => self.name.clone(),
            "fs" => self.fs.clone(),
            "fsLabel" => self.fs.as_deref().map(fs_label),
            "total" => self.total,
            "free" => self.free,
            "removable" => self.removable,
        }
    }
}

/// A file system name people recognise.
pub fn fs_label(fs: &str) -> String {
    match fs.to_ascii_lowercase().as_str() {
        "msdos" | "vfat" | "fat32" | "fat" => "FAT32".into(),
        "exfat" => "exFAT".into(),
        "ntfs" | "ntfs3" | "fuseblk" => "NTFS".into(),
        "apfs" => "APFS".into(),
        "hfs" => "Mac OS Extended".into(),
        "ext4" | "ext3" | "ext2" => fs.to_ascii_lowercase(),
        _ => fs.to_string(),
    }
}

fn is_fat32(fs: Option<&str>) -> bool {
    fs.is_some_and(|f| fs_label(f) == "FAT32")
}

/// External and removable drives that can be written to.
pub fn drives() -> Vec<Drive> {
    let mut d = list_drives();
    d.sort_by(|a, b| b.removable.cmp(&a.removable).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    d
}

/// Whether `path` is the top of a drive (where Serato looks for crates).
pub fn is_drive_root(path: &Path) -> bool {
    if cfg!(windows) {
        return path.parent().is_none() && path.has_root();
    }
    list_drives().iter().any(|d| d.path == path)
}

/// Total and free bytes of the file system holding `path`.
pub fn space(path: &Path) -> Option<(u64, u64)> {
    space_of(path)
}

#[cfg(unix)]
fn space_of(path: &Path) -> Option<(u64, u64)> {
    // `df -kP` is POSIX: 1024-byte blocks, one line per file system.
    let out = std::process::Command::new("df").arg("-kP").arg(path).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().nth(1)?;
    let cols: Vec<&str> = line.split_whitespace().collect();
    // The device name may contain spaces; the numbers follow it.
    let nums: Vec<u64> = cols.iter().filter_map(|c| c.parse::<u64>().ok()).collect();
    if nums.len() < 3 {
        return None;
    }
    Some((nums[0] * 1024, nums[2] * 1024))
}

#[cfg(target_os = "macos")]
fn list_drives() -> Vec<Drive> {
    let Ok(out) = std::process::Command::new("/sbin/mount").stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output() else { return Vec::new() };
    parse_mount(&String::from_utf8_lossy(&out.stdout))
        .into_iter()
        .map(|(path, fs)| {
            let (total, free) = space_of(&path).map_or((None, None), |(t, f)| (Some(t), Some(f)));
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            Drive { path, name, fs: Some(fs), total, free, removable: true }
        })
        .collect()
}

/// Writable local volumes under /Volumes from `mount` output:
/// "/dev/disk4s1 on /Volumes/USB STICK (msdos, local, nodev, nosuid, noowners)".
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_mount(text: &str) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((device, rest)) = line.split_once(" on ") else { continue };
        let Some(open) = rest.rfind(" (") else { continue };
        let (path, opts) = (&rest[..open], rest[open + 2..].trim_end_matches(')'));
        let opts: Vec<&str> = opts.split(',').map(str::trim).collect();
        if !device.starts_with("/dev/") || !path.starts_with("/Volumes/") || !opts.contains(&"local") || opts.contains(&"read-only") {
            continue;
        }
        out.push((PathBuf::from(path), opts.first().copied().unwrap_or("").to_string()));
    }
    out
}

#[cfg(target_os = "linux")]
fn list_drives() -> Vec<Drive> {
    let Ok(text) = std::fs::read_to_string("/proc/mounts") else { return Vec::new() };
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split(' ').collect();
        if cols.len() < 4 {
            continue;
        }
        // Spaces and other characters are written as octal escapes ("\040").
        let path = unescape_mount(cols[1]);
        let removable_place = ["/media/", "/run/media/", "/mnt/"].iter().any(|p| path.starts_with(p));
        if !removable_place || cols[3].split(',').any(|o| o == "ro") {
            continue;
        }
        let path = PathBuf::from(path);
        let (total, free) = space_of(&path).map_or((None, None), |(t, f)| (Some(t), Some(f)));
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        out.push(Drive { path, name, fs: Some(cols[2].to_string()), total, free, removable: true });
    }
    out
}

#[cfg(target_os = "linux")]
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 4 <= b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(windows)]
mod win {
    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetLogicalDrives() -> u32;
        pub fn GetDriveTypeW(root: *const u16) -> u32;
        pub fn GetDiskFreeSpaceExW(dir: *const u16, free_to_caller: *mut u64, total: *mut u64, total_free: *mut u64) -> i32;
        pub fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_component: *mut u32,
            flags: *mut u32,
            fs: *mut u16,
            fs_len: u32,
        ) -> i32;
        pub fn SetErrorMode(mode: u32) -> u32;
    }
    pub const DRIVE_REMOVABLE: u32 = 2;
    pub const DRIVE_FIXED: u32 = 3;
    pub const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;
    pub const SEM_FAILCRITICALERRORS: u32 = 0x0001;
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn space_of(path: &Path) -> Option<(u64, u64)> {
    let p = wide(&path.to_string_lossy());
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: valid, NUL-terminated path and out-pointers to locals.
    let ok = unsafe { win::GetDiskFreeSpaceExW(p.as_ptr(), &mut avail, &mut total, &mut free) };
    (ok != 0).then_some((total, avail))
}

#[cfg(windows)]
fn list_drives() -> Vec<Drive> {
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_ascii_uppercase();
    // No "insert a disk" dialogs for empty card readers.
    // SAFETY: plain Win32 calls with valid arguments.
    let previous = unsafe { win::SetErrorMode(win::SEM_FAILCRITICALERRORS) };
    let mask = unsafe { win::GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = char::from(b'A' + i as u8);
        let root = format!("{letter}:\\");
        if format!("{letter}:") == system {
            continue;
        }
        let w = wide(&root);
        let kind = unsafe { win::GetDriveTypeW(w.as_ptr()) };
        if kind != win::DRIVE_REMOVABLE && kind != win::DRIVE_FIXED {
            continue;
        }
        let mut name = [0u16; 261];
        let mut fs = [0u16; 261];
        let (mut serial, mut max_len, mut flags) = (0u32, 0u32, 0u32);
        // SAFETY: buffers and lengths match; the root is NUL-terminated.
        let ok = unsafe { win::GetVolumeInformationW(w.as_ptr(), name.as_mut_ptr(), 261, &mut serial, &mut max_len, &mut flags, fs.as_mut_ptr(), 261) };
        if ok == 0 || flags & win::FILE_READ_ONLY_VOLUME != 0 {
            continue;
        }
        let text = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
        let label = text(&name);
        let path = PathBuf::from(&root);
        let (total, free) = space_of(&path).map_or((None, None), |(t, f)| (Some(t), Some(f)));
        out.push(Drive {
            name: if label.is_empty() { format!("Drive {letter}:") } else { format!("{label} ({letter}:)") },
            path,
            fs: Some(text(&fs)).filter(|s| !s.is_empty()),
            total,
            free,
            removable: kind == win::DRIVE_REMOVABLE,
        });
    }
    unsafe { win::SetErrorMode(previous) };
    out
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn list_drives() -> Vec<Drive> {
    Vec::new()
}

#[cfg(not(any(unix, windows)))]
fn space_of(_path: &Path) -> Option<(u64, u64)> {
    None
}

// --------------------------------------------------------------- options

#[derive(Clone, Debug, PartialEq)]
pub struct SyncOptions {
    /// Format, quality, key notation and best copy, as for a DJ export;
    /// `rekordbox_xml` writes `SONORA/rekordbox.xml`.
    pub export: ExportOptions,
    pub m3u: bool,
    pub serato: bool,
    /// Remove files SONORA put there earlier that are no longer in these
    /// playlists (only those unchanged since).
    pub remove_stale: bool,
    /// "01 Name" folders, in the order of the playlist tree.
    pub number_folders: bool,
}

impl SyncOptions {
    pub fn from_json(p: &Value) -> Result<SyncOptions, String> {
        Ok(SyncOptions {
            export: ExportOptions::from_json(p)?,
            m3u: p.get("m3u").and_then(Value::as_bool).unwrap_or(true),
            serato: p.get("serato").and_then(Value::as_bool).unwrap_or(true),
            remove_stale: p.get("removeStale").and_then(Value::as_bool).unwrap_or(false),
            number_folders: p.get("numberFolders").and_then(Value::as_bool).unwrap_or(true),
        })
    }
}

// -------------------------------------------------------------- manifest

/// A file SONORA wrote, as it left it. Paths are relative to the drive (or
/// chosen folder), with `/` between names.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: String,
    /// The playlist and track an audio file belongs to (0 for playlist files).
    pub playlist: i64,
    pub track: i64,
    /// What the file was made from (see [`content_key`]).
    pub key: String,
    pub bytes: u64,
    pub mtime: i64,
    pub hash: String,
}

impl Entry {
    fn to_json(&self) -> Value {
        obj! {
            "path" => self.path.clone(), "playlist" => self.playlist, "track" => self.track, "key" => self.key.clone(),
            "bytes" => self.bytes, "mtime" => self.mtime, "hash" => self.hash.clone(),
        }
    }

    fn from_json(v: &Value) -> Option<Entry> {
        let path = v.get("path")?.as_str()?.to_string();
        // Only plain relative paths in the folders SONORA writes to. The
        // record is on the drive, so it is not trusted: no "..", no drive
        // letters or streams (":"), no other separators.
        let parts: Vec<&str> = path.split('/').collect();
        let inside = parts.len() >= 2 && (parts[0] == SYNC_DIR || parts[0] == "_Serato_");
        if !inside || parts.iter().any(|c| c.is_empty() || *c == "." || *c == ".." || c.contains([':', '\\']) || c.chars().any(char::is_control)) {
            return None;
        }
        Some(Entry {
            path,
            playlist: v.get("playlist").and_then(Value::as_i64).unwrap_or(0),
            track: v.get("track").and_then(Value::as_i64).unwrap_or(0),
            key: v.get("key").and_then(Value::as_str).unwrap_or("").to_string(),
            bytes: v.get("bytes").and_then(Value::as_u64)?,
            mtime: v.get("mtime").and_then(Value::as_i64).unwrap_or(0),
            hash: v.get("hash").and_then(Value::as_str).unwrap_or("").to_string(),
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Manifest {
    pub entries: Vec<Entry>,
}

fn manifest_path(root: &Path) -> PathBuf {
    root.join(SYNC_DIR).join(MANIFEST)
}

pub fn read_manifest(root: &Path) -> Manifest {
    let Ok(text) = std::fs::read_to_string(manifest_path(root)) else { return Manifest::default() };
    let Ok(v) = sonora_json::parse(&text) else { return Manifest::default() };
    Manifest { entries: v.get("files").and_then(Value::as_array).map(|a| a.iter().filter_map(Entry::from_json).collect()).unwrap_or_default() }
}

fn write_manifest(root: &Path, m: &Manifest, app_version: &str) -> std::io::Result<()> {
    let p = manifest_path(root);
    std::fs::create_dir_all(p.parent().unwrap_or(root))?;
    let v = obj! {
        "about" => "Files SONORA put on this drive. SONORA changes only these, and only while they are as it left them. Delete this file and SONORA treats them as yours.",
        "version" => 1,
        "app" => app_version,
        "files" => Value::Array(m.entries.iter().map(Entry::to_json).collect()),
    };
    let tmp = p.with_extension("json.sonora-part");
    std::fs::write(&tmp, v.to_string_pretty())?;
    std::fs::rename(&tmp, &p)
}

fn rel_to_path(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, c| p.join(c))
}

fn mtime_of(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The file is exactly as SONORA left it: same size, and the same
/// modification time or (after a time-zone change, say) the same content.
fn intact(root: &Path, e: &Entry) -> bool {
    let p = rel_to_path(root, &e.path);
    let Ok(m) = std::fs::metadata(&p) else { return false };
    if !m.is_file() || m.len() != e.bytes {
        return false;
    }
    mtime_of(&m) == e.mtime || (!e.hash.is_empty() && crate::index::quick_hash(&p).is_ok_and(|h| h == e.hash))
}

fn record(root: &Path, rel: &str, playlist: i64, track: i64, key: &str) -> Option<Entry> {
    let p = rel_to_path(root, rel);
    let m = std::fs::metadata(&p).ok()?;
    Some(Entry { path: rel.to_string(), playlist, track, key: key.to_string(), bytes: m.len(), mtime: mtime_of(&m), hash: crate::index::quick_hash(&p).unwrap_or_default() })
}

/// FNV-1a, for compact content keys.
fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// What a file is made from: the source file, the format and the tags. A
/// change to any of them writes the file again.
pub fn content_key(item: &PlanItem, o: &ExportOptions) -> String {
    let t = &item.track;
    let s = &item.source;
    let source = s.file_hash.clone().unwrap_or_else(|| format!("{}-{}-{}", s.path, s.size, s.mtime));
    fnv(&format!(
        "1|{}|{:?}|{}|{}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        o.target.extension(),
        o.quality,
        o.notation.id(),
        s.id,
        source,
        t.title,
        t.artist,
        t.album,
        t.dj_genre,
        t.dj_bpm.map(|b| (b * 100.0).round() as i64),
        t.dj_key
    ))
}

// ------------------------------------------------------------------ plan

/// One playlist as it goes onto the drive.
#[derive(Clone, Debug)]
pub struct Leaf {
    pub id: i64,
    pub name: String,
    pub kind: Kind,
    /// Its folder, relative to the drive: "SONORA/01 Sets/02 Warm-up".
    pub dir: String,
    /// The names of it and its folders on the drive, outermost first.
    pub path_names: Vec<String>,
    pub items: Vec<PlanItem>,
}

/// A folder of the playlist tree (for crates and the XML tree).
#[derive(Clone, Debug)]
pub struct Branch {
    pub id: i64,
    pub name: String,
    pub path_names: Vec<String>,
}

/// A playlist already on the drive that is not sent this time.
#[derive(Clone, Debug)]
pub struct Other {
    pub id: i64,
    pub path_names: Vec<String>,
    /// Its files on the drive (in their numbered order) and their tracks.
    pub files: Vec<(String, TrackRow)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Keep,
    /// Same file, new place (the order or a name changed).
    Rename(String),
    /// Written again over SONORA's own older version.
    Update(String),
    New,
}

#[derive(Clone, Debug)]
pub struct FilePlan {
    pub leaf: usize,
    pub item: usize,
    pub path: String,
    pub action: Action,
    pub key: String,
    pub estimate: u64,
}

#[derive(Clone, Debug)]
pub struct SyncPlan {
    pub root: PathBuf,
    pub leaves: Vec<Leaf>,
    pub branches: Vec<Branch>,
    /// Playlists sent earlier and not this time: their files stay, and the
    /// rekordbox.xml still lists them.
    pub others: Vec<Other>,
    /// Every playlist and folder that exists now.
    pub existing: HashSet<i64>,
    pub files: Vec<FilePlan>,
    /// SONORA's files no longer in these playlists (or whose playlist is gone).
    pub stale: Vec<Entry>,
    pub manifest: Manifest,
    pub options: SyncOptions,
    pub serato_possible: bool,
    pub fs: Option<String>,
    pub total: Option<u64>,
    pub free: Option<u64>,
    pub bytes_write: u64,
    pub bytes_freed: u64,
    pub problem: Option<String>,
    pub warnings: Vec<String>,
}

/// `path` is `desired` with " (n)" before the extension, in the same folder.
fn is_variant(path: &str, desired: &str) -> bool {
    let (Some((dir_a, name_a)), Some((dir_b, name_b))) = (path.rsplit_once('/'), desired.rsplit_once('/')) else { return false };
    let (stem, ext) = name_b.rsplit_once('.').unwrap_or((name_b, ""));
    let Some(rest) = name_a.strip_prefix(&format!("{stem} (")) else { return false };
    let Some(n) = rest.strip_suffix(&format!(").{ext}")) else { return false };
    dir_a == dir_b && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
}

fn pad(n: usize, of: usize) -> String {
    let w = of.to_string().len().max(2);
    format!("{n:0w$}")
}

fn short(name: &str, max: usize) -> String {
    let n: String = clean_name(name).chars().take(max).collect();
    n.trim_end_matches(['.', ' ']).to_string()
}

/// Bytes a file will probably take.
fn estimate(item: &PlanItem, o: &ExportOptions) -> u64 {
    let s = &item.source;
    let secs = s.duration.unwrap_or(0.0);
    if secs <= 0.0 {
        return s.size.max(0) as u64;
    }
    let pcm = |rate: f64, bits: f64| (secs * rate * 2.0 * bits / 8.0) as u64 + 512 * 1024;
    match o.target {
        Target::Mp3 if s.format.as_deref() == Some("MP3") => s.size.max(0) as u64,
        Target::Mp3 => (secs * 320_000.0 / 8.0) as u64 + 512 * 1024,
        _ if o.quality == Quality::Universal => pcm(44_100.0, 16.0),
        _ => {
            let rate = s.sample_rate.unwrap_or(44_100).min(48_000) as f64;
            let bits = if s.lossless == Some(true) { s.bit_depth.unwrap_or(16).min(24) as f64 } else { 16.0 };
            pcm(rate, bits)
        }
    }
}

/// Playlists to send: the chosen ones and everything inside chosen
/// folders, with their folders on the drive.
/// The chosen playlists with their names on the drive, the folders above
/// them, and the names on the drive of every playlist.
type Layout = (Vec<(Node, Vec<String>)>, Vec<Branch>, HashMap<i64, Vec<String>>);

fn layout(c: &Connection, ids: &[i64], number: bool) -> Result<Layout, PlaylistError> {
    let all: Vec<Node> = c.query(
        "SELECT id, parent_id, kind, name, position, rules, created_at, updated_at FROM playlists ORDER BY position, id",
        crate::params![],
        |r| Node { id: r.i64(0), parent_id: r.opt_i64(1), kind: Kind::from_id(&r.text(2)).unwrap_or(Kind::Playlist), name: r.text(3), position: r.i64(4), rules: None, created_at: 0, updated_at: 0 },
    )?;
    let by_id: HashMap<i64, &Node> = all.iter().map(|n| (n.id, n)).collect();
    for id in ids {
        if !by_id.contains_key(id) {
            return Err(PlaylistError::NotFound(format!("playlist {id} not found")));
        }
    }
    // Names on the drive: numbered by place among siblings, unique among them.
    let mut names: HashMap<i64, String> = HashMap::new();
    let mut siblings: HashMap<Option<i64>, Vec<&Node>> = HashMap::new();
    for n in &all {
        siblings.entry(n.parent_id).or_default().push(n);
    }
    for list in siblings.values() {
        let mut used: HashSet<String> = HashSet::new();
        for (i, n) in list.iter().enumerate() {
            let base = short(&n.name, MAX_FOLDER_NAME);
            let base = if base.is_empty() { "Playlist".to_string() } else { base };
            let mut name = if number { format!("{} {base}", pad(i + 1, list.len())) } else { base.clone() };
            let mut k = 2;
            while !used.insert(name.to_lowercase()) {
                name = if number { format!("{} {base} ({k})", pad(i + 1, list.len())) } else { format!("{base} ({k})") };
                k += 1;
            }
            names.insert(n.id, name);
        }
    }
    let path_of = |id: i64| {
        let mut out = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            out.push(names.get(&i).cloned().unwrap_or_default());
            cur = by_id.get(&i).and_then(|n| n.parent_id);
            if out.len() > 64 {
                break;
            }
        }
        out.reverse();
        out
    };
    let chosen: HashSet<i64> = ids.iter().copied().collect();
    let inside_chosen = |n: &Node| {
        let mut cur = Some(n.id);
        while let Some(i) = cur {
            if chosen.contains(&i) {
                return true;
            }
            cur = by_id.get(&i).and_then(|x| x.parent_id);
        }
        false
    };
    // Tree order: depth first, by position.
    let mut ordered = Vec::new();
    fn walk<'a>(parent: Option<i64>, siblings: &HashMap<Option<i64>, Vec<&'a Node>>, out: &mut Vec<&'a Node>) {
        for n in siblings.get(&parent).cloned().unwrap_or_default() {
            out.push(n);
            walk(Some(n.id), siblings, out);
        }
    }
    walk(None, &siblings, &mut ordered);
    let mut leaves = Vec::new();
    let mut branches = Vec::new();
    let mut needed_branches: HashSet<i64> = HashSet::new();
    for n in &ordered {
        if n.kind != Kind::Folder && inside_chosen(n) {
            leaves.push(((*n).clone(), path_of(n.id)));
            let mut cur = n.parent_id;
            while let Some(p) = cur {
                needed_branches.insert(p);
                cur = by_id.get(&p).and_then(|x| x.parent_id);
            }
        }
    }
    for n in &ordered {
        if n.kind == Kind::Folder && needed_branches.contains(&n.id) {
            branches.push(Branch { id: n.id, name: n.name.clone(), path_names: path_of(n.id) });
        }
    }
    let all_paths = all.iter().filter(|n| n.kind != Kind::Folder).map(|n| (n.id, path_of(n.id))).collect();
    Ok((leaves, branches, all_paths))
}

/// Works out what sending `ids` to `root` would do.
pub fn plan(c: &Connection, root: &Path, ids: &[i64], options: &SyncOptions) -> Result<SyncPlan, PlaylistError> {
    let (nodes, branches, all_paths) = layout(c, ids, options.number_folders)?;
    let manifest = read_manifest(root);
    let mut leaves = Vec::new();
    for (node, path_names) in nodes {
        let track_ids = playlists::track_ids(c, node.id)?;
        let items = djexport::plan(c, &track_ids, &options.export)?;
        let dir = std::iter::once(SYNC_DIR.to_string()).chain(path_names.iter().cloned()).collect::<Vec<_>>().join("/");
        leaves.push(Leaf { id: node.id, name: node.name.clone(), kind: node.kind, dir, path_names, items });
    }
    let ext = options.export.target.extension();
    // Intact files SONORA wrote, by (playlist, track).
    let mut owned: HashMap<(i64, i64), Vec<&Entry>> = HashMap::new();
    let mut intact_paths: HashSet<&str> = HashSet::new();
    for e in &manifest.entries {
        if e.track != 0 && intact(root, e) {
            owned.entry((e.playlist, e.track)).or_default().push(e);
            intact_paths.insert(e.path.as_str());
        }
    }
    let mut files = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    let mut claimed: HashSet<String> = HashSet::new();
    let mut warnings = Vec::new();
    for (li, leaf) in leaves.iter().enumerate() {
        let exported: Vec<usize> = leaf.items.iter().enumerate().filter(|(_, i)| i.skip.is_none()).map(|(k, _)| k).collect();
        for (n, &k) in exported.iter().enumerate() {
            let item = &leaf.items[k];
            let base = short(&base_name(&item.track), MAX_FILE_BASE);
            let path = format!("{}/{} - {base}.{ext}", leaf.dir, pad(n + 1, exported.len().max(100)));
            let key = content_key(item, &options.export);
            let candidates = owned.get(&(leaf.id, item.track.id)).cloned().unwrap_or_default();
            let same = candidates.iter().find(|e| e.key == key && !claimed.contains(&e.path));
            // Someone else's file holds this name: SONORA's copy keeps the
            // "(2)" name it got beside it.
            let foreign = |p: &str| rel_to_path(root, p).exists() && !intact_paths.contains(p);
            let path = match same {
                Some(e) if e.path != path && foreign(&path) && is_variant(&e.path, &path) => e.path.clone(),
                _ => path,
            };
            let action = match same {
                Some(e) if e.path == path => Action::Keep,
                Some(e) => Action::Rename(e.path.clone()),
                None => match candidates.iter().find(|e| !claimed.contains(&e.path)) {
                    Some(old) => Action::Update(old.path.clone()),
                    None => Action::New,
                },
            };
            if let Action::Rename(p) | Action::Update(p) = &action {
                claimed.insert(p.clone());
            }
            if action == Action::Keep {
                claimed.insert(path.clone());
            }
            // A file of someone else's at this place stays; SONORA picks another name then.
            if foreign(&path) && action != Action::Keep {
                warnings.push(format!("{path} is not a file SONORA wrote; the new file gets another name."));
            }
            used.insert(path.clone());
            let est = if matches!(action, Action::Keep | Action::Rename(_)) { 0 } else { estimate(item, &options.export) };
            files.push(FilePlan { leaf: li, item: k, path, action, key, estimate: est });
        }
        let missing = leaf.items.iter().filter(|i| i.skip == Some("missing")).count();
        if missing > 0 {
            warnings.push(format!("{}: {} missing from disk; {} left out.", leaf.name, if missing == 1 { "a track's file is" } else { "some tracks' files are" }, if missing == 1 { "it is" } else { "they are" }));
        }
    }
    // Stale: SONORA's audio files of these playlists that nothing claims,
    // and those of playlists that no longer exist.
    let sent: HashSet<i64> = leaves.iter().map(|l| l.id).collect();
    let existing: HashSet<i64> = c.query("SELECT id FROM playlists", crate::params![], |r| r.i64(0))?.into_iter().collect();
    let stale: Vec<Entry> = manifest
        .entries
        .iter()
        .filter(|e| e.track != 0 && (sent.contains(&e.playlist) || !existing.contains(&e.playlist)) && !claimed.contains(&e.path) && !used.contains(&e.path) && intact(root, e))
        .cloned()
        .collect();
    let replaced: u64 = files.iter().filter_map(|f| if let Action::Update(old) = &f.action { manifest.entries.iter().find(|e| &e.path == old).map(|e| e.bytes) } else { None }).sum();
    let bytes_write: u64 = files.iter().map(|f| f.estimate).sum();
    let bytes_freed = if options.remove_stale { stale.iter().map(|e| e.bytes).sum::<u64>() } else { 0 } + replaced;
    let (total, free) = space(root).map_or((None, None), |(t, f)| (Some(t), Some(f)));
    let fs = drives().into_iter().filter(|d| root.starts_with(&d.path)).max_by_key(|d| d.path.as_os_str().len()).and_then(|d| d.fs);
    let mut problem = None;
    if !root.is_dir() {
        problem = Some(format!("{} is not available. Connect the drive or choose another folder.", root.display()));
    } else if leaves.is_empty() {
        problem = Some("Choose at least one playlist.".into());
    } else if let Some(free) = free {
        let need = bytes_write.saturating_sub(bytes_freed) + SPACE_MARGIN;
        if need > free {
            problem = Some(format!("Not enough space: the files need about {}, the drive has {} free.", human_bytes(need), human_bytes(free)));
        }
    }
    if is_fat32(fs.as_deref()) {
        if let Some(f) = files.iter().find(|f| f.estimate > FAT32_LIMIT) {
            problem = Some(format!("{} would be larger than the 4 GB a FAT32 drive can hold in one file. Choose MP3, or format the drive as exFAT.", f.path));
        }
    }
    // Playlists on the drive from earlier sends, not sent now.
    let mut others = Vec::new();
    let mut by_playlist: HashMap<i64, Vec<&Entry>> = HashMap::new();
    for e in &manifest.entries {
        if e.track != 0 && !sent.contains(&e.playlist) && existing.contains(&e.playlist) && intact(root, e) {
            by_playlist.entry(e.playlist).or_default().push(e);
        }
    }
    for (id, mut entries) in by_playlist {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let rows = get_tracks(c, &entries.iter().map(|e| e.track).collect::<Vec<_>>())?;
        let by_id: HashMap<i64, TrackRow> = rows.into_iter().map(|r| (r.id, r)).collect();
        let files = entries.iter().filter_map(|e| by_id.get(&e.track).map(|t| (e.path.clone(), t.clone()))).collect();
        others.push(Other { id, path_names: all_paths.get(&id).cloned().unwrap_or_default(), files });
    }
    others.sort_by(|a, b| a.path_names.cmp(&b.path_names));
    let serato_possible = is_drive_root(root);
    let n_files = files.len();
    if n_files == 0 && problem.is_none() && !leaves.is_empty() {
        warnings.push("These playlists hold no tracks that can be sent.".into());
    }
    Ok(SyncPlan {
        root: root.to_path_buf(),
        leaves,
        branches,
        others,
        existing,
        files,
        stale,
        manifest,
        options: options.clone(),
        serato_possible,
        fs,
        total,
        free,
        bytes_write,
        bytes_freed,
        problem,
        warnings,
    })
}

fn human_bytes(b: u64) -> String {
    let gb = b as f64 / 1e9;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", (b as f64 / 1e6).max(1.0))
    }
}

impl SyncPlan {
    fn count(&self, f: impl Fn(&Action) -> bool) -> usize {
        self.files.iter().filter(|x| f(&x.action)).count()
    }

    pub fn to_json(&self, running: bool) -> Value {
        let leaves: Vec<Value> = self
            .leaves
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let mine: Vec<&FilePlan> = self.files.iter().filter(|f| f.leaf == i).collect();
                obj! {
                    "id" => l.id,
                    "name" => l.name.clone(),
                    "kind" => l.kind.id(),
                    "dir" => l.dir.clone(),
                    "tracks" => mine.len(),
                    "skipped" => l.items.iter().filter(|x| x.skip.is_some()).count(),
                    "bytes" => mine.iter().map(|f| f.estimate).sum::<u64>(),
                }
            })
            .collect();
        // A preview of the files (the first ones of each playlist).
        let mut shown: HashMap<usize, usize> = HashMap::new();
        let preview: Vec<Value> = self
            .files
            .iter()
            .filter(|f| {
                let n = shown.entry(f.leaf).or_insert(0);
                *n += 1;
                *n <= 60
            })
            .take(600)
            .map(|f| {
                let item = &self.leaves[f.leaf].items[f.item];
                let (action, from) = match &f.action {
                    Action::Keep => ("keep", None),
                    Action::Rename(p) => ("rename", Some(p.clone())),
                    Action::Update(p) => ("update", Some(p.clone())),
                    Action::New => ("new", None),
                };
                obj! {
                    "path" => f.path.clone(), "action" => action, "from" => from, "trackId" => item.track.id,
                    "warnings" => Value::Array(item.warnings.iter().map(ToJson::to_json).collect()),
                }
            })
            .collect();
        obj! {
            "root" => self.root.to_string_lossy().to_string(),
            "folder" => self.root.join(SYNC_DIR).to_string_lossy().to_string(),
            "playlists" => Value::Array(leaves),
            "files" => obj! {
                "total" => self.files.len(),
                "new" => self.count(|a| *a == Action::New),
                "update" => self.count(|a| matches!(a, Action::Update(_))),
                "rename" => self.count(|a| matches!(a, Action::Rename(_))),
                "keep" => self.count(|a| *a == Action::Keep),
                "stale" => self.stale.len(),
                "staleBytes" => self.stale.iter().map(|e| e.bytes).sum::<u64>(),
            },
            "preview" => Value::Array(preview),
            "bytes" => obj! { "write" => self.bytes_write, "freed" => self.bytes_freed, "free" => self.free, "total" => self.total },
            "fs" => self.fs.clone(),
            "fsLabel" => self.fs.as_deref().map(fs_label),
            "seratoPossible" => self.serato_possible,
            "problem" => self.problem.clone(),
            "warnings" => self.warnings.clone(),
            "running" => running,
        }
    }
}

// --------------------------------------------------------- playlist files

/// An M3U8 playlist of the files in its own folder.
pub fn m3u8(name: &str, entries: &[(String, Option<f64>, String)]) -> String {
    let mut s = String::from("#EXTM3U\n");
    s.push_str(&format!("#PLAYLIST:{}\n", name.replace(['\r', '\n'], " ")));
    for (file, secs, title) in entries {
        s.push_str(&format!("#EXTINF:{},{}\n{file}\n", secs.map(|d| d.round() as i64).unwrap_or(-1), title.replace(['\r', '\n'], " ")));
    }
    s
}

fn utf16be(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn serato_field(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(tag);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
}

/// A Serato crate listing `paths` (relative to the drive, `/` between names).
pub fn serato_crate(paths: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    serato_field(&mut out, b"vrsn", &utf16be("1.0/Serato ScratchLive Crate"));
    for p in paths {
        let mut track = Vec::new();
        serato_field(&mut track, b"ptrk", &utf16be(p));
        serato_field(&mut out, b"otrk", &track);
    }
    out
}

/// A crate's file name: nested crates join their names with "%%".
fn crate_name(path_names: &[String]) -> String {
    let parts: Vec<String> = std::iter::once(SYNC_DIR.to_string()).chain(path_names.iter().map(|n| n.replace("%%", "%"))).collect();
    format!("{}.crate", parts.join("%%"))
}

struct XmlTrack {
    path: PathBuf,
    title: String,
    artist: Option<String>,
    album: Option<String>,
    genre: Option<String>,
    bpm: Option<f64>,
    key: Option<String>,
    bytes: u64,
    secs: Option<f64>,
}

/// rekordbox.xml: every track once, and the playlists (names on the drive,
/// outermost first; members as indexes into `tracks`) in a tree under a
/// "SONORA" folder.
fn rekordbox_tree(tracks: &[XmlTrack], playlists: &[(Vec<String>, Vec<usize>)], kind: Target, app_version: &str) -> String {
    let kind = match kind {
        Target::Aiff => "AIFF File",
        Target::Wav => "WAV File",
        Target::Mp3 => "MP3 File",
    };
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<DJ_PLAYLISTS Version=\"1.0.0\">\n");
    x.push_str(&format!("  <PRODUCT Name=\"SONORA\" Version=\"{}\" Company=\"SONORA\"/>\n", xml_escape(app_version)));
    x.push_str(&format!("  <COLLECTION Entries=\"{}\">\n", tracks.len()));
    for (i, t) in tracks.iter().enumerate() {
        let a = |n: &str, v: &str| format!(" {n}=\"{}\"", xml_escape(v));
        let mut s = format!("    <TRACK TrackID=\"{}\"", i + 1);
        s += &a("Name", &t.title);
        s += &a("Artist", t.artist.as_deref().unwrap_or(""));
        s += &a("Album", t.album.as_deref().unwrap_or(""));
        s += &a("Genre", t.genre.as_deref().unwrap_or(""));
        s += &a("Kind", kind);
        s += &a("Size", &t.bytes.to_string());
        s += &a("TotalTime", &t.secs.map(|d| (d.round() as i64).to_string()).unwrap_or_default());
        s += &a("AverageBpm", &t.bpm.map(|b| format!("{b:.2}")).unwrap_or_default());
        s += &a("Tonality", t.key.as_deref().unwrap_or(""));
        s += &a("Location", &file_url(&t.path));
        s += "/>\n";
        x.push_str(&s);
    }
    x.push_str("  </COLLECTION>\n  <PLAYLISTS>\n    <NODE Type=\"0\" Name=\"ROOT\" Count=\"1\">\n");
    #[derive(Default)]
    struct Folder {
        name: String,
        folders: Vec<Folder>,
        playlists: Vec<usize>,
    }
    let mut top = Folder { name: SYNC_DIR.into(), ..Default::default() };
    for (pi, (names, _)) in playlists.iter().enumerate() {
        let mut cur = &mut top;
        for name in &names[..names.len().saturating_sub(1)] {
            let at = match cur.folders.iter().position(|f| &f.name == name) {
                Some(i) => i,
                None => {
                    cur.folders.push(Folder { name: name.clone(), ..Default::default() });
                    cur.folders.len() - 1
                }
            };
            cur = &mut cur.folders[at];
        }
        cur.playlists.push(pi);
    }
    fn write(f: &Folder, playlists: &[(Vec<String>, Vec<usize>)], depth: usize, x: &mut String) {
        let pad = "  ".repeat(depth);
        x.push_str(&format!("{pad}<NODE Type=\"0\" Name=\"{}\" Count=\"{}\">\n", xml_escape(&f.name), f.folders.len() + f.playlists.len()));
        for sub in &f.folders {
            write(sub, playlists, depth + 1, x);
        }
        for &pi in &f.playlists {
            let (names, members) = &playlists[pi];
            let name = names.last().cloned().unwrap_or_default();
            x.push_str(&format!("{pad}  <NODE Name=\"{}\" Type=\"1\" KeyType=\"0\" Entries=\"{}\">\n", xml_escape(&name), members.len()));
            for &t in members {
                x.push_str(&format!("{pad}    <TRACK Key=\"{}\"/>\n", t + 1));
            }
            x.push_str(&format!("{pad}  </NODE>\n"));
        }
        x.push_str(&format!("{pad}</NODE>\n"));
    }
    write(&top, playlists, 3, &mut x);
    x.push_str("    </NODE>\n  </PLAYLISTS>\n</DJ_PLAYLISTS>\n");
    x
}

// ------------------------------------------------------------------- job

/// The one sync that can run at a time.
#[derive(Default)]
pub struct Syncer {
    running: AtomicBool,
    cancel: Arc<AtomicBool>,
    last: Mutex<Option<Value>>,
}

impl Syncer {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) -> bool {
        if self.is_running() {
            self.cancel.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    pub fn last(&self) -> Value {
        self.last.lock().ok().and_then(|l| l.clone()).unwrap_or(Value::Null)
    }

    /// Starts sending on a background thread; `false` if a sync is running.
    pub fn start(self: &Arc<Self>, plan: SyncPlan, events: Arc<EventBus>, tools: ToolPaths, app_version: String) -> bool {
        if self.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.cancel.store(false, Ordering::SeqCst);
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("sonora-usb".into()).spawn(move || {
            let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&plan, &events, &tools, &me.cancel, &app_version))).unwrap_or_else(|_| {
                obj! { "error" => "Sending stopped because of an internal error. Details are in the log.", "root" => plan.root.to_string_lossy().to_string() }
            });
            if let Ok(mut l) = me.last.lock() {
                *l = Some(report.clone());
            }
            me.running.store(false, Ordering::SeqCst);
            events.emit("usb.finished", &report);
        });
        if spawned.is_err() {
            self.running.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }
}

/// Moves `from` to `to` (same drive), never over an existing file.
fn move_file(from: &Path, to: &Path) -> std::io::Result<PathBuf> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let dest = if to.exists() {
        let name = to.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        unique_path(to.parent().unwrap_or(Path::new(".")), &name)
    } else {
        to.to_path_buf()
    };
    std::fs::rename(from, &dest)?;
    Ok(dest)
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/")).unwrap_or_default()
}

/// Writes a playlist file SONORA owns: over its own earlier version when
/// that is unchanged, never over anything else (then under another name).
fn write_owned(root: &Path, rel: &str, playlist: i64, data: &[u8], manifest: &mut Manifest, warnings: &mut Vec<String>) -> Option<String> {
    let path = rel_to_path(root, rel);
    let mine = manifest.entries.iter().position(|e| e.path == rel && e.track == 0);
    let target = if path.exists() {
        match mine {
            Some(i) if intact(root, &manifest.entries[i]) => path.clone(),
            _ => {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                warnings.push(format!("{rel} is not SONORA's; it was left as it is and the new one has another name."));
                unique_path(path.parent().unwrap_or(root), &name)
            }
        }
    } else {
        path.clone()
    };
    if let Some(dir) = target.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return None;
        }
    }
    let tmp = target.with_file_name(format!(".{}.sonora-part", target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
    if std::fs::write(&tmp, data).and_then(|_| std::fs::rename(&tmp, &target)).is_err() {
        let _ = std::fs::remove_file(&tmp);
        warnings.push(format!("{rel} could not be written."));
        return None;
    }
    let rel_written = rel_of(root, &target);
    manifest.entries.retain(|e| e.path != rel_written);
    if let Some(e) = record(root, &rel_written, playlist, 0, "") {
        manifest.entries.push(e);
    }
    Some(rel_written)
}

/// Removes folders below `SONORA/` that held SONORA's files and are now
/// empty, deepest first. Removing a folder that is not empty fails, so
/// nothing else goes.
fn prune_empty(root: &Path, used: &[String]) {
    let mut dirs: HashSet<String> = HashSet::new();
    for rel in used {
        let mut parts: Vec<&str> = rel.split('/').collect();
        parts.pop();
        while parts.len() > 1 && parts[0] == SYNC_DIR {
            dirs.insert(parts.join("/"));
            parts.pop();
        }
    }
    let mut dirs: Vec<String> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
    for d in dirs {
        let _ = std::fs::remove_dir(rel_to_path(root, &d));
    }
}

fn run(plan: &SyncPlan, events: &EventBus, tools: &ToolPaths, cancel: &AtomicBool, app_version: &str) -> Value {
    let started = Instant::now();
    let root = &plan.root;
    let mut manifest = plan.manifest.clone();
    let mut warnings: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let (mut written, mut renamed, mut removed, mut bytes) = (0usize, 0usize, 0usize, 0u64);
    let mut cancelled = false;
    // Where each planned file ended up (relative path), by plan index.
    let mut placed: Vec<Option<String>> = vec![None; plan.files.len()];
    let save = |m: &Manifest, warnings: &mut Vec<String>| {
        if let Err(e) = write_manifest(root, m, app_version) {
            warnings.push(format!("SONORA's record of its files could not be saved: {e}"));
        }
    };
    let write_total = plan.files.iter().filter(|f| matches!(f.action, Action::New | Action::Update(_))).count();
    let mut write_done = 0usize;
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut progress = |done: usize, fraction: f64, current: &str, phase: &str, force: bool| {
        if force || last_emit.elapsed() >= Duration::from_millis(200) {
            last_emit = Instant::now();
            events.emit(
                "usb.progress",
                &obj! { "done" => done, "total" => write_total, "fraction" => num((done as f64 + fraction) / write_total.max(1) as f64, 3), "current" => current, "phase" => phase },
            );
        }
    };
    if let Err(e) = std::fs::create_dir_all(root.join(SYNC_DIR)) {
        return obj! { "error" => format!("{} cannot be written to: {e}", root.display()), "root" => root.to_string_lossy().to_string() };
    }
    progress(0, 0.0, "", "preparing", true);

    // 1. Files that left the playlists, when asked: SONORA's own, unchanged.
    if plan.options.remove_stale {
        for e in &plan.stale {
            if !intact(root, e) {
                continue;
            }
            if std::fs::remove_file(rel_to_path(root, &e.path)).is_ok() {
                manifest.entries.retain(|x| x.path != e.path);
                removed += 1;
            }
        }
        save(&manifest, &mut warnings);
    }

    // 2. Reordered and renamed files: first aside, then into place, so two
    //    files can swap names.
    let mut aside: Vec<(usize, PathBuf)> = Vec::new();
    for (i, f) in plan.files.iter().enumerate() {
        if let Action::Rename(from) = &f.action {
            let src = rel_to_path(root, from);
            let tmp = src.with_file_name(format!(".sonora-move-{i}"));
            match std::fs::rename(&src, &tmp) {
                Ok(()) => aside.push((i, tmp)),
                Err(e) => warnings.push(format!("{from} could not be moved: {e}")),
            }
        }
    }
    for (i, tmp) in aside {
        let f = &plan.files[i];
        let old = if let Action::Rename(from) = &f.action { from.clone() } else { continue };
        match move_file(&tmp, &rel_to_path(root, &f.path)) {
            Ok(dest) => {
                let rel = rel_of(root, &dest);
                if let Some(e) = manifest.entries.iter_mut().find(|e| e.path == old) {
                    e.path = rel.clone();
                    e.playlist = plan.leaves[f.leaf].id;
                }
                placed[i] = Some(rel);
                renamed += 1;
            }
            Err(e) => {
                // Put it back where it was.
                let _ = std::fs::rename(&tmp, rel_to_path(root, &old));
                warnings.push(format!("{old} could not be moved to {}: {e}", f.path));
            }
        }
    }
    for (i, f) in plan.files.iter().enumerate() {
        if f.action == Action::Keep {
            placed[i] = Some(f.path.clone());
        }
    }
    save(&manifest, &mut warnings);

    // 3. New and changed files.
    for (i, f) in plan.files.iter().enumerate() {
        if !matches!(f.action, Action::New | Action::Update(_)) {
            continue;
        }
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let leaf = &plan.leaves[f.leaf];
        let item = &leaf.items[f.item];
        let title = item.track.display_title();
        progress(write_done, 0.0, &title, "writing", true);
        let dest = rel_to_path(root, &f.path);
        let folder = dest.parent().map(Path::to_path_buf).unwrap_or_else(|| root.clone());
        let name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Err(e) = std::fs::create_dir_all(&folder) {
            failed.push(obj! { "trackId" => item.track.id, "title" => title, "error" => format!("the folder could not be made: {e}") });
            continue;
        }
        // The older version steps aside until the new one is complete.
        let old = if let Action::Update(old) = &f.action {
            let src = rel_to_path(root, old);
            let tmp = src.with_file_name(format!(".sonora-old-{i}"));
            match std::fs::rename(&src, &tmp) {
                Ok(()) => Some((old.clone(), tmp)),
                Err(_) => None,
            }
        } else {
            None
        };
        let result = export_file(item, &folder, &name, &plan.options.export, tools, cancel, &mut |p, force| progress(write_done, p, &title, "writing", force));
        match result {
            Ok(w) => {
                let rel = rel_of(root, &w.path);
                if let Some((old_rel, tmp)) = old {
                    let _ = std::fs::remove_file(&tmp);
                    manifest.entries.retain(|e| e.path != old_rel);
                }
                manifest.entries.retain(|e| e.path != rel);
                if let Some(e) = record(root, &rel, leaf.id, item.track.id, &f.key) {
                    manifest.entries.push(e);
                }
                bytes += w.bytes;
                placed[i] = Some(rel);
                written += 1;
            }
            Err(err) => {
                if let Some((old_rel, tmp)) = old {
                    let _ = std::fs::rename(&tmp, rel_to_path(root, &old_rel));
                    // The older version still serves this place.
                    placed[i] = Some(old_rel);
                }
                match err {
                    ExportError::Cancelled => {
                        cancelled = true;
                        break;
                    }
                    ExportError::Failed(msg) => {
                        crate::logging::log_fields("warn", "usb", &msg, obj! { "track" => item.track.id });
                        failed.push(obj! { "trackId" => item.track.id, "title" => title, "error" => msg });
                    }
                }
            }
        }
        write_done += 1;
        if write_done.is_multiple_of(10) {
            save(&manifest, &mut warnings);
        }
    }
    save(&manifest, &mut warnings);

    // 4. Playlist files, for what is on the drive now.
    let mut m3u_count = 0;
    let mut crates = 0;
    let mut xml_path: Option<String> = None;
    let mut written_now: HashSet<String> = HashSet::new();
    let mut xml_tracks: Vec<XmlTrack> = Vec::new();
    let mut xml_index: HashMap<String, usize> = HashMap::new();
    let mut xml_lists: Vec<(Vec<String>, Vec<usize>)> = Vec::new();
    let notation = plan.options.export.notation;
    let mut add_xml = |rel: &str, t: &TrackRow, tracks: &mut Vec<XmlTrack>| {
        *xml_index.entry(rel.to_string()).or_insert_with(|| {
            let p = rel_to_path(root, rel);
            tracks.push(XmlTrack {
                bytes: std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0),
                path: p,
                title: t.title.clone().unwrap_or_else(|| t.display_title()),
                artist: t.artist.clone(),
                album: t.album.clone(),
                genre: t.dj_genre.clone(),
                bpm: t.dj_bpm,
                key: t.dj_key.filter(|k| (0..24).contains(k)).map(|k| format_key(k as u8, notation, true)),
                secs: t.duration,
            });
            tracks.len() - 1
        })
    };
    for (li, leaf) in plan.leaves.iter().enumerate() {
        let mut m3u = Vec::new();
        let mut crate_paths = Vec::new();
        let mut members = Vec::new();
        for (i, f) in plan.files.iter().enumerate().filter(|(_, f)| f.leaf == li) {
            let Some(rel) = &placed[i] else { continue };
            let t = &leaf.items[f.item].track;
            let file = rel.rsplit('/').next().unwrap_or(rel).to_string();
            let display = match t.artist.as_deref().filter(|a| !a.trim().is_empty()) {
                Some(a) => format!("{a} - {}", t.display_title()),
                None => t.display_title(),
            };
            m3u.push((file, t.duration, display));
            crate_paths.push(rel.clone());
            members.push(add_xml(rel, t, &mut xml_tracks));
        }
        xml_lists.push((leaf.path_names.clone(), members));
        if plan.options.m3u {
            let name = format!("{}/{}.m3u8", leaf.dir, short(&leaf.name, MAX_FOLDER_NAME));
            if let Some(rel) = write_owned(root, &name, leaf.id, m3u8(&leaf.name, &m3u).as_bytes(), &mut manifest, &mut warnings) {
                written_now.insert(rel);
                m3u_count += 1;
            }
        }
        if plan.options.serato && plan.serato_possible {
            let rel = format!("{SERATO_DIR}/{}", crate_name(&leaf.path_names));
            if let Some(rel) = write_owned(root, &rel, leaf.id, &serato_crate(&crate_paths), &mut manifest, &mut warnings) {
                written_now.insert(rel);
                crates += 1;
            }
        }
    }
    if plan.options.serato && plan.serato_possible {
        // Folders as crates too, so Serato nests the playlists under them.
        let mut parents: Vec<(i64, Option<Vec<String>>)> = vec![(0, None)];
        parents.extend(plan.branches.iter().map(|b| (b.id, Some(b.path_names.clone()))));
        for (id, p) in parents {
            let rel = format!("{SERATO_DIR}/{}", p.map(|p| crate_name(&p)).unwrap_or_else(|| format!("{SYNC_DIR}.crate")));
            if !rel_to_path(root, &rel).exists() || manifest.entries.iter().any(|e| e.path == rel && e.track == 0) {
                if let Some(rel) = write_owned(root, &rel, id, &serato_crate(&[]), &mut manifest, &mut warnings) {
                    written_now.insert(rel);
                }
            }
        }
    }
    for o in &plan.others {
        let members = o.files.iter().map(|(rel, t)| add_xml(rel, t, &mut xml_tracks)).collect();
        xml_lists.push((o.path_names.clone(), members));
    }
    if plan.options.export.rekordbox_xml && !xml_tracks.is_empty() {
        let xml = rekordbox_tree(&xml_tracks, &xml_lists, plan.options.export.target, app_version);
        xml_path = write_owned(root, &format!("{SYNC_DIR}/rekordbox.xml"), 0, xml.as_bytes(), &mut manifest, &mut warnings);
        if let Some(rel) = &xml_path {
            written_now.insert(rel.clone());
        }
    }
    // Playlist files SONORA made for these playlists (or for playlists and
    // folders that are gone) and did not write again: outdated, so they go,
    // when unchanged since.
    let sent: HashSet<i64> = plan.leaves.iter().map(|l| l.id).chain(plan.branches.iter().map(|b| b.id)).collect();
    let outdated: Vec<Entry> = manifest
        .entries
        .iter()
        .filter(|e| e.track == 0 && e.playlist != 0 && !written_now.contains(&e.path) && (sent.contains(&e.playlist) || !plan.existing.contains(&e.playlist)))
        .cloned()
        .collect();
    for e in outdated {
        if intact(root, &e) && std::fs::remove_file(rel_to_path(root, &e.path)).is_ok() {
            manifest.entries.retain(|x| x.path != e.path);
        }
    }
    save(&manifest, &mut warnings);
    let used: Vec<String> = plan.manifest.entries.iter().map(|e| e.path.clone()).collect();
    prune_empty(root, &used);
    progress(write_total, 0.0, "", "done", true);

    let kept = plan.files.iter().filter(|f| f.action == Action::Keep).count();
    obj! {
        "root" => root.to_string_lossy().to_string(),
        "folder" => root.join(SYNC_DIR).to_string_lossy().to_string(),
        "playlists" => plan.leaves.len(),
        "written" => written,
        "renamed" => renamed,
        "kept" => kept,
        "removed" => removed,
        "bytes" => bytes,
        "failed" => Value::Array(failed),
        "m3u" => m3u_count,
        "crates" => crates,
        "rekordboxXml" => xml_path.map(|r| rel_to_path(root, &r).to_string_lossy().to_string()),
        "warnings" => warnings,
        "cancelled" => cancelled,
        "elapsedMs" => started.elapsed().as_millis() as i64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_output_keeps_writable_external_volumes() {
        let text = "/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)\n\
                    /dev/disk3s5 on /System/Volumes/Data (apfs, local, journaled, nobrowse)\n\
                    /dev/disk4s1 on /Volumes/USB STICK (msdos, local, nodev, nosuid, noowners, noatime, fskit)\n\
                    /dev/disk5s1 on /Volumes/SONORA (2) (hfs, local, nodev, nosuid, read-only, noowners, quarantine, mounted by dj)\n\
                    //dj@nas/music on /Volumes/music (smbfs, nodev, nosuid, mounted by dj)\n\
                    /dev/disk6s2 on /Volumes/Gig (exfat, local, nodev, nosuid, noowners)\n";
        let v = parse_mount(text);
        assert_eq!(v, vec![(PathBuf::from("/Volumes/USB STICK"), "msdos".to_string()), (PathBuf::from("/Volumes/Gig"), "exfat".to_string())]);
        assert_eq!(fs_label("msdos"), "FAT32");
        assert_eq!(fs_label("exfat"), "exFAT");
    }

    #[test]
    fn serato_crates_and_m3u() {
        let c = serato_crate(&["SONORA/01 Set/001 - A - B.mp3".into()]);
        assert_eq!(&c[0..4], b"vrsn");
        let vlen = u32::from_be_bytes([c[4], c[5], c[6], c[7]]) as usize;
        assert_eq!(vlen, "1.0/Serato ScratchLive Crate".len() * 2);
        let rest = &c[8 + vlen..];
        assert_eq!(&rest[0..4], b"otrk");
        assert_eq!(&rest[8..12], b"ptrk");
        let plen = u32::from_be_bytes([rest[12], rest[13], rest[14], rest[15]]) as usize;
        let path: Vec<u16> = rest[16..16 + plen].chunks(2).map(|b| u16::from_be_bytes([b[0], b[1]])).collect();
        assert_eq!(String::from_utf16(&path).unwrap(), "SONORA/01 Set/001 - A - B.mp3");
        assert_eq!(crate_name(&["01 Sets".into(), "02 Warm%%up".into()]), "SONORA%%01 Sets%%02 Warm%up.crate");
        let m = m3u8("Warm-up", &[("001 - A - B.aiff".into(), Some(301.6), "A - B".into())]);
        assert_eq!(m, "#EXTM3U\n#PLAYLIST:Warm-up\n#EXTINF:302,A - B\n001 - A - B.aiff\n");
    }

    #[test]
    fn manifest_paths_stay_on_the_drive() {
        let ok = obj! { "path" => "SONORA/01 Set/001 - A.aiff", "bytes" => 10 };
        assert!(Entry::from_json(&ok).is_some());
        assert!(Entry::from_json(&obj! { "path" => "_Serato_/Subcrates/SONORA%%01 Set.crate", "bytes" => 10 }).is_some());
        for bad in ["../x.aiff", "/etc/passwd", "SONORA/../../x", "SONORA//x", "C:\\x", "", "SONORA/C:x.aiff", "SONORA/a\\..\\..\\x", "Music/x.aiff", "SONORA", "SONORA/x.aiff:stream"] {
            assert!(Entry::from_json(&obj! { "path" => bad, "bytes" => 10 }).is_none(), "{bad}");
        }
        assert_eq!(pad(7, 12), "07");
        assert_eq!(pad(7, 100), "007");
        assert_eq!(pad(7, 1200), "0007");
    }
}
