//! Moving files to the system's Trash (macOS), Recycle Bin (Windows) or the
//! freedesktop.org trash (other systems), from where the user can restore
//! them. Nothing here deletes a file for good: when a file cannot go to the
//! Trash, it stays where it is and the caller gets the reason.

use std::path::Path;

/// What the Trash is called on this system, as the interface names it.
pub fn trash_name() -> &'static str {
    if cfg!(windows) {
        "Recycle Bin"
    } else {
        "Trash"
    }
}

/// Moves `path` (a file) to the Trash. On success the file is no longer at
/// `path`.
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{} is not a full path", path.display()));
    }
    // A link goes to the Trash itself, as in Finder; what it points to stays.
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !(meta.is_file() || meta.file_type().is_symlink()) {
        return Err(format!("{} is not a file", path.display()));
    }
    platform::trash(path)?;
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(format!("{} is still in its folder: it was not moved to the {}", path.display(), trash_name()));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod platform {
    //! NSFileManager's trashItemAtURL through the Objective-C runtime: the
    //! same call Finder's "Move to Trash" makes, including the volume's own
    //! Trash on external drives.

    use std::ffi::{c_char, c_void, CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    type Id = *mut c_void;
    type Sel = *mut c_void;

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    pub fn trash(path: &Path) -> Result<(), String> {
        let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| format!("{} is not a valid path", path.display()))?;
        // SAFETY: Objective-C messages to Foundation classes with the
        // argument and return types of their declarations; objc_msgSend is
        // called through a pointer of the exact type, as the ABI requires.
        // Objects returned are autoreleased into the pool popped below.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let result = send(path, &c_path);
            objc_autoreleasePoolPop(pool);
            result
        }
    }

    unsafe fn send(path: &Path, c_path: &CStr) -> Result<(), String> {
        let msg = objc_msgSend as unsafe extern "C" fn();
        let to_id: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(msg);
        let to_id_with_str: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id = std::mem::transmute(msg);
        let to_id_with_id: unsafe extern "C" fn(Id, Sel, Id) -> Id = std::mem::transmute(msg);
        let to_str: unsafe extern "C" fn(Id, Sel) -> *const c_char = std::mem::transmute(msg);
        let trash_item: unsafe extern "C" fn(Id, Sel, Id, *mut Id, *mut Id) -> i8 = std::mem::transmute(msg);
        let sel = |name: &CStr| sel_registerName(name.as_ptr());

        let ns_string = objc_getClass(c"NSString".as_ptr());
        let ns_url = objc_getClass(c"NSURL".as_ptr());
        let file_manager = objc_getClass(c"NSFileManager".as_ptr());
        if ns_string.is_null() || ns_url.is_null() || file_manager.is_null() {
            return Err("the system's Trash is not available".into());
        }
        let text = to_id_with_str(ns_string, sel(c"stringWithUTF8String:"), c_path.as_ptr());
        if text.is_null() {
            return Err(format!("{} is not a valid path", path.display()));
        }
        let url = to_id_with_id(ns_url, sel(c"fileURLWithPath:"), text);
        let manager = to_id(file_manager, sel(c"defaultManager"));
        let mut error: Id = std::ptr::null_mut();
        let moved = trash_item(manager, sel(c"trashItemAtURL:resultingItemURL:error:"), url, std::ptr::null_mut(), &mut error);
        if moved != 0 {
            return Ok(());
        }
        let reason = if error.is_null() {
            None
        } else {
            let description = to_id(error, sel(c"localizedDescription"));
            let utf8 = if description.is_null() { std::ptr::null() } else { to_str(description, sel(c"UTF8String")) };
            (!utf8.is_null()).then(|| CStr::from_ptr(utf8).to_string_lossy().into_owned())
        };
        Err(reason.unwrap_or_else(|| format!("{} could not be moved to the Trash", path.display())))
    }
}

#[cfg(windows)]
mod platform {
    //! SHFileOperation with "allow undo": the file goes to the Recycle Bin.
    //! USB sticks, memory cards and network shares have no Recycle Bin, and
    //! Windows would delete a file there for good, so SONORA leaves such
    //! files where they are. When a fixed drive cannot take a file (its
    //! Recycle Bin is turned off or too small), Windows asks first
    //! (FOF_WANTNUKEWARNING), and declining leaves the file in place.

    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    #[repr(C)]
    struct FileOp {
        hwnd: *mut c_void,
        func: u32,
        from: *const u16,
        to: *const u16,
        flags: u16,
        any_aborted: i32,
        name_mappings: *mut c_void,
        progress_title: *const u16,
    }

    const FO_DELETE: u32 = 3;
    const FOF_SILENT: u16 = 0x0004;
    const FOF_NOCONFIRMATION: u16 = 0x0010;
    const FOF_ALLOWUNDO: u16 = 0x0040;
    const FOF_NOERRORUI: u16 = 0x0400;
    const FOF_WANTNUKEWARNING: u16 = 0x4000;
    const DRIVE_FIXED: u32 = 3;

    #[link(name = "shell32")]
    extern "system" {
        fn SHFileOperationW(op: *mut FileOp) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetVolumePathNameW(file: *const u16, volume: *mut u16, len: u32) -> i32;
        fn GetDriveTypeW(root: *const u16) -> u32;
    }

