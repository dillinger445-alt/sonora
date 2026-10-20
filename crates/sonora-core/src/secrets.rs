//! Secrets SONORA keeps for the person (slskd's API key), held by the
//! operating system: the login keychain on macOS, Credential Manager on
//! Windows. Never in the library database, the logs or error messages.
//!
//! On macOS the item is saved so that any program running as the person
//! can read it without macOS asking first (what Keychain Access calls
//! "Allow all applications to access this item"). The keychain's usual
//! setting ties an item to the exact build that saved it, and a build
//! without a Developer ID signature counts as a new program every time it
//! is built: each update then asks "SONORA wants to use your confidential
//! information", and a missed or declined dialog makes the key look lost.
//! The key is in slskd.yml as plain text anyway, so this setting keeps it
//! exactly as private as slskd does.
//!
//! Elsewhere (Linux, which SONORA is only developed on) and when
//! `SONORA_SECRET_STORE=file` is set (tests), a file in the data folder
//! that only the person's account can read stands in.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where secrets are kept.
pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    /// Removing a secret that is not there is not an error.
    fn delete(&self, name: &str) -> Result<(), String>;
    /// What the interface calls the store.
    fn label(&self) -> &'static str;
}

/// The name items carry in the system's store.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
const SERVICE: &str = "SONORA";

/// The store for this computer.
pub fn system(data_dir: &Path) -> Arc<dyn SecretStore> {
    if std::env::var("SONORA_SECRET_STORE").is_ok_and(|v| v == "file") {
        return Arc::new(FileStore::new(data_dir));
    }
    #[cfg(target_os = "macos")]
    {
        Arc::new(keychain::Keychain { service: SERVICE.into() })
    }
    #[cfg(windows)]
    {
        Arc::new(credman::CredentialManager { service: SERVICE.into() })
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Arc::new(FileStore::new(data_dir))
    }
}

/// A file only the person's account can read (development and tests).
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(data_dir: &Path) -> FileStore {
        FileStore { path: data_dir.join("secrets.json") }
    }

    fn load(&self) -> Result<sonora_json::Value, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(t) => sonora_json::parse(&t).map_err(|_| "the secrets file is damaged".to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(sonora_json::Value::object()),
            Err(e) => Err(format!("the secrets file could not be read: {e}")),
        }
    }

    fn save(&self, v: &sonora_json::Value) -> Result<(), String> {
        let tmp = self.path.with_extension("json.tmp");
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut f = o.open(&tmp)?;
            f.write_all(v.to_string_compact().as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, &self.path)
        };
        write().map_err(|e| format!("the secrets file could not be written: {e}"))
    }
}

impl SecretStore for FileStore {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self.load()?.get(name).and_then(sonora_json::Value::as_str).map(str::to_string))
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        let mut v = self.load()?;
        v.set(name, value);
        self.save(&v)
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        let mut v = self.load()?;
        if let Some(m) = v.as_object_mut() {
            if m.remove(name).is_some() {
                return self.save(&v);
            }
        }
        Ok(())
    }

    fn label(&self) -> &'static str {
        "file"
    }
}

/// Bytes from the operating system's random number generator.
pub fn random_bytes(n: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; n];
    #[cfg(windows)]
    {
        #[link(name = "bcrypt")]
        extern "system" {
            fn BCryptGenRandom(algorithm: *mut std::ffi::c_void, buffer: *mut u8, length: u32, flags: u32) -> i32;
        }
        const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 2;
        // SAFETY: the buffer is valid for `n` bytes; no algorithm handle is needed with this flag.
        let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), buf.as_mut_ptr(), n as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
        if status != 0 {
            return Err(format!("the system could not provide random numbers ({status:#x})"));
        }
    }
    #[cfg(not(windows))]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).map_err(|e| format!("the system could not provide random numbers: {e}"))?;
    }
    Ok(buf)
}

/// A random key of letters and digits (`len` characters, about 5.95 bits each).
pub fn random_key(len: usize) -> Result<String, String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(len);
    while out.len() < len {
        for b in random_bytes(len * 2)? {
            // 248 = 4 × 62: keeping only bytes below it keeps every character equally likely.
            if b < 248 && out.len() < len {
                out.push(ALPHABET[(b % 62) as usize] as char);
            }
        }
    }
    Ok(out)
}

