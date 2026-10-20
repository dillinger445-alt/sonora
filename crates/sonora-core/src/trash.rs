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
    //! IFileOperation, as Explorer and Chromium use it, with a progress sink
    //! that refuses every delete Windows could not recycle: when a drive's
    //! Recycle Bin is turned off or too small for the file, the file stays
    //! where it is rather than being deleted for good. (SHFileOperation with
    //! FOF_NOCONFIRMATION can delete such a file without asking.) USB
    //! sticks, memory cards and network shares, which have no Recycle Bin,
    //! are refused up front with a clearer reason.

    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    type Hresult = i32;

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    impl Guid {
        fn is(&self, other: &Guid) -> bool {
            self.data1 == other.data1 && self.data2 == other.data2 && self.data3 == other.data3 && self.data4 == other.data4
        }
    }

    const CLSID_FILE_OPERATION: Guid = Guid { data1: 0x3ad0_5575, data2: 0x8857, data3: 0x4850, data4: [0x92, 0x77, 0x11, 0xb8, 0x5b, 0xdb, 0x8e, 0x09] };
    const IID_IFILE_OPERATION: Guid = Guid { data1: 0x947a_ab5f, data2: 0x0a5c, data3: 0x4c13, data4: [0xb4, 0xd6, 0x4b, 0xf7, 0x83, 0x6f, 0xc9, 0xf8] };
    const IID_ISHELL_ITEM: Guid = Guid { data1: 0x4382_6d1e, data2: 0xe718, data3: 0x42ee, data4: [0xbc, 0x55, 0xa1, 0xe2, 0x61, 0xc3, 0x7b, 0xfe] };
    const IID_IUNKNOWN: Guid = Guid { data1: 0, data2: 0, data3: 0, data4: [0xc0, 0, 0, 0, 0, 0, 0, 0x46] };
    const IID_IPROGRESS_SINK: Guid = Guid { data1: 0x04b0_f1a7, data2: 0x9490, data3: 0x44bc, data4: [0x96, 0xe1, 0x42, 0x96, 0xa3, 0x12, 0x52, 0xe2] };

    const S_OK: Hresult = 0;
    const E_POINTER: Hresult = 0x8000_4003_u32 as i32;
    const E_NOINTERFACE: Hresult = 0x8000_4002_u32 as i32;
    const E_ABORT: Hresult = 0x8000_4004_u32 as i32;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    const COINIT_DISABLE_OLE1DDE: u32 = 0x4;
    const CLSCTX_ALL: u32 = 0x17;
    // FOF_NO_UI (silent, no confirmation, no error dialog, no new-folder
    // confirmation): nothing is ever asked, because the sink refuses what
    // cannot be recycled.
    const FOF_NO_UI: u32 = 0x0004 | 0x0010 | 0x0400 | 0x0200;
    const FOFX_RECYCLEONDELETE: u32 = 0x0008_0000;
    const FOFX_ADDUNDORECORD: u32 = 0x2000_0000;
    const TSF_DELETE_RECYCLE_IF_POSSIBLE: u32 = 0x80;
    const DRIVE_FIXED: u32 = 3;

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> Hresult;
        fn CoUninitialize();
        fn CoCreateInstance(clsid: *const Guid, outer: *mut c_void, context: u32, iid: *const Guid, out: *mut *mut c_void) -> Hresult;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn SHCreateItemFromParsingName(path: *const u16, bind: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> Hresult;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetVolumePathNameW(file: *const u16, volume: *mut u16, len: u32) -> i32;
        fn GetDriveTypeW(root: *const u16) -> u32;
    }

    /// IUnknown: the first three slots of every COM object's table.
    #[repr(C)]
    struct UnknownVtbl {
        query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> Hresult,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }

    /// IFileOperation's table, in declaration order (shobjidl_core.h).
    /// Slots SONORA never calls are plain pointers.
    #[repr(C)]
    struct FileOperationVtbl {
        unknown: UnknownVtbl,
        advise: usize,
        unadvise: usize,
        set_operation_flags: unsafe extern "system" fn(*mut c_void, u32) -> Hresult,
        set_progress_message: usize,
        set_progress_dialog: usize,
        set_properties: usize,
        set_owner_window: usize,
        apply_properties_to_item: usize,
        apply_properties_to_items: usize,
        rename_item: usize,
        rename_items: usize,
        move_item: usize,
        move_items: usize,
        copy_item: usize,
        copy_items: usize,
        delete_item: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> Hresult,
        delete_items: usize,
        new_item: usize,
        perform_operations: unsafe extern "system" fn(*mut c_void) -> Hresult,
        get_any_operations_aborted: unsafe extern "system" fn(*mut c_void, *mut i32) -> Hresult,
    }

    type Item = *mut c_void;
    type Text = *const u16;

    /// IFileOperationProgressSink's table, in declaration order.
    #[repr(C)]
    struct SinkVtbl {
        unknown: UnknownVtbl,
        start_operations: unsafe extern "system" fn(*mut c_void) -> Hresult,
        finish_operations: unsafe extern "system" fn(*mut c_void, Hresult) -> Hresult,
        pre_rename_item: unsafe extern "system" fn(*mut c_void, u32, Item, Text) -> Hresult,
        post_rename_item: unsafe extern "system" fn(*mut c_void, u32, Item, Text, Hresult, Item) -> Hresult,
        pre_move_item: unsafe extern "system" fn(*mut c_void, u32, Item, Item, Text) -> Hresult,
        post_move_item: unsafe extern "system" fn(*mut c_void, u32, Item, Item, Text, Hresult, Item) -> Hresult,
        pre_copy_item: unsafe extern "system" fn(*mut c_void, u32, Item, Item, Text) -> Hresult,
        post_copy_item: unsafe extern "system" fn(*mut c_void, u32, Item, Item, Text, Hresult, Item) -> Hresult,
        pre_delete_item: unsafe extern "system" fn(*mut c_void, u32, Item) -> Hresult,
        post_delete_item: unsafe extern "system" fn(*mut c_void, u32, Item, Hresult, Item) -> Hresult,
        pre_new_item: unsafe extern "system" fn(*mut c_void, u32, Item, Text) -> Hresult,
        post_new_item: unsafe extern "system" fn(*mut c_void, u32, Item, Text, Text, u32, Hresult, Item) -> Hresult,
        update_progress: unsafe extern "system" fn(*mut c_void, u32, u32) -> Hresult,
        reset_timer: unsafe extern "system" fn(*mut c_void) -> Hresult,
        pause_timer: unsafe extern "system" fn(*mut c_void) -> Hresult,
        resume_timer: unsafe extern "system" fn(*mut c_void) -> Hresult,
    }

    /// The progress sink: a COM object of SONORA's own.
    #[repr(C)]
    struct Sink {
        vtbl: *const SinkVtbl,
        refs: AtomicU32,
        /// Windows was about to delete the file for good, and was stopped.
        refused: AtomicBool,
    }

    unsafe extern "system" fn sink_query(this: *mut c_void, iid: *const Guid, out: *mut *mut c_void) -> Hresult {
        if out.is_null() {
            return E_POINTER;
        }
        if !iid.is_null() && ((*iid).is(&IID_IUNKNOWN) || (*iid).is(&IID_IPROGRESS_SINK)) {
            sink_add_ref(this);
            *out = this;
            S_OK
        } else {
            *out = null_mut();
            E_NOINTERFACE
        }
    }
    unsafe extern "system" fn sink_add_ref(this: *mut c_void) -> u32 {
        (*(this as *const Sink)).refs.fetch_add(1, Ordering::SeqCst) + 1
    }
    // The sink belongs to `recycle`, which frees it; Release only counts.
    unsafe extern "system" fn sink_release(this: *mut c_void) -> u32 {
        (*(this as *const Sink)).refs.fetch_sub(1, Ordering::SeqCst).saturating_sub(1)
    }
    unsafe extern "system" fn pre_delete(this: *mut c_void, flags: u32, _item: Item) -> Hresult {
        if flags & TSF_DELETE_RECYCLE_IF_POSSIBLE == 0 {
            // Not recyclable: deleting would destroy the file. Stop instead.
            (*(this as *const Sink)).refused.store(true, Ordering::SeqCst);
            return E_ABORT;
        }
        S_OK
    }
    unsafe extern "system" fn ok(_: *mut c_void) -> Hresult {
        S_OK
    }
    unsafe extern "system" fn ok_hr(_: *mut c_void, _: Hresult) -> Hresult {
        S_OK
    }
    unsafe extern "system" fn ok_rename(_: *mut c_void, _: u32, _: Item, _: Text) -> Hresult {
        S_OK
    }
    unsafe extern "system" fn ok_post_rename(_: *mut c_void, _: u32, _: Item, _: Text, _: Hresult, _: Item) -> Hresult {
        S_OK
    }
    unsafe extern "system" fn ok_transfer(_: *mut c_void, _: u32, _: Item, _: Item, _: Text) -> Hresult {
        S_OK
    }
    unsafe extern "system" fn ok_post_transfer(_: *mut c_void, _: u32, _: Item, _: Item, _: Text, _: Hresult, _: Item) -> Hresult {
        S_OK
    }
    unsafe extern "system" fn ok_post_delete(_: *mut c_void, _: u32, _: Item, _: Hresult, _: Item) -> Hresult {
        S_OK
    }
    unsafe extern "system" fn ok_post_new(_: *mut c_void, _: u32, _: Item, _: Text, _: Text, _: u32, _: Hresult, _: Item) -> Hresult {
        S_OK
    }
    unsafe extern "system" fn ok_progress(_: *mut c_void, _: u32, _: u32) -> Hresult {
        S_OK
    }

    static SINK_VTBL: SinkVtbl = SinkVtbl {
        unknown: UnknownVtbl { query_interface: sink_query, add_ref: sink_add_ref, release: sink_release },
        start_operations: ok,
        finish_operations: ok_hr,
        pre_rename_item: ok_rename,
        post_rename_item: ok_post_rename,
        pre_move_item: ok_transfer,
        post_move_item: ok_post_transfer,
        pre_copy_item: ok_transfer,
        post_copy_item: ok_post_transfer,
        pre_delete_item: pre_delete,
        post_delete_item: ok_post_delete,
        pre_new_item: ok_rename,
        post_new_item: ok_post_new,
        update_progress: ok_progress,
        reset_timer: ok,
        pause_timer: ok,
        resume_timer: ok,
    };

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
        // IFileOperation works in a single-threaded apartment: a thread of
        // its own, whatever the caller's thread has set up.
        let shown = path.display().to_string();
        std::thread::Builder::new()
            .name("sonora-recycle".into())
            // SAFETY: see `recycle`.
            .spawn(move || unsafe { recycle(&name) })
            .map_err(|e| format!("{shown} could not be moved to the Recycle Bin ({e})"))?
            .join()
            .map_err(|_| format!("{shown} could not be moved to the Recycle Bin"))?
            .map_err(|e| match e {
                Failure::NotRecyclable => "Windows could not put it in the Recycle Bin (it may be turned off for this drive, or the file may be too big for it), so the file was left where it is".to_string(),
                Failure::Code(step, hr) => format!("{shown} could not be moved to the Recycle Bin ({step} failed, error {:#010x})", hr as u32),
                Failure::Aborted => format!("{shown} was not moved to the Recycle Bin"),
            })
    }

    enum Failure {
        /// The sink stopped a delete that would not have gone to the Recycle Bin.
        NotRecyclable,
        Code(&'static str, Hresult),
        Aborted,
    }

    fn check(step: &'static str, hr: Hresult) -> Result<(), Failure> {
        if hr < 0 {
            Err(Failure::Code(step, hr))
        } else {
            Ok(())
        }
    }

    /// Recycles one file in an apartment of this thread's own.
    ///
    /// SAFETY: `name` is a NUL-terminated path; the COM objects are used
    /// through tables laid out as their declarations, released before the
    /// apartment closes, and the sink outlives every use Windows makes of it
    /// (it is only freed once its count shows no reference is left).
    unsafe fn recycle(name: &[u16]) -> Result<(), Failure> {
        check("CoInitializeEx", CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE))?;
        let sink = Box::into_raw(Box::new(Sink { vtbl: &SINK_VTBL, refs: AtomicU32::new(1), refused: AtomicBool::new(false) }));
        let mut op: *mut c_void = null_mut();
        let mut item: *mut c_void = null_mut();
        let result = (|| {
            check("CoCreateInstance", CoCreateInstance(&CLSID_FILE_OPERATION, null_mut(), CLSCTX_ALL, &IID_IFILE_OPERATION, &mut op))?;
            let ops = &**(op as *const *const FileOperationVtbl);
            check("SetOperationFlags", (ops.set_operation_flags)(op, FOF_NO_UI | FOFX_RECYCLEONDELETE | FOFX_ADDUNDORECORD))?;
            check("SHCreateItemFromParsingName", SHCreateItemFromParsingName(name.as_ptr(), null_mut(), &IID_ISHELL_ITEM, &mut item))?;
            check("DeleteItem", (ops.delete_item)(op, item, sink as *mut c_void))?;
            let performed = (ops.perform_operations)(op);
            if (*sink).refused.load(Ordering::SeqCst) {
                return Err(Failure::NotRecyclable);
            }
            check("PerformOperations", performed)?;
            let mut aborted = 0i32;
            check("GetAnyOperationsAborted", (ops.get_any_operations_aborted)(op, &mut aborted))?;
            if aborted != 0 {
                return Err(Failure::Aborted);
            }
            Ok(())
        })();
        if !item.is_null() {
            ((**(item as *const *const UnknownVtbl)).release)(item);
        }
        if !op.is_null() {
            ((**(op as *const *const UnknownVtbl)).release)(op);
        }
        // Only our own reference left: free it. Otherwise a few bytes stay
        // rather than risking Windows calling into freed memory.
        if (*sink).refs.load(Ordering::SeqCst) == 1 {
            drop(Box::from_raw(sink));
        }
        CoUninitialize();
        result
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
        // The full original path (on Unix, from the root), percent-encoded.
        let prefix = if cfg!(unix) { "[Trash Info]\nPath=/" } else { "[Trash Info]\nPath=" };
        assert!(info.starts_with(prefix), "{info}");
        assert!(info.contains("a%20b.flac\nDeletionDate="), "{info}");
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

    // The Recycle Bin turned off for the drive (as a person can in its
    // properties): Windows would delete the file for good, so it must stay.
    // It changes this user's setting for a moment, so it runs only on
    // request (CI runs it on Windows).
    #[cfg(windows)]
    #[test]
    #[ignore = "turns this user's Recycle Bin off for the drive for a moment"]
    fn windows_never_deletes_what_it_cannot_recycle() {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetVolumePathNameW(file: *const u16, volume: *mut u16, len: u32) -> i32;
            fn GetVolumeNameForVolumeMountPointW(mount: *const u16, name: *mut u16, len: u32) -> i32;
        }
        let d = tmp("nuke");
        let f = d.join("keep me.flac");
        std::fs::write(&f, b"audio").unwrap();
        let wide: Vec<u16> = f.as_os_str().encode_wide().chain([0]).collect();
        let (mut root, mut volume) = (vec![0u16; 1024], vec![0u16; 1024]);
        // SAFETY: NUL-terminated input, buffers of the stated length.
        unsafe {
            assert_ne!(GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), 1024), 0);
            assert_ne!(GetVolumeNameForVolumeMountPointW(root.as_ptr(), volume.as_mut_ptr(), 1024), 0);
        }
        let volume = String::from_utf16_lossy(&volume[..volume.iter().position(|&c| c == 0).unwrap()]);
        // "\\?\Volume{GUID}\" → "{GUID}"
        let guid = volume.trim_end_matches('\\').rsplit("Volume").next().unwrap().to_string();
        let key = format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{guid}");
        let reg = |args: &[&str]| std::process::Command::new("reg").args(args).status().unwrap().success();
        assert!(reg(&["add", &key, "/v", "NukeOnDelete", "/t", "REG_DWORD", "/d", "1", "/f"]));
        let result = move_to_trash(&f);
        reg(&["delete", &key, "/v", "NukeOnDelete", "/f"]);
        assert_eq!(std::fs::read(&f).ok().as_deref(), Some(&b"audio"[..]), "the file must stay where it was: {result:?}");
        // Refused by the progress sink, not by some other failure.
        assert!(result.as_ref().is_err_and(|e| e.contains("could not put it in the Recycle Bin")), "{result:?}");
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