    /// Whether the drive holding `path` (NUL-terminated) has a Recycle Bin:
    /// fixed drives do; removable ones, network shares, discs and RAM disks
    /// do not.
    fn has_recycle_bin(path: &[u16]) -> bool {
        let mut root = vec![0u16; 1024];
        // SAFETY: `path` is NUL-terminated and `root` holds `root.len()` units.
        let found = unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) };
        // SAFETY: on success GetVolumePathNameW wrote a NUL-terminated path.
        found != 0 && unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_FIXED
    }

    pub fn trash(path: &Path) -> Result<(), String> {
        let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
        if name.contains(&0) {
            return Err(format!("{} is not a valid path", path.display()));
        }
        name.push(0);
        if !has_recycle_bin(&name) {
            return Err("the drive it is on has no Recycle Bin (a USB stick, a memory card or a network share), so the file was left where it is. Delete it in File Explorer if you are sure".into());
        }
        // A list of paths, each NUL-terminated, ending with an empty one.
        let mut from = name;
        from.push(0);
        let mut op = FileOp {
            hwnd: std::ptr::null_mut(),
            func: FO_DELETE,
            from: from.as_ptr(),
            to: std::ptr::null(),
            flags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING | FOF_SILENT | FOF_NOERRORUI,
            any_aborted: 0,
            name_mappings: std::ptr::null_mut(),
            progress_title: std::ptr::null(),
        };
        // SAFETY: `op` is a valid SHFILEOPSTRUCTW (natural alignment on
        // 64-bit Windows) whose `from` list outlives the call.
        let code = unsafe { SHFileOperationW(&mut op) };
        if code != 0 {
            return Err(format!("{} could not be moved to the Recycle Bin (error {code:#x})", path.display()));
        }
        if op.any_aborted != 0 {
            return Err(format!("{} was not moved to the Recycle Bin", path.display()));
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    //! The freedesktop.org trash in the user's home (used for development and
    //! tests; SONORA ships for macOS and Windows).

    use std::path::{Path, PathBuf};

    pub fn trash(path: &Path) -> Result<(), String> {
        let home = std::env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok_or("no home folder to keep a trash in")?;
        super::trash_into(path, &home.join("Trash"))
    }
}

/// Moves `path` into the freedesktop.org trash at `trash_dir` (its `files`
/// and `info` folders), on the same drive only.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn trash_into(path: &Path, trash_dir: &Path) -> Result<(), String> {
    let files = trash_dir.join("files");
    let info = trash_dir.join("info");
    std::fs::create_dir_all(&files).and_then(|_| std::fs::create_dir_all(&info)).map_err(|e| format!("the trash at {} cannot be used: {e}", trash_dir.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    for n in 1..10_000 {
        let candidate = if n == 1 { name.clone() } else { format!("{name}.{n}") };
        let info_path = info.join(format!("{candidate}.trashinfo"));
        // Creating the info file first reserves the name.
        let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&info_path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("the trash at {} cannot be used: {e}", trash_dir.display())),
        };
        let target = files.join(&candidate);
        if target.exists() {
            let _ = std::fs::remove_file(&info_path);
            continue;
        }
        use std::io::Write;
        let record = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", percent_encode(&path.to_string_lossy()), deletion_date());
        if let Err(e) = file.write_all(record.as_bytes()) {
            let _ = std::fs::remove_file(&info_path);
            return Err(format!("the trash at {} cannot be used: {e}", trash_dir.display()));
        }
        return match std::fs::rename(path, &target) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&info_path);
                Err(format!("{} could not be moved to the trash (it may be on another drive): {e}", path.display()))
            }
        };
    }
    Err(format!("{} could not be moved to the trash: too many files of that name in it", path.display()))
}

#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// "2026-10-20T04:30:00" (UTC: the standard library has no time zones).
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn deletion_date() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}", rem / 3_600, rem % 3_600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("sonora-trash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn freedesktop_trash_keeps_the_file_and_where_it_came_from() {
        let d = tmp("fd");
        let trash = d.join("Trash");
        let a = d.join("music/a b.flac");
        std::fs::create_dir_all(a.parent().unwrap()).unwrap();
        std::fs::write(&a, b"one").unwrap();
        trash_into(&a, &trash).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read(trash.join("files/a b.flac")).unwrap(), b"one");
        let info = std::fs::read_to_string(trash.join("info/a b.flac.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath=/"), "{info}");
        assert!(info.contains("a%20b.flac"), "{info}");
        // A second file of the same name gets another name in the trash.
        std::fs::write(&a, b"two").unwrap();
        trash_into(&a, &trash).unwrap();
        assert_eq!(std::fs::read(trash.join("files/a b.flac.2")).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_files_go_to_the_trash() {
        let d = tmp("only");
        assert!(move_to_trash(&d).is_err(), "a folder");
        assert!(move_to_trash(&d.join("missing.flac")).is_err());
        assert!(move_to_trash(Path::new("relative.flac")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    // The system's own Trash: macOS and Windows (CI runs both). Elsewhere
    // the freedesktop trash in the home folder, which tests leave alone.
    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn files_go_to_the_system_trash() {
        let d = tmp("system");
        let f = d.join(format!("sonora trash test {}.wav", std::process::id()));
        std::fs::write(&f, b"RIFF").unwrap();
        move_to_trash(&f).unwrap();
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn dates_are_civil() {
        let s = deletion_date();
        assert_eq!(s.len(), 19, "{s}");
        assert!(s.starts_with("20"), "{s}");
        assert_eq!(&s[10..11], "T");
    }
}