#[cfg(target_os = "macos")]
mod keychain {
    //! The login keychain, through the Security framework's item API.
    //!
    //! Items are added with an access object whose "decrypt" rule names no
    //! application, which the keychain reads as "any application": the
    //! same thing `security add-generic-password -A` and Keychain Access's
    //! "Allow all applications to access this item" do. Without it the rule
    //! names the build that saved the item, and every other build (an
    //! update, a development build, a build with a different signature)
    //! makes macOS ask before each read. See the module documentation.

    use std::ffi::c_void;
    use std::ptr;

    type CFTypeRef = *const c_void;
    type CFIndex = isize;
    type OSStatus = i32;
    /// `SecKeychainPromptSelector`: flags for the dialog an access rule
    /// shows, a 16-bit option set.
    type PromptSelector = u16;
    /// `kSecKeychainPromptRequirePassphase`: the dialog asks for the
    /// keychain's password instead of just confirming.
    const PROMPT_REQUIRE_PASSPHRASE: PromptSelector = 0x0001;

    /// Only the address of these structures is used.
    #[repr(C)]
    struct Opaque {
        _private: [u8; 0],
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: CFTypeRef;
        static kCFTypeDictionaryKeyCallBacks: Opaque;
        static kCFTypeDictionaryValueCallBacks: Opaque;
        fn CFDictionaryCreate(allocator: CFTypeRef, keys: *const CFTypeRef, values: *const CFTypeRef, count: CFIndex, key_callbacks: *const Opaque, value_callbacks: *const Opaque) -> CFTypeRef;
        fn CFStringCreateWithBytes(allocator: CFTypeRef, bytes: *const u8, count: CFIndex, encoding: u32, external: u8) -> CFTypeRef;
        fn CFDataCreate(allocator: CFTypeRef, bytes: *const u8, length: CFIndex) -> CFTypeRef;
        fn CFDataGetLength(data: CFTypeRef) -> CFIndex;
        fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
        fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFDataGetTypeID() -> usize;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecClass: CFTypeRef;
        static kSecClassGenericPassword: CFTypeRef;
        static kSecAttrService: CFTypeRef;
        static kSecAttrAccount: CFTypeRef;
        static kSecAttrLabel: CFTypeRef;
        static kSecAttrAccess: CFTypeRef;
        static kSecValueData: CFTypeRef;
        static kSecReturnData: CFTypeRef;
        static kSecMatchLimit: CFTypeRef;
        static kSecMatchLimitOne: CFTypeRef;
        static kSecACLAuthorizationDecrypt: CFTypeRef;
        fn SecItemAdd(attributes: CFTypeRef, result: *mut CFTypeRef) -> OSStatus;
        fn SecItemCopyMatching(query: CFTypeRef, result: *mut CFTypeRef) -> OSStatus;
        fn SecItemDelete(query: CFTypeRef) -> OSStatus;
        // The access (ACL) API of the file-based keychain. Deprecated since
        // macOS 10.10 in favour of the data-protection keychain, which an
        // app needs a provisioning profile to use; still the API behind
        // `security` and Keychain Access.
        fn SecAccessCreate(descriptor: CFTypeRef, trusted_list: CFTypeRef, access: *mut CFTypeRef) -> OSStatus;
        fn SecAccessCopyMatchingACLList(access: CFTypeRef, authorization_tag: CFTypeRef) -> CFTypeRef;
        fn SecACLCopyContents(acl: CFTypeRef, application_list: *mut CFTypeRef, description: *mut CFTypeRef, prompt_selector: *mut PromptSelector) -> OSStatus;
        fn SecACLSetContents(acl: CFTypeRef, application_list: CFTypeRef, description: CFTypeRef, prompt_selector: PromptSelector) -> OSStatus;
    }

    const UTF8: u32 = 0x0800_0100;
    const NOT_FOUND: OSStatus = -25300;

