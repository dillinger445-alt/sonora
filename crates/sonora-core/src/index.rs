//! Filesystem indexing: discovering audio files and identifying them.
//!
//! Candidates are chosen by extension (cheap), but nothing downstream trusts
//! the extension: the analysis identifies the format from file content.
//! Each file gets a *quick identity* (size plus hashes of three 64 KiB
//! windows) that survives renames and moves, so a moved file can be
//! re-linked to its existing analysis instead of being analysed again.

use sonora_engine::hash::Hash128;
use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "wave", "bwf", "flac", "aif", "aiff", "aifc", "mp3", "m4a", "mp4", "aac", "alac", "ogg", "oga", "opus", "wma", "wv", "ape", "caf", "mka", "webm",
];

const MAX_DEPTH: usize = 64;
const WINDOW: u64 = 64 * 1024;

pub fn is_audio_path(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub path: PathBuf,
    pub size: u64,
    /// Modification time, ms since the epoch.
    pub mtime: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WalkStats {
    pub files_seen: u64,
    pub audio_files: u64,
    pub dirs: u64,
    pub unreadable: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct WalkOptions {
    pub skip_hidden: bool,
    pub follow_symlinks: bool,
}

pub fn file_candidate(path: &Path) -> std::io::Result<Candidate> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"));
    }
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
    Ok(Candidate { path: path.to_path_buf(), size: meta.len(), mtime })
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

/// Walks `root` depth-first, calling `on_file` for every audio candidate.
/// Unreadable directories are recorded and skipped; symlink loops are
/// impossible because symlinked directories are either skipped or tracked
/// by canonical path.
pub fn walk(root: &Path, opts: WalkOptions, cancel: &AtomicBool, mut on_file: impl FnMut(Candidate)) -> WalkStats {
    let mut stats = WalkStats::default();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if let Ok(canon) = dir.canonicalize() {
            if !visited.insert(canon) {
                continue;
            }
        }
        stats.dirs += 1;
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                if stats.unreadable.len() < 50 {
                    stats.unreadable.push(format!("{}: {e}", dir.display()));
                }
                continue;
            }
        };
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            if opts.skip_hidden && is_hidden(&name) {
                continue;
            }
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let (is_dir, is_file) = if ft.is_symlink() {
                if !opts.follow_symlinks {
                    continue;
                }
                match std::fs::metadata(&path) {
                    Ok(m) => (m.is_dir(), m.is_file()),
                    Err(_) => continue,
                }
            } else {
                (ft.is_dir(), ft.is_file())
            };
            if is_dir {
                if depth + 1 < MAX_DEPTH {
                    subdirs.push(path);
                }
            } else if is_file {
                stats.files_seen += 1;
                if is_audio_path(&path) {
                    if let Ok(c) = file_candidate(&path) {
                        stats.audio_files += 1;
                        on_file(c);
                    }
                }
            }
        }
        // Visit sub-directories in name order (stack: reverse push).
        subdirs.sort();
        for d in subdirs.into_iter().rev() {
            stack.push((d, depth + 1));
        }
    }
    stats
}

/// Size + hashes of the first, middle and last 64 KiB: a cheap identity that
/// survives renames and moves (32 hex chars).
pub fn quick_hash(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    let mut h = Hash128::new();
    h.update(&size.to_le_bytes());
    let windows: Vec<(u64, u64)> =
        if size <= 3 * WINDOW { vec![(0, size)] } else { vec![(0, WINDOW), (size / 2 - WINDOW / 2, WINDOW), (size - WINDOW, WINDOW)] };
    let mut buf = Vec::new();
    for (off, len) in windows {
        f.seek(SeekFrom::Start(off))?;
        buf.clear();
        (&mut f).take(len).read_to_end(&mut buf)?;
        h.update(&buf);
    }
    Ok(h.hex())
}

/// `path` lies inside `root` (component-wise, so "/music2" is not inside "/music").
pub fn is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// Validates a user-supplied path: absolute and existing.
pub fn validate_path(p: &str) -> Result<PathBuf, String> {
    if p.trim().is_empty() {
        return Err("empty path".into());
    }
    if p.contains('\0') {
        return Err("path contains a NUL byte".into());
    }
    let path = PathBuf::from(p);
    if !path.is_absolute() {
        return Err(format!("not an absolute path: {p}"));
    }
    if !path.exists() {
        return Err(format!("does not exist: {p}"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("sonora-index-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rand_suffix() -> u64 {
        std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64 % 1_000_000_007
    }

    #[test]
    fn walk_filters_and_skips_hidden() {
        let d = tmp();
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::create_dir_all(d.join(".hidden")).unwrap();
        for f in ["a/1.flac", "a/b/2.MP3", "a/notes.txt", ".hidden/3.wav", "a/.4.wav"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let cancel = AtomicBool::new(false);
        let mut found = Vec::new();
        let s = walk(&d, WalkOptions { skip_hidden: true, follow_symlinks: false }, &cancel, |c| found.push(c.path));
        found.sort();
        assert_eq!(found, vec![d.join("a/1.flac"), d.join("a/b/2.MP3")]);
        assert_eq!(s.audio_files, 2);
        let mut all = 0;
        walk(&d, WalkOptions { skip_hidden: false, follow_symlinks: false }, &cancel, |_| all += 1);
        assert_eq!(all, 4);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_loops_terminate() {
        let d = tmp();
        std::fs::create_dir_all(d.join("x")).unwrap();
        std::fs::write(d.join("x/a.wav"), b"x").unwrap();
        std::os::unix::fs::symlink(&d, d.join("x/loop")).unwrap();
        let cancel = AtomicBool::new(false);
        let mut n = 0;
        walk(&d, WalkOptions { skip_hidden: true, follow_symlinks: true }, &cancel, |_| n += 1);
        assert_eq!(n, 1);
    }

    #[test]
    fn quick_hash_identity() {
        let d = tmp();
        let big: Vec<u8> = (0..500_000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(d.join("a.wav"), &big).unwrap();
        std::fs::write(d.join("b.wav"), &big).unwrap();
        let mut changed = big.clone();
        changed[250_000] ^= 1;
        std::fs::write(d.join("c.wav"), &changed).unwrap();
        std::fs::write(d.join("small.wav"), b"tiny").unwrap();
        let (a, b, c) = (quick_hash(&d.join("a.wav")).unwrap(), quick_hash(&d.join("b.wav")).unwrap(), quick_hash(&d.join("c.wav")).unwrap());
        assert_eq!(a, b);
        assert_ne!(a, c, "middle window covers the change");
        assert_eq!(quick_hash(&d.join("small.wav")).unwrap().len(), 32);
    }

    #[test]
    fn path_validation_and_containment() {
        assert!(validate_path("relative/path").is_err());
        assert!(validate_path("").is_err());
        assert!(is_within(Path::new("/music/a/b.flac"), Path::new("/music")));
        assert!(!is_within(Path::new("/music2/b.flac"), Path::new("/music")));
    }
}