    /// A CoreFoundation object this code created, released when dropped.
    struct Owned(CFTypeRef);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the object was created (or copied) by this code and is released once.
                unsafe { CFRelease(self.0) }
            }
        }
    }

    fn owned(p: CFTypeRef) -> Result<Owned, String> {
        if p.is_null() {
            Err("the keychain request could not be prepared".into())
        } else {
            Ok(Owned(p))
        }
    }

    fn string(s: &str) -> Result<Owned, String> {
        // SAFETY: the bytes are valid UTF-8 for the given length.
        owned(unsafe { CFStringCreateWithBytes(ptr::null(), s.as_ptr(), s.len() as CFIndex, UTF8, 0) })
    }

    fn data(b: &[u8]) -> Result<Owned, String> {
        // SAFETY: the bytes are valid for the given length; CFDataCreate copies them.
        owned(unsafe { CFDataCreate(ptr::null(), b.as_ptr(), b.len() as CFIndex) })
    }

    fn dict(pairs: &[(CFTypeRef, CFTypeRef)]) -> Result<Owned, String> {
        let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
        let values: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
        // SAFETY: both arrays hold `pairs.len()` valid CoreFoundation objects; the
        // standard callbacks retain them for the dictionary's lifetime.
        owned(unsafe {
            CFDictionaryCreate(ptr::null(), keys.as_ptr(), values.as_ptr(), pairs.len() as CFIndex, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks)
        })
    }

    fn describe(status: OSStatus) -> String {
        match status {
            -25308 => "macOS did not let SONORA use the keychain right now (is it locked?)".into(),
            -128 => "access to the keychain was declined".into(),
            -25293 => "the keychain could not confirm access".into(),
            -25291 => "no keychain is available".into(),
            -25299 => "the keychain already has an item by this name".into(),
            s => format!("the keychain reported error {s}"),
        }
    }

    /// The access for a new item: its reading rules name no application,
    /// which the keychain takes as "any application", so no build of SONORA
    /// (or an update that is signed differently) makes macOS ask first.
    /// `None` when the keychain would not make one; the item then gets the
    /// keychain's default access, which names this build only.
    fn open_access(label: &Owned) -> Option<Owned> {
        let mut access: CFTypeRef = ptr::null();
        // SAFETY: the descriptor is a valid CFString; a null trusted list
        // stands for the calling application; `access` receives a +1 object.
        if unsafe { SecAccessCreate(label.0, ptr::null(), &mut access) } != 0 || access.is_null() {
            return None;
        }
        let access = Owned(access);
        // SAFETY: `access` is valid and the tag is a constant CFString; the
        // array (+1, or null) holds the rules that govern reading the secret.
        let rules = unsafe { SecAccessCopyMatchingACLList(access.0, kSecACLAuthorizationDecrypt) };
        if rules.is_null() {
            return None;
        }
        let rules = Owned(rules);
        // SAFETY: `rules` is a valid array; its elements are borrowed from it.
        let count = unsafe { CFArrayGetCount(rules.0) };
        for i in 0..count {
            // SAFETY: `i` is within the array.
            let rule = unsafe { CFArrayGetValueAtIndex(rules.0, i) };
            if rule.is_null() {
                return None;
            }
            let mut applications: CFTypeRef = ptr::null();
            let mut description: CFTypeRef = ptr::null();
            let mut prompt: PromptSelector = 0;
            // SAFETY: `rule` is a valid ACL object; the out-parameters receive
            // +1 objects (or null) and the rule's prompt flags.
            if unsafe { SecACLCopyContents(rule, &mut applications, &mut description, &mut prompt) } != 0 {
                return None;
            }
            let _applications = Owned(applications);
            let description = Owned(description);
            let description = if description.0.is_null() { label.0 } else { description.0 };
            // SAFETY: `rule` and `description` are valid; a null application
            // list means any application, and the dialog that could still be
            // shown is the plain confirmation, not the password one.
            if unsafe { SecACLSetContents(rule, ptr::null(), description, prompt & !PROMPT_REQUIRE_PASSPHRASE) } != 0 {
                return None;
            }
        }
        Some(access)
    }

    pub struct Keychain {
        pub service: String,
    }

    impl Keychain {
        /// The query naming one item: SONORA's generic password with this account name.
        fn with_item<T>(&self, name: &str, extra: &[(CFTypeRef, CFTypeRef)], f: impl FnOnce(&Owned) -> T) -> Result<T, String> {
            let service = string(&self.service)?;
            let account = string(name)?;
            // SAFETY: the Security constants are valid CFString objects for the life of the process.
            let mut pairs = unsafe { vec![(kSecClass, kSecClassGenericPassword), (kSecAttrService, service.0), (kSecAttrAccount, account.0)] };
            pairs.extend_from_slice(extra);
            let q = dict(&pairs)?;
            Ok(f(&q))
        }
    }

    impl super::SecretStore for Keychain {
        fn get(&self, name: &str) -> Result<Option<String>, String> {
            // SAFETY: constants as above.
            let extra = unsafe { [(kSecReturnData, kCFBooleanTrue), (kSecMatchLimit, kSecMatchLimitOne)] };
            let mut result: CFTypeRef = ptr::null();
            // SAFETY: the query is a valid dictionary; `result` receives a +1 object or stays null.
            let status = self.with_item(name, &extra, |q| unsafe { SecItemCopyMatching(q.0, &mut result) })?;
            let result = Owned(result);
            match status {
                0 => {
                    // SAFETY: `result` is a valid object; only CFData is read.
                    unsafe {
                        if result.0.is_null() || CFGetTypeID(result.0) != CFDataGetTypeID() {
                            return Err("the keychain item could not be read".into());
                        }
                        let len = CFDataGetLength(result.0).max(0) as usize;
                        let ptr = CFDataGetBytePtr(result.0);
                        if len == 0 || ptr.is_null() {
                            return Ok(Some(String::new()));
                        }
                        let bytes = std::slice::from_raw_parts(ptr, len);
                        String::from_utf8(bytes.to_vec()).map(Some).map_err(|_| "the keychain item could not be read".into())
                    }
                }
                NOT_FOUND => Ok(None),
                s => Err(describe(s)),
            }
        }

        /// Replaces the item rather than updating it: changing an item that
        /// another build saved makes macOS ask first, and only a new item
        /// gets the open access from [`open_access`]. Removing an item
        /// never asks.
        fn set(&self, name: &str, value: &str) -> Result<(), String> {
            super::SecretStore::delete(self, name)?;
            let secret = data(value.as_bytes())?;
            let label = string(&format!("{} ({})", self.service, name))?;
            let access = open_access(&label);
            // SAFETY: constants as above.
            let mut extra = unsafe { vec![(kSecValueData, secret.0), (kSecAttrLabel, label.0)] };
            if let Some(a) = &access {
                // SAFETY: constant as above.
                extra.push(unsafe { (kSecAttrAccess, a.0) });
            }
            // SAFETY: the attributes dictionary is valid, and `secret`, `label`
            // and `access` outlive the call; no result is requested.
            let status = self.with_item(name, &extra, |q| unsafe { SecItemAdd(q.0, ptr::null_mut()) })?;
            if status == 0 {
                Ok(())
            } else {
                Err(describe(status))
            }
        }

        fn delete(&self, name: &str) -> Result<(), String> {
            // SAFETY: the query is a valid dictionary.
            match self.with_item(name, &[], |q| unsafe { SecItemDelete(q.0) })? {
                0 | NOT_FOUND => Ok(()),
                s => Err(describe(s)),
            }
        }

        fn label(&self) -> &'static str {
            "keychain"
        }
    }
}

#[cfg(windows)]
mod credman {
    //! Windows Credential Manager (generic credentials of the signed-in user).

    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    /// CREDENTIALW, field for field.
    #[repr(C)]
    struct Credential {
        flags: u32,
        kind: u32,
        target_name: *mut u16,
        comment: *mut u16,
        last_written: FileTime,
        blob_size: u32,
        blob: *mut u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut c_void,
        target_alias: *mut u16,
        user_name: *mut u16,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn CredWriteW(credential: *const Credential, flags: u32) -> i32;
        fn CredReadW(target: *const u16, kind: u32, flags: u32, credential: *mut *mut Credential) -> i32;
        fn CredDeleteW(target: *const u16, kind: u32, flags: u32) -> i32;
        fn CredFree(buffer: *const c_void);
    }

    const CRED_TYPE_GENERIC: u32 = 1;
    const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;
    const ERROR_NOT_FOUND: i32 = 1168;

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    fn last_error() -> (i32, String) {
        let e = std::io::Error::last_os_error();
        (e.raw_os_error().unwrap_or(0), format!("Credential Manager reported an error: {e}"))
    }

    pub struct CredentialManager {
        pub service: String,
    }

    impl CredentialManager {
        fn target(&self, name: &str) -> Vec<u16> {
            wide(&format!("{}/{}", self.service, name))
        }
    }

    impl super::SecretStore for CredentialManager {
        fn get(&self, name: &str) -> Result<Option<String>, String> {
            let target = self.target(name);
            let mut cred: *mut Credential = ptr::null_mut();
            // SAFETY: `target` is NUL-terminated; on success `cred` points to memory freed below.
            if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) } == 0 {
                let (code, msg) = last_error();
                return if code == ERROR_NOT_FOUND { Ok(None) } else { Err(msg) };
            }
            // SAFETY: CredReadW succeeded, so `cred` is valid until CredFree.
            let value = unsafe {
                let c = &*cred;
                let bytes = if c.blob.is_null() { &[][..] } else { std::slice::from_raw_parts(c.blob, c.blob_size as usize) };
                let v = String::from_utf8(bytes.to_vec());
                CredFree(cred as *const c_void);
                v
            };
            value.map(Some).map_err(|_| "the saved credential could not be read".into())
        }

        fn set(&self, name: &str, value: &str) -> Result<(), String> {
            let mut target = self.target(name);
            let mut user = wide(&self.service);
            let mut blob = value.as_bytes().to_vec();
            let cred = Credential {
                flags: 0,
                kind: CRED_TYPE_GENERIC,
                target_name: target.as_mut_ptr(),
                comment: ptr::null_mut(),
                last_written: FileTime { low: 0, high: 0 },
                blob_size: blob.len() as u32,
                blob: blob.as_mut_ptr(),
                persist: CRED_PERSIST_LOCAL_MACHINE,
                attribute_count: 0,
                attributes: ptr::null_mut(),
                target_alias: ptr::null_mut(),
                user_name: user.as_mut_ptr(),
            };
            // SAFETY: every pointer in `cred` is valid for the duration of the call.
            if unsafe { CredWriteW(&cred, 0) } == 0 {
                return Err(last_error().1);
            }
            Ok(())
        }

        fn delete(&self, name: &str) -> Result<(), String> {
            let target = self.target(name);
            // SAFETY: `target` is NUL-terminated.
            if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
                let (code, msg) = last_error();
                if code != ERROR_NOT_FOUND {
                    return Err(msg);
                }
            }
            Ok(())
        }

        fn label(&self) -> &'static str {
            "credential-manager"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sonora-secrets-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_file_store_keeps_secrets_to_its_owner() {
        let dir = temp("file");
        let s = FileStore::new(&dir);
        assert_eq!(s.get("a").unwrap(), None);
        s.set("a", "one").unwrap();
        s.set("b", "two \"quoted\"").unwrap();
        assert_eq!(s.get("a").unwrap().as_deref(), Some("one"));
        assert_eq!(s.get("b").unwrap().as_deref(), Some("two \"quoted\""));
        s.set("a", "uno").unwrap();
        assert_eq!(s.get("a").unwrap().as_deref(), Some("uno"));
        s.delete("a").unwrap();
        s.delete("a").unwrap();
        assert_eq!(s.get("a").unwrap(), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("secrets.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "readable by others: {mode:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn random_keys_are_letters_and_digits() {
        let a = random_key(32).unwrap();
        let b = random_key(32).unwrap();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()), "{a}");
        assert_ne!(a, b);
    }

    /// The real store of this computer (the macOS keychain or Windows
    /// Credential Manager). Run in CI with `--ignored`.
    #[test]
    #[ignore]
    fn system_store_round_trip() {
        let dir = temp("system");
        std::env::remove_var("SONORA_SECRET_STORE");
        let s = system(&dir);
        let name = format!("ci-test-{}", std::process::id());
        let _ = s.delete(&name);
        assert_eq!(s.get(&name).unwrap(), None);
        s.set(&name, "first value").unwrap();
        assert_eq!(s.get(&name).unwrap().as_deref(), Some("first value"));
        // On macOS another program reads the item without a dialog (see the
        // module documentation). With the keychain's default access,
        // `security` would wait for the person to allow it instead.
        #[cfg(target_os = "macos")]
        assert_eq!(security_reads(&name).as_deref(), Some("first value"), "`security` could not read the item without a dialog");
        s.set(&name, "second value, ünïcode").unwrap();
        assert_eq!(s.get(&name).unwrap().as_deref(), Some("second value, ünïcode"));
        s.delete(&name).unwrap();
        assert_eq!(s.get(&name).unwrap(), None);
        s.delete(&name).unwrap();
        eprintln!("store: {}", s.label());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What `security find-generic-password -w` prints for SONORA's item,
    /// or `None` when it fails or waits (for a dialog) longer than 20 s.
    #[cfg(target_os = "macos")]
    fn security_reads(name: &str) -> Option<String> {
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};
        let mut child = Command::new("security")
            .args(["find-generic-password", "-s", SERVICE, "-a", name, "-w"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        let out = child.wait_with_output().ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim_end_matches(['\r', '\n']).to_string())
    }
}
