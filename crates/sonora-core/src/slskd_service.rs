//! slskd, set up and kept running by SONORA (macOS).
//!
//! The person clicks one button and confirms one screen that lists what
//! will happen. SONORA then downloads slskd's official release from GitHub,
//! checks it against the checksum pinned below, unpacks it in SONORA's data
//! folder, writes slskd's settings (the person's Soulseek account, their
//! download folder, a web page on this computer only, a random API key that
//! SONORA also keeps in the keychain), adds a launch agent so macOS starts
//! slskd at login and restarts it if it crashes, starts it and waits until it
//! has signed in to Soulseek.
//!
//! What SONORA writes: everything under `<SONORA's data>/slskd/`, the launch
//! agent `~/Library/LaunchAgents/app.sonora.analyzer.slskd.plist`, and the
//! download folder the person chose when it does not exist yet. slskd.yml is
//! copied to a dated backup before every change. Nothing is deleted unless
//! the person asks for it: the files one setup run made (after a failed run),
//! or the launch agent.
//!
//! The Soulseek password goes into slskd.yml only, a file only the person's
//! account can read: slskd reads its account from its settings, and a helper
//! that read the keychain at login could stall on a keychain prompt with no
//! one there to answer it. SONORA keeps no copy.
//!
//! slskd is not signed by Apple. SONORA checks the archive against the
//! checksum below before unpacking it, and never changes macOS's security
//! settings or a file's quarantine.

use crate::app::App;
use crate::logging;
use crate::sha256;
use crate::yaml::{self, quote};
use sonora_json::{obj, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// The slskd release SONORA sets up. A new SONORA release can move this
/// (with the new checksums); the person is then offered "Update slskd".
pub const VERSION: &str = "0.26.1";
/// The launch agent's name (SONORA's bundle identifier, then `.slskd`).
pub const LABEL: &str = "app.sonora.analyzer.slskd";
/// Where the web page and API of SONORA's slskd answer (this computer only).
pub const PORT: u16 = 5030;
/// The processes of the official Soulseek app.
pub const SOULSEEK_QT: &str = "SoulseekQt";

/// One release archive and what it must be.
#[derive(Clone, Debug, PartialEq)]
pub struct Build {
    pub runtime: String,
    pub file: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// The archives of [`VERSION`] for each Mac, with their size and SHA-256
/// (from github.com/slskd/slskd/releases).
pub fn pinned(runtime: &str) -> Option<Build> {
    let (size, sha) = match runtime {
        "osx-x64" => (60_607_605, "ccc278d659a658787a575cc40a0be9a19511e0578ba457a4c0675db987e4e8e7"),
        "osx-arm64" => (58_333_710, "5cf79a4d41ad40906aababc6bd05585899f102e7c544e38157df747f01cb6f4b"),
        _ => return None,
    };
    let file = format!("slskd-{VERSION}-{runtime}.zip");
    Some(Build { url: format!("https://github.com/slskd/slskd/releases/download/{VERSION}/{file}"), runtime: runtime.to_string(), file, size, sha256: sha.to_string() })
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ---------------------------------------------------------------- the system

/// What launchd says about the launch agent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentState {
    pub loaded: bool,
    pub pid: Option<u32>,
    pub last_exit: Option<i32>,
    /// The signal that ended its last run (9 when the system killed it).
    pub last_signal: Option<i32>,
}

/// A running slskd process.
#[derive(Clone, Debug, PartialEq)]
pub struct Process {
    pub pid: u32,
    /// The program's path.
    pub path: String,
}

/// Everything SONORA asks of the operating system to install and run slskd.
pub trait System: Send + Sync {
    /// Whether SONORA sets up slskd here (macOS).
    fn supported(&self) -> bool;
    fn home(&self) -> Option<PathBuf>;
    /// The archive for this computer's processor.
    fn build(&self) -> Option<Build>;
    /// Downloads `url` to `dest`, reporting the bytes so far.
    fn fetch(&self, url: &str, dest: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<(), String>;
    /// Unpacks a zip archive into `dest` (a new folder).
    fn unpack(&self, zip: &Path, dest: &Path) -> Result<(), String>;
    /// Where launch agents live (`~/Library/LaunchAgents`).
    fn agents_dir(&self) -> Option<PathBuf>;
    /// Loads the launch agent (`command` is what it runs).
    fn load(&self, plist: &Path, command: &[String]) -> Result<(), String>;
    /// Stops slskd and unloads the launch agent.
    fn unload(&self) -> Result<(), String>;
    /// Starts slskd (again, when `restart`).
    fn kickstart(&self, restart: bool) -> Result<(), String>;
    fn agent(&self) -> AgentState;
    /// The file carries macOS's mark for files downloaded from the internet
    /// (which Gatekeeper checks before it lets a program run).
    fn quarantined(&self, path: &Path) -> bool;
    /// Every running slskd.
    fn processes(&self) -> Vec<Process>;
    fn app_running(&self, name: &str) -> bool;
    fn quit_app(&self, name: &str) -> Result<(), String>;
    fn terminate(&self, pid: u32) -> Result<(), String>;
}

/// Runs a program and returns what it printed, or what went wrong.
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).output().map_err(|e| format!("{program} could not be run: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        let err = if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err.to_string() };
        Err(format!("{} ({})", err.chars().take(300).collect::<String>(), out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())))
    }
}

/// curl's exit codes, in words.
fn curl_failure(code: Option<i32>, stderr: &str) -> String {
    match code {
        Some(6) => "the computer could not find github.com. Is it connected to the internet?".into(),
        Some(7) => "the computer could not connect to github.com".into(),
        Some(22) => "GitHub did not hand over the file".into(),
        Some(28) => "the download took too long".into(),
        Some(35) | Some(51) | Some(58) | Some(60) | Some(77) | Some(83) | Some(90) | Some(91) => "the secure connection to github.com could not be checked".into(),
        Some(23) => "the file could not be written (is the disk full?)".into(),
        _ => {
            let t = stderr.trim();
            if t.is_empty() {
                "the download failed".into()
            } else {
                format!("the download failed: {}", t.chars().take(200).collect::<String>())
            }
        }
    }
}

/// Downloads with the system's curl, over https only.
pub fn curl_fetch(curl: &str, url: &str, dest: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
    let mut child = Command::new(curl)
        .args(["-fsSL", "--proto", "=https", "--proto-redir", "=https", "--tlsv1.2", "--retry", "2", "--connect-timeout", "20", "-o"])
        .arg(dest)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl could not be run: {e}"))?;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            progress(std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0));
            if status.success() {
                return Ok(());
            }
            let mut err = String::new();
            if let Some(mut s) = child.stderr.take() {
                use std::io::Read;
                let _ = s.read_to_string(&mut err);
            }
            return Err(curl_failure(status.code(), &err));
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".into());
        }
        progress(std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0));
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(unix)]
fn uid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { getuid() }
}

#[cfg(not(unix))]
fn uid() -> u32 {
    0
}

/// launchd's view of a service (`launchctl print`).
pub fn parse_launchctl_print(text: &str) -> AgentState {
    let mut s = AgentState { loaded: true, ..AgentState::default() };
    for line in text.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("pid = ") {
            s.pid = v.trim().parse().ok();
        } else if let Some(v) = t.strip_prefix("last exit code = ") {
            // "0", "78: EX_CONFIG" or "(never exited)".
            s.last_exit = v.split(':').next().and_then(|n| n.trim().parse().ok());
        } else if let Some(v) = t.strip_prefix("last terminating signal = ") {
            // "Killed: 9".
            s.last_signal = v.rsplit(':').next().and_then(|n| n.trim().parse().ok());
        }
    }
    s
}

/// The real thing: launchd, ps, curl and ditto.
pub struct MacSystem;

impl System for MacSystem {
    fn supported(&self) -> bool {
        cfg!(target_os = "macos")
    }

    fn home(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
    }

    fn build(&self) -> Option<Build> {
        // The processor itself: an Intel build of SONORA may run translated on Apple silicon.
        let arm = run("/usr/sbin/sysctl", &["-n", "hw.optional.arm64"]).map(|o| o.trim() == "1").unwrap_or(cfg!(target_arch = "aarch64"));
        pinned(if arm { "osx-arm64" } else { "osx-x64" })
    }

    fn fetch(&self, url: &str, dest: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
        curl_fetch("/usr/bin/curl", url, dest, cancel, progress)
    }

    fn unpack(&self, zip: &Path, dest: &Path) -> Result<(), String> {
        let (z, d) = (zip.to_string_lossy().into_owned(), dest.to_string_lossy().into_owned());
        run("/usr/bin/ditto", &["-x", "-k", &z, &d]).map(|_| ()).map_err(|e| format!("the archive could not be unpacked: {e}"))
    }

    fn agents_dir(&self) -> Option<PathBuf> {
        self.home().map(|h| h.join("Library").join("LaunchAgents"))
    }

    fn load(&self, plist: &Path, _command: &[String]) -> Result<(), String> {
        let domain = format!("gui/{}", uid());
        let p = plist.to_string_lossy().into_owned();
        run("/bin/launchctl", &["bootstrap", &domain, &p]).map(|_| ()).map_err(|e| format!("macOS did not load slskd's launch agent: {e}"))
    }

    fn unload(&self) -> Result<(), String> {
        let target = format!("gui/{}/{LABEL}", uid());
        match run("/bin/launchctl", &["bootout", &target]) {
            Ok(_) => Ok(()),
            // Not loaded: nothing to stop.
            Err(_) if !self.agent().loaded => Ok(()),
            Err(e) => Err(format!("macOS did not stop slskd: {e}")),
        }
    }

    fn kickstart(&self, restart: bool) -> Result<(), String> {
        let target = format!("gui/{}/{LABEL}", uid());
        let mut args = vec!["kickstart"];
        if restart {
            args.push("-k");
        }
        args.push(&target);
        run("/bin/launchctl", &args).map(|_| ()).map_err(|e| format!("macOS did not start slskd: {e}"))
    }

    fn agent(&self) -> AgentState {
        let target = format!("gui/{}/{LABEL}", uid());
        match run("/bin/launchctl", &["print", &target]) {
            Ok(text) => parse_launchctl_print(&text),
            Err(_) => AgentState::default(),
        }
    }

    fn quarantined(&self, path: &Path) -> bool {
        run("/usr/bin/xattr", &["-p", "com.apple.quarantine", &path.to_string_lossy()]).is_ok()
    }

    fn processes(&self) -> Vec<Process> {
        let Ok(text) = run("/bin/ps", &["-axww", "-o", "pid=,comm="]) else { return Vec::new() };
        text.lines()
            .filter_map(|l| {
                let t = l.trim_start();
                let (pid, path) = t.split_once(char::is_whitespace)?;
                let path = path.trim();
                let name = Path::new(path).file_name()?.to_string_lossy().into_owned();
                if name != "slskd" {
                    return None;
                }
                Some(Process { pid: pid.parse().ok()?, path: path.to_string() })
            })
            .collect()
    }

    fn app_running(&self, name: &str) -> bool {
        run("/usr/bin/pgrep", &["-x", name]).is_ok()
    }

    fn quit_app(&self, name: &str) -> Result<(), String> {
        let script = format!("tell application \"{}\" to quit", name.replace('"', ""));
        run("/usr/bin/osascript", &["-e", &script]).map(|_| ())
    }

    fn terminate(&self, pid: u32) -> Result<(), String> {
        run("/bin/kill", &["-TERM", &pid.to_string()]).map(|_| ())
    }
}

/// A stand-in for tests and the browser bridge's end-to-end tests
/// (`SONORA_SLSKD_SYSTEM=test`): downloads come from a local archive, and
/// the launch agent is only noted. With `spawn` (`SONORA_SLSKD_SYSTEM=spawn`,
/// for developing on Linux) loading the agent runs slskd as a child process.
pub struct TestSystem {
    pub home: PathBuf,
    pub agents: PathBuf,
    pub archive: Option<PathBuf>,
    pub build: Option<Build>,
    pub spawn: bool,
    pub state: Mutex<TestState>,
}

#[derive(Default)]
pub struct TestState {
    pub loaded: bool,
    pub running: bool,
    pub last_exit: Option<i32>,
    pub calls: Vec<String>,
    pub processes: Vec<Process>,
    pub soulseek_qt: bool,
    pub fetch_error: Option<String>,
    /// The installed program carries the quarantine mark, and "macOS"
    /// kills it whenever it starts.
    pub quarantined: bool,
    last_signal: Option<i32>,
    command: Vec<String>,
    child: Option<std::process::Child>,
}

impl TestSystem {
    pub fn new(home: &Path, agents: &Path, archive: Option<&Path>) -> TestSystem {
        let build = archive.and_then(|a| {
            let size = std::fs::metadata(a).ok()?.len();
            let sha = sha256::file_hex(a).ok()?;
            let file = a.file_name()?.to_string_lossy().into_owned();
            Some(Build { runtime: "test".into(), url: format!("file://{}", a.display()), file, size, sha256: sha })
        });
        TestSystem { home: home.to_path_buf(), agents: agents.to_path_buf(), archive: archive.map(Path::to_path_buf), build, spawn: false, state: Mutex::new(TestState::default()) }
    }

    /// From `SONORA_SLSKD_TEST_ARCHIVE` (the "release" to install), with the
    /// launch agents in `<data>/test-agents`.
    pub fn from_env(data_dir: &Path, spawn: bool) -> TestSystem {
        let archive = std::env::var_os("SONORA_SLSKD_TEST_ARCHIVE").map(PathBuf::from);
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| data_dir.to_path_buf());
        let mut s = TestSystem::new(&home, &data_dir.join("test-agents"), archive.as_deref());
        s.spawn = spawn;
        s
    }

    fn note(&self, call: String) {
        lock(&self.state).calls.push(call);
    }

    /// A quarantined program is killed as it starts (as macOS may do).
    fn blocked(&self) -> bool {
        let mut st = lock(&self.state);
        if st.quarantined {
            st.running = false;
            st.last_signal = Some(9);
        }
        st.quarantined
    }

    fn start_child(&self) -> Result<(), String> {
        let mut st = lock(&self.state);
        if let Some(c) = st.child.as_mut() {
            if c.try_wait().ok().flatten().is_none() {
                return Ok(());
            }
        }
        let (program, args) = st.command.split_first().ok_or("no command")?;
        let log = args.windows(2).find(|w| w[0] == "--app-dir").map(|w| PathBuf::from(&w[1]).join("logs").join("service.log"));
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null());
        if let Some(l) = log.and_then(|l| {
            std::fs::create_dir_all(l.parent()?).ok()?;
            std::fs::OpenOptions::new().create(true).append(true).open(l).ok()
        }) {
            cmd.stderr(l);
        }
        let child = cmd.spawn().map_err(|e| e.to_string())?;
        st.child = Some(child);
        Ok(())
    }

    fn stop_child(&self) {
        if let Some(mut c) = lock(&self.state).child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for TestSystem {
    fn drop(&mut self) {
        self.stop_child();
    }
}

impl System for TestSystem {
    fn supported(&self) -> bool {
        true
    }

    fn home(&self) -> Option<PathBuf> {
        Some(self.home.clone())
    }

    fn build(&self) -> Option<Build> {
        self.build.clone()
    }

    fn fetch(&self, url: &str, dest: &Path, _cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
        self.note(format!("fetch {url}"));
        if let Some(e) = lock(&self.state).fetch_error.take() {
            return Err(e);
        }
        let src = self.archive.as_ref().ok_or("no archive to download")?;
        let n = std::fs::copy(src, dest).map_err(|e| e.to_string())?;
        progress(n);
        Ok(())
    }

    fn unpack(&self, zip: &Path, dest: &Path) -> Result<(), String> {
        self.note("unpack".into());
        std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
        let (z, d) = (zip.to_string_lossy().into_owned(), dest.to_string_lossy().into_owned());
        run("unzip", &["-q", "-o", &z, "-d", &d]).map(|_| ())
    }

    fn agents_dir(&self) -> Option<PathBuf> {
        Some(self.agents.clone())
    }

    fn load(&self, plist: &Path, command: &[String]) -> Result<(), String> {
        self.note(format!("load {}", plist.display()));
        {
            let mut st = lock(&self.state);
            st.loaded = true;
            st.command = command.to_vec();
        }
        let run_at_load = std::fs::read_to_string(plist).map(|t| t.contains("<key>RunAtLoad</key>\n\t<true/>")).unwrap_or(false);
        if run_at_load {
            if self.blocked() {
                return Ok(());
            }
            lock(&self.state).running = true;
            if self.spawn {
                self.start_child()?;
            }
        }
        Ok(())
    }

    fn unload(&self) -> Result<(), String> {
        self.note("unload".into());
        {
            let mut st = lock(&self.state);
            st.loaded = false;
            st.running = false;
        }
        self.stop_child();
        Ok(())
    }

    fn kickstart(&self, restart: bool) -> Result<(), String> {
        self.note(if restart { "restart".into() } else { "kickstart".into() });
        if !lock(&self.state).loaded {
            return Err("not loaded".into());
        }
        if self.blocked() {
            return Ok(());
        }
        if self.spawn {
            if restart {
                self.stop_child();
            }
            self.start_child()?;
        }
        lock(&self.state).running = true;
        Ok(())
    }

    fn agent(&self) -> AgentState {
        if self.spawn {
            let mut st = lock(&self.state);
            if let Some(c) = st.child.as_mut() {
                if let Ok(Some(status)) = c.try_wait() {
                    st.last_exit = status.code();
                    st.child = None;
                    st.running = false;
                }
            }
            let pid = st.child.as_ref().map(std::process::Child::id);
            return AgentState { loaded: st.loaded, pid, last_exit: st.last_exit, last_signal: st.last_signal };
        }
        let st = lock(&self.state);
        AgentState { loaded: st.loaded, pid: if st.running { Some(4242) } else { None }, last_exit: st.last_exit, last_signal: st.last_signal }
    }

    fn quarantined(&self, _path: &Path) -> bool {
        lock(&self.state).quarantined
    }

    fn processes(&self) -> Vec<Process> {
        let st = lock(&self.state);
        let mut v = st.processes.clone();
        if st.running && !self.spawn {
            v.push(Process { pid: 4242, path: "managed".into() });
        }
        if let Some(c) = st.child.as_ref() {
            v.push(Process { pid: c.id(), path: "managed".into() });
        }
        v
    }

    fn app_running(&self, name: &str) -> bool {
        name == SOULSEEK_QT && lock(&self.state).soulseek_qt
    }

    fn quit_app(&self, name: &str) -> Result<(), String> {
        self.note(format!("quit {name}"));
        lock(&self.state).soulseek_qt = false;
        Ok(())
    }

    fn terminate(&self, pid: u32) -> Result<(), String> {
        self.note(format!("terminate {pid}"));
        lock(&self.state).processes.retain(|p| p.pid != pid);
        Ok(())
    }
}

/// The system SONORA uses: the real one, or the stand-in the environment asks for.
pub fn system_for(data_dir: &Path) -> Arc<dyn System> {
    match std::env::var("SONORA_SLSKD_SYSTEM").as_deref() {
        Ok("test") => Arc::new(TestSystem::from_env(data_dir, false)),
        Ok("spawn") => Arc::new(TestSystem::from_env(data_dir, true)),
        _ => Arc::new(MacSystem),
    }
}

// --------------------------------------------------------------- the layout

/// Where SONORA's slskd lives.
#[derive(Clone, Debug)]
pub struct Layout {
    pub root: PathBuf,
    pub data: PathBuf,
    pub config: PathBuf,
    pub logs: PathBuf,
    pub incomplete: PathBuf,
    pub archives: PathBuf,
    pub agents: Option<PathBuf>,
}

impl Layout {
    pub fn new(data_dir: &Path, agents: Option<PathBuf>) -> Layout {
        let root = data_dir.join("slskd");
        let data = root.join("data");
        Layout {
            config: data.join("slskd.yml"),
            logs: data.join("logs"),
            incomplete: data.join("incomplete"),
            archives: root.join("downloads"),
            data,
            root,
            agents,
        }
    }

    pub fn app(&self, version: &str) -> PathBuf {
        self.root.join(format!("app-{version}"))
    }

    pub fn binary(&self, version: &str) -> PathBuf {
        self.app(version).join("slskd")
    }

    pub fn plist(&self) -> Option<PathBuf> {
        self.agents.as_ref().map(|a| a.join(format!("{LABEL}.plist")))
    }

    pub fn service_log(&self) -> PathBuf {
        self.logs.join("service.log")
    }

    /// The versions unpacked so far, newest first.
    pub fn installed(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.root)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().to_str()?.strip_prefix("app-").map(str::to_string))
                    .filter(|ver| self.binary(ver).is_file())
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|ver| std::cmp::Reverse(version_key(ver)));
        v
    }

    /// The command the launch agent runs.
    pub fn command(&self, version: &str) -> Vec<String> {
        vec![self.binary(version).to_string_lossy().into_owned(), "--app-dir".into(), self.data.to_string_lossy().into_owned(), "--no-logo".into()]
    }
}

fn version_key(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The launch agent. `background`: macOS starts slskd at login and starts
/// it again if it stops unexpectedly; otherwise SONORA starts it while it
/// is open. Its output is discarded (slskd keeps its own log files); errors
/// go to `logs/service.log`.
pub fn plist(layout: &Layout, version: &str, background: bool) -> String {
    let args: String = layout.command(version).iter().map(|a| format!("\t\t<string>{}</string>\n", xml(a))).collect();
    let keep_alive = if background { "<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>" } else { "<false/>" };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<!-- slskd for SONORA. SONORA writes this file; turn slskd off in SONORA (Soulseek, Setup) rather than editing it. -->\n\
<plist version=\"1.0\">\n<dict>\n\
\t<key>Label</key>\n\t<string>{label}</string>\n\
\t<key>ProgramArguments</key>\n\t<array>\n{args}\t</array>\n\
\t<key>WorkingDirectory</key>\n\t<string>{wd}</string>\n\
\t<key>RunAtLoad</key>\n\t<{run}/>\n\
\t<key>KeepAlive</key>\n\t{keep_alive}\n\
\t<key>ThrottleInterval</key>\n\t<integer>30</integer>\n\
\t<key>ProcessType</key>\n\t<string>Standard</string>\n\
\t<key>StandardOutPath</key>\n\t<string>/dev/null</string>\n\
\t<key>StandardErrorPath</key>\n\t<string>{err}</string>\n\
</dict>\n</plist>\n",
        label = LABEL,
        wd = xml(&layout.app(version).to_string_lossy()),
        run = if background { "true" } else { "false" },
        err = xml(&layout.service_log().to_string_lossy()),
    )
}

// ----------------------------------------------------------- slskd.yml

/// What SONORA writes into slskd.yml.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub username: String,
    pub password: String,
    pub downloads: String,
    pub incomplete: String,
    pub shares: Vec<String>,
    pub port: u16,
    pub web_password: String,
    pub api_key: String,
}

/// SONORA's slskd.yml set up again: the account, the folders, the web page
/// on this computer only and SONORA's key are written into the file as it
/// is, and everything else in it stays. The shared folders change only when
/// the person changed the one the setup showed (`old_shares` is what the
/// file has; the setup shows the first). `None` when the file lacks a line
/// SONORA needs: it is then written anew (after a backup).
pub fn reconfigure(text: &str, s: &Settings, old_shares: &[String], share: Option<&Path>) -> Option<String> {
    let mut t = text.to_string();
    let port = s.port.to_string();
    let values: [(&[&str], String); 9] = [
        (&["soulseek", "username"], quote(&s.username)),
        (&["soulseek", "password"], quote(&s.password)),
        (&["directories", "downloads"], quote(&s.downloads)),
        (&["directories", "incomplete"], quote(&s.incomplete)),
        (&["web", "port"], port),
        (&["web", "ip_address"], "127.0.0.1".into()),
        (&["web", "https", "disabled"], "true".into()),
        (&["web", "authentication", "password"], quote(&s.web_password)),
        (&["web", "authentication", "api_keys", "sonora", "key"], quote(&s.api_key)),
    ];
    for (path, value) in values.iter() {
        t = yaml::replace_scalar(&t, path, value)?;
    }
    let shares: Vec<String> = match share {
        None => Vec::new(),
        Some(p) => {
            let p = p.to_string_lossy().into_owned();
            if old_shares.contains(&p) {
                old_shares.to_vec()
            } else {
                let mut v = old_shares.to_vec();
                if v.is_empty() {
                    v.push(p);
                } else {
                    v[0] = p;
                }
                v
            }
        }
    };
    if shares != old_shares {
        t = yaml::replace_list(&t, &["shares", "directories"], &shares)?;
    }
    Some(t)
}

/// slskd.yml as SONORA writes it.
pub fn render(s: &Settings) -> String {
    let mut out = String::new();
    out.push_str("# slskd's settings, written by SONORA. SONORA backs this file up before it\n");
    out.push_str("# changes it. Only your account can read it: it holds your Soulseek password.\n");
    out.push_str("soulseek:\n");
    out.push_str(&format!("  username: {}\n  password: {}\n", quote(&s.username), quote(&s.password)));
    out.push_str("directories:\n");
    out.push_str(&format!("  downloads: {}\n  incomplete: {}\n", quote(&s.downloads), quote(&s.incomplete)));
    out.push_str("shares:\n");
    if s.shares.is_empty() {
        out.push_str("  directories: []\n");
    } else {
        out.push_str("  directories:\n");
        for d in &s.shares {
            out.push_str(&format!("    - {}\n", quote(d)));
        }
    }
    out.push_str("web:\n");
    out.push_str(&format!("  port: {}\n  ip_address: 127.0.0.1\n  https:\n    disabled: true\n", s.port));
    out.push_str("  authentication:\n    username: slskd\n");
    out.push_str(&format!("    password: {}\n", quote(&s.web_password)));
    out.push_str("    api_keys:\n      sonora:\n");
    out.push_str(&format!("        key: {}\n        role: readwrite\n", quote(&s.api_key)));
    out.push_str("retention:\n  search: 1440\n");
    out.push_str("flags:\n  no_version_check: true\n");
    out
}

/// What SONORA reuses from an existing slskd.yml.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub path: PathBuf,
    pub username: Option<String>,
    pub password: Option<String>,
    pub downloads: Option<String>,
    pub shares: Vec<String>,
    pub web_password: Option<String>,
    pub api_key: Option<String>,
}

pub fn read_found(path: &Path) -> Option<Found> {
    let text = std::fs::read_to_string(path).ok()?;
    let n = yaml::parse(&text);
    let s = |p: &[&str]| n.str_at(p).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    Some(Found {
        path: path.to_path_buf(),
        username: s(&["soulseek", "username"]),
        password: s(&["soulseek", "password"]),
        downloads: s(&["directories", "downloads"]),
        shares: n.strings_at(&["shares", "directories"]).into_iter().filter(|d| !d.trim().is_empty()).collect(),
        web_password: s(&["web", "authentication", "password"]),
        api_key: s(&["web", "authentication", "api_keys", "sonora", "key"]),
    })
}

/// A file only the person's account can read and write, replaced in one step.
pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
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
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    };
    let r = write();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// `YYYYMMDD-HHMMSS` (UTC) for backup names.
fn stamp(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Copies `path` to `<name>.<stamp>.bak` next to it (never over an older backup).
pub fn backup(path: &Path) -> std::io::Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "slskd.yml".into());
    let base = format!("{name}.{}", stamp(SystemTime::now()));
    let mut dest = path.with_file_name(format!("{base}.bak"));
    let mut n = 2;
    while dest.exists() {
        dest = path.with_file_name(format!("{base}-{n}.bak"));
        n += 1;
    }
    std::fs::copy(path, &dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600));
    }
    Ok(Some(dest))
}

/// Changes one value in slskd.yml (after a backup).
pub fn change_value(config: &Path, path: &[&str], value: &str) -> Result<bool, String> {
    let text = std::fs::read_to_string(config).map_err(|e| format!("slskd.yml could not be read: {e}"))?;
    let Some(changed) = yaml::replace_scalar(&text, path, &quote(value)) else { return Ok(false) };
    if changed == text {
        return Ok(true);
    }
    backup(config).map_err(|e| format!("slskd.yml could not be backed up: {e}"))?;
    write_private(config, &changed).map_err(|e| format!("slskd.yml could not be written: {e}"))?;
    Ok(true)
}

// ------------------------------------------------------- slskd's log

/// Why slskd is not signed in, from its log.
#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    /// No user name or password in slskd.yml.
    NoAccount,
    WrongPassword,
    /// Another program signed in with the same account (often the Soulseek app).
    SignedInElsewhere,
    /// The Soulseek server could not be reached.
    Network(String),
}

impl Reason {
    pub fn code(&self) -> &'static str {
        match self {
            Reason::NoAccount => "no_account",
            Reason::WrongPassword => "wrong_password",
            Reason::SignedInElsewhere => "signed_in_elsewhere",
            Reason::Network(_) => "network",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Reason::NoAccount => "slskd has no Soulseek user name or password.".into(),
            Reason::WrongPassword => "Soulseek did not accept the user name or password.".into(),
            Reason::SignedInElsewhere => "The same Soulseek account signed in somewhere else (the Soulseek app, say), so Soulseek signed slskd out.".into(),
            Reason::Network(m) => format!("slskd could not reach the Soulseek server ({m})."),
        }
    }
}

/// The latest word on signing in, in slskd's log text (the last line that
/// says something about it wins; a successful sign-in clears the rest).
pub fn reason_in(log: &str) -> Option<Reason> {
    let mut found: Option<Reason> = None;
    for line in log.lines() {
        if line.contains("Logged in to the Soulseek server as") || line.contains("Connected to the Soulseek server") {
            found = None;
        } else if line.contains("username and/or password invalid") {
            found = Some(Reason::NoAccount);
        } else if line.contains("invalid username or password") {
            found = Some(Reason::WrongPassword);
        } else if line.contains("another client logged in using the same username") {
            found = Some(Reason::SignedInElsewhere);
        } else if let Some(i) = line.find("Failed to reconnect: ").or_else(|| line.find("Reconnect attempt failed: ")) {
            let msg = line[i..].split_once(": ").map(|x| x.1).unwrap_or("").trim();
            found = Some(Reason::Network(msg.chars().take(160).collect()));
        } else if let Some(i) = line.find("Disconnected from the Soulseek server: ") {
            let msg = line[i + "Disconnected from the Soulseek server: ".len()..].trim();
            if !msg.starts_with("the client is shutting down") && !msg.starts_with("disconnected by the user") {
                found = Some(Reason::Network(msg.chars().take(160).collect()));
            }
        }
    }
    found
}

/// slskd's newest log file (`slskd-YYYYMMDD.log`).
pub fn newest_log(logs: &Path) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(logs)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("slskd-") && n.ends_with(".log")))
        .collect();
    v.sort();
    v.pop()
}

/// The end of a text file (at most `max` bytes), from `from` on.
fn tail(path: &Path, from: u64, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = from.max(len.saturating_sub(max));
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = f.take(max).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

// -------------------------------------------------------------- the setup

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Download,
    Verify,
    Install,
    Configure,
    Start,
    SignIn,
}

pub const STEPS: [Step; 6] = [Step::Download, Step::Verify, Step::Install, Step::Configure, Step::Start, Step::SignIn];

impl Step {
    pub fn id(self) -> &'static str {
        match self {
            Step::Download => "download",
            Step::Verify => "verify",
            Step::Install => "install",
            Step::Configure => "configure",
            Step::Start => "start",
            Step::SignIn => "signin",
        }
    }
}

/// What the person chose in the setup.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub username: String,
    /// `None`: keep the password of the slskd.yml in `adopt`.
    pub password: Option<String>,
    pub downloads: PathBuf,
    pub share: Option<PathBuf>,
    /// The slskd.yml settings are taken over from.
    pub adopt: Option<PathBuf>,
}

impl Request {
    /// Checks what came from the interface (the password itself is never echoed).
    pub fn from_json(p: &Value) -> Result<Request, String> {
        let text = |k: &str| p.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let username = text("username").ok_or("Enter your Soulseek user name.")?;
        if username.chars().count() > 64 || username.chars().any(char::is_control) {
            return Err("That Soulseek user name cannot be right: it is too long or holds control characters.".into());
        }
        let password = match p.get("password").and_then(Value::as_str) {
            Some(pw) if !pw.is_empty() => {
                if pw.chars().count() > 256 || pw.chars().any(char::is_control) {
                    return Err("That password holds characters slskd cannot use (line breaks or control characters).".into());
                }
                Some(pw.to_string())
            }
            _ => None,
        };
        let path = |k: &str| -> Result<Option<PathBuf>, String> {
            match text(k) {
                None => Ok(None),
                Some(s) => {
                    let p = PathBuf::from(&s);
                    if !p.is_absolute() {
                        return Err(format!("{s} is not a full folder path."));
                    }
                    Ok(Some(p))
                }
            }
        };
        let downloads = path("downloads")?.ok_or("Choose the folder for finished downloads.")?;
        let share = path("share")?;
        if let Some(s) = &share {
            if !s.is_dir() {
                return Err(format!("The folder to share, {}, does not exist.", s.display()));
            }
        }
        let adopt = path("adopt")?;
        if password.is_none() && adopt.is_none() {
            return Err("Enter your Soulseek password.".into());
        }
        Ok(Request { username, password, downloads, share, adopt })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunError {
    pub step: Step,
    pub message: String,
    /// What would help: "retry", "password", "stop_other", "quit_soulseek_qt", "network", "folder".
    pub fix: &'static str,
}

#[derive(Default)]
struct Run {
    active: bool,
    done: Vec<Step>,
    current: Option<Step>,
    error: Option<RunError>,
    finished: bool,
    downloaded: u64,
    total: u64,
    /// Files and folders this run made (for "remove what the setup made").
    created: Vec<PathBuf>,
    request: Option<Request>,
    signed_in_as: Option<String>,
    started_at: Option<i64>,
    /// slskd's log and its length when the Start step began: what slskd
    /// says from there on is about this setup.
    log_mark: Option<(PathBuf, u64)>,
}

/// SONORA's slskd: its setup, its launch agent and its health.
pub struct Service {
    system: Mutex<Arc<dyn System>>,
    run: Mutex<Run>,
    cancel: AtomicBool,
    /// SONORA is quitting: the watcher stops.
    quitting: AtomicBool,
    /// Held while SONORA changes the launch agent (writes, loads, starts,
    /// stops or removes it), so no two of these cross: the watcher never
    /// starts slskd again in the middle of Stop, Turn off or SONORA quitting.
    agent_turn: Mutex<()>,
}

impl Default for Service {
    fn default() -> Self {
        Service {
            system: Mutex::new(Arc::new(MacSystem)),
            run: Mutex::new(Run::default()),
            cancel: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
            agent_turn: Mutex::new(()),
        }
    }
}

/// How often SONORA looks at its slskd while it runs.
const WATCH_EVERY: Duration = Duration::from_secs(20);

/// Whether the computer slept between two looks: the wall clock moved on
/// further than the time that passed while it was awake (`Instant` does
/// not count sleep). `wall` is None when the clock went back.
pub fn slept(awake: Duration, wall: Option<Duration>) -> bool {
    wall.is_some_and(|w| w > awake + Duration::from_secs(30))
}

/// What installing the launch agent did.
struct Agent {
    /// The plist file did not exist before.
    new: bool,
    /// launchd loaded it now (and, in the background mode, started slskd).
    loaded_now: bool,
}

type Fail = (String, &'static str);

fn fail<T>(message: impl Into<String>, fix: &'static str) -> Result<T, Fail> {
    Err((message.into(), fix))
}

impl Service {
    pub fn with_system(system: Arc<dyn System>) -> Service {
        Service { system: Mutex::new(system), ..Service::default() }
    }

    pub fn set_system(&self, s: Arc<dyn System>) {
        *lock(&self.system) = s;
    }

    pub fn system(&self) -> Arc<dyn System> {
        Arc::clone(&lock(&self.system))
    }

    pub fn layout(&self, app: &App) -> Layout {
        Layout::new(&app.config.data_dir, self.system().agents_dir())
    }

    /// The person stopped slskd: SONORA does not start it again until they
    /// start it (and launchd does not at login either).
    pub fn is_stopped(&self, app: &App) -> bool {
        app.settings().slskd_stopped
    }

    fn set_stopped(&self, app: &App, on: bool) -> Result<(), String> {
        let mut s = app.settings();
        if s.slskd_stopped == on {
            return Ok(());
        }
        s.slskd_stopped = on;
        s.save(&app.db.write()).map_err(|e| e.to_string())?;
        *app.scans.shared.settings.write().unwrap_or_else(|e| e.into_inner()) = s;
        Ok(())
    }

    /// SONORA set up this slskd (its settings file is there).
    pub fn configured(&self, app: &App) -> bool {
        self.system().supported() && self.layout(app).config.is_file()
    }

    /// The version the launch agent runs (the newest unpacked).
    pub fn installed_version(&self, app: &App) -> Option<String> {
        self.layout(app).installed().into_iter().next()
    }

    /// Settings files that can be taken over: SONORA's own, the one its
    /// setup script made on the Desktop, and slskd's default one.
    pub fn candidates(&self, app: &App) -> Vec<PathBuf> {
        let mut v = vec![self.layout(app).config];
        if let Some(home) = self.system().home() {
            v.push(home.join("Desktop").join("slskd").join("data").join("slskd.yml"));
            v.push(home.join("Library").join("Application Support").join("slskd").join("slskd.yml"));
            v.push(home.join(".local").join("share").join("slskd").join("slskd.yml"));
        }
        v
    }

    fn run_json(&self) -> Value {
        let r = lock(&self.run);
        if !r.active && r.done.is_empty() && r.error.is_none() && !r.finished {
            return Value::Null;
        }
        let steps: Vec<Value> = STEPS
            .iter()
            .map(|s| {
                let state = if r.done.contains(s) {
                    "done"
                } else if r.error.as_ref().is_some_and(|e| e.step == *s) {
                    "failed"
                } else if r.current == Some(*s) && r.active {
                    "running"
                } else {
                    "waiting"
                };
                obj! { "id" => s.id(), "state" => state }
            })
            .collect();
        obj! {
            "active" => r.active,
            "finished" => r.finished,
            "steps" => Value::Array(steps),
            "downloaded" => r.downloaded as i64,
            "total" => r.total as i64,
            "error" => r.error.as_ref().map(|e| obj! { "step" => e.step.id(), "message" => e.message.clone(), "fix" => e.fix }).unwrap_or(Value::Null),
            "created" => Value::Array(r.created.iter().map(|p| Value::String(p.to_string_lossy().into_owned())).collect()),
            "passwordKept" => r.request.as_ref().is_some_and(|q| q.password.is_some() || q.adopt.is_some()),
            "signedInAs" => r.signed_in_as.clone(),
            "startedAt" => r.started_at,
        }
    }

    fn emit(&self, app: &App) {
        app.events.emit("slskd.setup", &self.run_json());
    }

    /// slskd processes other than the one launchd runs for SONORA (one
    /// started by hand, even from SONORA's folder, reads other settings).
    pub fn others(&self, _app: &App) -> Vec<Process> {
        let sys = self.system();
        let own = sys.agent().pid;
        sys.processes().into_iter().filter(|p| Some(p.pid) != own).collect()
    }

    /// What the Setup page shows about SONORA's slskd (nothing is asked of slskd itself).
    pub fn status(&self, app: &App) -> Value {
        let sys = self.system();
        let layout = self.layout(app);
        let supported = sys.supported();
        let configured = supported && layout.config.is_file();
        let agent = if supported { sys.agent() } else { AgentState::default() };
        let installed = if supported { self.installed_version(app) } else { None };
        let found = if configured { read_found(&layout.config) } else { None };
        let reason = if configured { newest_log(&layout.logs).and_then(|l| reason_in(&tail(&l, 0, 256 * 1024))) } else { None };
        let key = app.slskd.key(app).ok().flatten();
        let key_problem = match (&found, &key) {
            (Some(f), Some(k)) if configured => match &f.api_key {
                None => Some("missing"),
                Some(fk) if fk != k => Some("different"),
                _ => None,
            },
            (Some(_), None) if configured => Some("not_saved"),
            _ => None,
        };
        obj! {
            "supported" => supported,
            "version" => VERSION,
            "installed" => installed.clone(),
            "updateAvailable" => installed.as_deref().is_some_and(|v| version_key(v) < version_key(VERSION)),
            "configured" => configured,
            "folder" => layout.root.to_string_lossy().into_owned(),
            "config" => configured.then(|| layout.config.to_string_lossy().into_owned()),
            "log" => if configured { newest_log(&layout.logs).or_else(|| Some(layout.logs.clone())).map(|p| p.to_string_lossy().into_owned()) } else { None },
            "background" => app.settings().slskd_background,
            "stopped" => app.settings().slskd_stopped,
            "agent" => obj! {
                "installed" => layout.plist().is_some_and(|p| p.is_file()),
                "loaded" => agent.loaded,
                "running" => agent.pid.is_some(),
                "pid" => agent.pid.map(|p| p as i64),
                "lastExit" => agent.last_exit.map(|c| c as i64),
            },
            "username" => found.as_ref().and_then(|f| f.username.clone()),
            "downloads" => found.as_ref().and_then(|f| f.downloads.clone()),
            "shares" => Value::Array(found.as_ref().map(|f| f.shares.iter().map(|s| Value::String(s.clone())).collect()).unwrap_or_default()),
            "keyProblem" => key_problem,
            "reason" => reason.as_ref().map(|r| obj! { "code" => r.code(), "message" => r.message() }).unwrap_or(Value::Null),
            "others" => if supported { Value::Array(self.others(app).iter().map(|p| obj! { "pid" => p.pid as i64, "path" => p.path.clone() }).collect()) } else { Value::Array(Vec::new()) },
            "soulseekQt" => supported && sys.app_running(SOULSEEK_QT),
            "setup" => self.run_json(),
        }
    }

    /// What the setup screen offers: the download, and what can be taken
    /// over from settings already on this computer. Reads the Desktop
    /// folder (macOS may ask SONORA first).
    pub fn plan(&self, app: &App) -> Value {
        let sys = self.system();
        let layout = self.layout(app);
        let build = sys.build();
        let found = self.candidates(app).into_iter().filter_map(|p| read_found(&p)).find(|f| f.username.is_some());
        let home = sys.home().unwrap_or_default();
        let default_downloads = home.join("Music").join("Soulseek");
        let downloads = found.as_ref().and_then(|f| f.downloads.clone()).unwrap_or_else(|| default_downloads.to_string_lossy().into_owned());
        obj! {
            "supported" => sys.supported(),
            "version" => VERSION,
            "download" => build.as_ref().map(|b| obj! { "url" => b.url.clone(), "size" => b.size as i64, "runtime" => b.runtime.clone(), "sha256" => b.sha256.clone() }).unwrap_or(Value::Null),
            "folder" => layout.root.to_string_lossy().into_owned(),
            "agent" => layout.plist().map(|p| p.to_string_lossy().into_owned()),
            "found" => found.as_ref().map(|f| obj! {
                "path" => f.path.to_string_lossy().into_owned(),
                "username" => f.username.clone(),
                "hasPassword" => f.password.is_some(),
                "downloads" => f.downloads.clone(),
                "shares" => Value::Array(f.shares.iter().map(|s| Value::String(s.clone())).collect()),
            }).unwrap_or(Value::Null),
            "downloads" => downloads,
            "share" => found.as_ref().and_then(|f| f.shares.first().cloned()),
            "soulseekQt" => sys.supported() && sys.app_running(SOULSEEK_QT),
            "others" => Value::Array(self.others(app).iter().map(|p| obj! { "pid" => p.pid as i64, "path" => p.path.clone() }).collect()),
            "protected" => Value::Array(["Desktop", "Documents", "Downloads"].iter().map(|d| Value::String(home.join(d).to_string_lossy().into_owned())).collect()),
            "home" => home.to_string_lossy().into_owned(),
        }
    }

    /// Starts the setup in the background (`slskd.setup` events report it).
    pub fn start(self: &Arc<Self>, app: &Arc<App>, req: Request) -> Result<Value, String> {
        if !self.system().supported() {
            return Err("SONORA sets up slskd by itself on macOS only.".into());
        }
        if crate::sources::accepted_at(&app.db.read()).is_none() {
            return Err("Read and accept the notice on the Soulseek page first.".into());
        }
        let own = app.config.data_dir.canonicalize().unwrap_or_else(|_| app.config.data_dir.clone());
        let chosen = req.downloads.canonicalize().unwrap_or_else(|_| req.downloads.clone());
        if chosen.starts_with(&own) {
            return Err("Choose a download folder outside SONORA's own data folder.".into());
        }
        {
            let mut r = lock(&self.run);
            if r.active {
                return Err("The setup is already running.".into());
            }
            // What an earlier run that did not finish made can still be removed.
            let created = if r.finished { Vec::new() } else { std::mem::take(&mut r.created) };
            *r = Run { active: true, request: Some(req), started_at: Some(crate::db::now_ms()), created, ..Run::default() };
        }
        self.spawn(app);
        Ok(self.run_json())
    }

    /// Goes on from the step that failed. A new password may come with it.
    pub fn retry(self: &Arc<Self>, app: &Arc<App>, password: Option<String>) -> Result<Value, String> {
        {
            let mut r = lock(&self.run);
            if r.active {
                return Err("The setup is already running.".into());
            }
            if r.request.is_none() {
                return Err("Start the setup again: SONORA was restarted since.".into());
            }
            if let Some(pw) = password.filter(|p| !p.is_empty()) {
                if pw.chars().any(char::is_control) {
                    return Err("That password holds characters slskd cannot use.".into());
                }
                if let Some(req) = r.request.as_mut() {
                    req.password = Some(pw);
                }
                // A new password goes into slskd.yml, and slskd starts again with it.
                r.done.retain(|s| !matches!(s, Step::Configure | Step::Start | Step::SignIn));
            }
            r.error = None;
            r.active = true;
            r.finished = false;
        }
        self.spawn(app);
        Ok(self.run_json())
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Removes the files and folders the last run made (asked for by the person).
    pub fn discard(&self, app: &App) -> Result<Value, String> {
        let created = {
            let mut r = lock(&self.run);
            if r.active {
                return Err("Wait for the setup to stop first.".into());
            }
            std::mem::take(&mut r.created)
        };
        let layout = self.layout(app);
        let _turn = lock(&self.agent_turn);
        if created.iter().any(|p| Some(p) == layout.plist().as_ref()) {
            let _ = self.system().unload();
        }
        let mut removed = Vec::new();
        for p in created.iter().rev() {
            // SONORA's own folders go with what is in them; any other folder
            // (the download folder it made) only while it is still empty.
            let gone = if p.is_dir() {
                if p.starts_with(&layout.root) {
                    std::fs::remove_dir_all(p)
                } else {
                    std::fs::remove_dir(p)
                }
            } else {
                std::fs::remove_file(p)
            };
            if gone.is_ok() {
                removed.push(Value::String(p.to_string_lossy().into_owned()));
            }
        }
        *lock(&self.run) = Run::default();
        logging::log_fields("info", "slskd", "removed what a setup run made", obj! { "count" => removed.len() as i64 });
        self.emit(app);
        Ok(obj! { "removed" => Value::Array(removed) })
    }

    fn spawn(self: &Arc<Self>, app: &Arc<App>) {
        self.cancel.store(false, Ordering::Relaxed);
        self.emit(app);
        let me = Arc::clone(self);
        let app = Arc::clone(app);
        let _ = std::thread::Builder::new().name("sonora-slskd-setup".into()).spawn(move || me.work(&app));
    }

    fn work(&self, app: &Arc<App>) {
        for step in STEPS {
            if lock(&self.run).done.contains(&step) {
                continue;
            }
            {
                let mut r = lock(&self.run);
                r.current = Some(step);
            }
            self.emit(app);
            let outcome = if self.cancel.load(Ordering::Relaxed) { fail("The setup was stopped.", "retry") } else { self.step(app, step) };
            match outcome {
                Ok(()) => {
                    lock(&self.run).done.push(step);
                }
                Err((message, fix)) => {
                    logging::log_fields("warn", "slskd", "setup step failed", obj! { "step" => step.id(), "fix" => fix });
                    let mut r = lock(&self.run);
                    r.error = Some(RunError { step, message, fix });
                    r.active = false;
                    r.current = None;
                    drop(r);
                    self.emit(app);
                    return;
                }
            }
        }
        {
            let mut r = lock(&self.run);
            r.active = false;
            r.finished = true;
            r.current = None;
            // The password is not needed any more.
            if let Some(q) = r.request.as_mut() {
                q.password = None;
            }
        }
        logging::log_fields("info", "slskd", "slskd set up", obj! { "version" => VERSION });
        self.emit(app);
        app.sources.wake();
    }

    fn created(&self, p: PathBuf) {
        let mut r = lock(&self.run);
        if !r.created.contains(&p) {
            r.created.push(p);
        }
    }

    fn step(&self, app: &Arc<App>, step: Step) -> Result<(), Fail> {
        let sys = self.system();
        let layout = self.layout(app);
        let Some(build) = sys.build() else { return fail("There is no slskd for this computer's processor.", "retry") };
        let archive = layout.archives.join(&build.file);
        match step {
            Step::Download => {
                if std::fs::metadata(&archive).is_ok_and(|m| m.len() == build.size) {
                    return Ok(());
                }
                if !layout.root.exists() {
                    std::fs::create_dir_all(&layout.archives).map_err(|e| (format!("SONORA could not make its slskd folder: {e}"), "retry"))?;
                    self.created(layout.root.clone());
                }
                std::fs::create_dir_all(&layout.archives).map_err(|e| (format!("SONORA could not make its slskd folder: {e}"), "retry"))?;
                let part = archive.with_extension("zip.part");
                let _ = std::fs::remove_file(&part);
                {
                    let mut r = lock(&self.run);
                    r.total = build.size;
                    r.downloaded = 0;
                }
                let mut last = Instant::now();
                let mut progress = |n: u64| {
                    lock(&self.run).downloaded = n;
                    if last.elapsed() > Duration::from_millis(400) {
                        last = Instant::now();
                        self.emit(app);
                    }
                };
                if let Err(e) = sys.fetch(&build.url, &part, &self.cancel, &mut progress) {
                    let _ = std::fs::remove_file(&part);
                    return fail(if e == "cancelled" { "The setup was stopped.".to_string() } else { format!("slskd could not be downloaded: {e}.") }, "network");
                }
                std::fs::rename(&part, &archive).map_err(|e| (format!("The download could not be kept: {e}"), "retry"))?;
                self.created(archive);
                Ok(())
            }
            Step::Verify => {
                let size = std::fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
                let sum = sha256::file_hex(&archive).unwrap_or_default();
                if size != build.size || !sum.eq_ignore_ascii_case(&build.sha256) {
                    // SONORA's own download: removed, so the next try fetches it again.
                    let _ = std::fs::remove_file(&archive);
                    lock(&self.run).done.retain(|s| *s != Step::Download);
                    return fail(
                        "The download is not the slskd release SONORA expects (its checksum differs), so SONORA did not use it. Try again; if it happens again, the release may have changed on GitHub.",
                        "retry",
                    );
                }
                Ok(())
            }
            Step::Install => {
                let app_dir = layout.app(VERSION);
                if layout.binary(VERSION).is_file() {
                    return Ok(());
                }
                let tmp = layout.root.join(format!(".app-{VERSION}.unpacking"));
                if tmp.exists() {
                    let _ = std::fs::remove_dir_all(&tmp);
                }
                std::fs::create_dir_all(&tmp).map_err(|e| (format!("SONORA could not make a folder for slskd: {e}"), "retry"))?;
                if let Err(e) = sys.unpack(&archive, &tmp) {
                    let _ = std::fs::remove_dir_all(&tmp);
                    return fail(format!("slskd could not be unpacked: {e}"), "retry");
                }
                let bin = tmp.join("slskd");
                if !bin.is_file() {
                    let _ = std::fs::remove_dir_all(&tmp);
                    return fail("The archive did not hold slskd.", "retry");
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755));
                }
                std::fs::rename(&tmp, &app_dir).map_err(|e| (format!("slskd could not be put in place: {e}"), "retry"))?;
                self.created(app_dir);
                Ok(())
            }
            Step::Configure => self.configure(app, &layout),
            Step::Start => self.start_agent(app, &layout),
            Step::SignIn => self.wait_signed_in(app, &layout),
        }
    }

    fn configure(&self, app: &Arc<App>, layout: &Layout) -> Result<(), Fail> {
        let req = lock(&self.run).request.clone().ok_or(("Start the setup again.".to_string(), "retry"))?;
        let adopted = req.adopt.as_deref().and_then(read_found);
        let own = read_found(&layout.config);
        let password = match req.password.clone().or_else(|| adopted.as_ref().and_then(|f| f.password.clone())) {
            Some(p) => p,
            None => return fail("Enter your Soulseek password.", "password"),
        };
        // The key SONORA already has, when it looks right; otherwise a new one.
        let saved = app.slskd.key(app).ok().flatten().filter(|k| crate::slskd::check_key(k).is_ok());
        let api_key = match saved {
            Some(k) => k,
            None => {
                let k = crate::secrets::random_key(32).map_err(|e| (format!("SONORA could not make a key: {e}"), "retry"))?;
                app.slskd.set_key(app, Some(&k)).map_err(|e| (e.message, "retry"))?;
                k
            }
        };
        let web_password = match own.as_ref().and_then(|f| f.web_password.clone()) {
            Some(p) => p,
            None => crate::secrets::random_key(20).map_err(|e| (format!("SONORA could not make a password: {e}"), "retry"))?,
        };
        if !req.downloads.exists() {
            std::fs::create_dir_all(&req.downloads).map_err(|e| (format!("The download folder {} could not be made: {e}", req.downloads.display()), "folder"))?;
            self.created(req.downloads.clone());
        } else if !req.downloads.is_dir() {
            return fail(format!("{} is not a folder.", req.downloads.display()), "folder");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::create_dir_all(&layout.data);
            let _ = std::fs::set_permissions(&layout.data, std::fs::Permissions::from_mode(0o700));
        }
        let settings = Settings {
            username: req.username.clone(),
            password,
            downloads: req.downloads.to_string_lossy().into_owned(),
            incomplete: layout.incomplete.to_string_lossy().into_owned(),
            shares: req.share.iter().map(|s| s.to_string_lossy().into_owned()).collect(),
            port: test_port().unwrap_or(PORT),
            web_password,
            api_key,
        };
        let existed = layout.config.is_file();
        // Set up again (or updated): only what SONORA needs changes, and the
        // rest of the person's slskd.yml stays as it is.
        let text = match std::fs::read_to_string(&layout.config).ok().and_then(|old| reconfigure(&old, &settings, own.as_ref().map(|f| f.shares.as_slice()).unwrap_or(&[]), req.share.as_deref())) {
            Some(t) => t,
            None => render(&settings),
        };
        backup(&layout.config).map_err(|e| (format!("slskd.yml could not be backed up: {e}"), "retry"))?;
        write_private(&layout.config, &text).map_err(|e| (format!("slskd's settings could not be written: {e}"), "retry"))?;
        if !existed {
            self.created(layout.config.clone());
        }
        // SONORA talks to its own slskd, and watches the folder it saves to.
        let mut s = app.settings();
        s.slskd_url = format!("http://127.0.0.1:{}", settings.port);
        if !existed {
            // The first setup runs slskd in the background; later ones keep the person's choice.
            s.slskd_background = true;
        }
        let folder = settings.downloads.clone();
        if s.soulseek_folder.as_deref() != Some(folder.as_str()) {
            crate::sources::note_existing(app, &req.downloads).map_err(|e| (format!("The download folder could not be read: {e}"), "folder"))?;
            s.soulseek_folder = Some(folder);
        }
        s.save(&app.db.write()).map_err(|e| (format!("SONORA's settings could not be saved: {e}"), "retry"))?;
        *app.scans.shared.settings.write().unwrap_or_else(|e| e.into_inner()) = s;
        app.sources.wake();
        Ok(())
    }

    /// Writes the launch agent (when it is new or different) and loads it.
    fn install_agent(&self, layout: &Layout, version: &str, background: bool) -> Result<Agent, String> {
        let plist_path = layout.plist().ok_or("There is no folder for launch agents.")?;
        let text = plist(layout, version, background);
        let old = std::fs::read_to_string(&plist_path).ok();
        let changed = old.as_deref() != Some(text.as_str());
        let sys = self.system();
        // launchd opens the error log before it starts slskd, and does not
        // start it when the log's folder is missing.
        std::fs::create_dir_all(&layout.logs).map_err(|e| format!("{} could not be made: {e}", layout.logs.display()))?;
        if changed {
            if old.is_some() && sys.agent().loaded {
                sys.unload()?;
            }
            if let Some(dir) = plist_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{} could not be made: {e}", dir.display()))?;
            }
            let tmp = plist_path.with_extension("plist.tmp");
            std::fs::write(&tmp, &text).and_then(|_| std::fs::rename(&tmp, &plist_path)).map_err(|e| format!("The launch agent could not be written: {e}"))?;
        }
        let loaded_now = !sys.agent().loaded;
        if loaded_now {
            sys.load(&plist_path, &layout.command(version))?;
        }
        Ok(Agent { new: old.is_none(), loaded_now })
    }

    fn start_agent(&self, app: &Arc<App>, layout: &Layout) -> Result<(), Fail> {
        let sys = self.system();
        let others = self.others(app);
        if !others.is_empty() {
            let list: Vec<String> = others.iter().map(|p| format!("{} (process {})", p.path, p.pid)).collect();
            return fail(
                format!("Another slskd is running ({}). Only one can use slskd's ports, so SONORA's slskd cannot start while it runs.", list.join(", ")),
                "stop_other",
            );
        }
        let version = self.installed_version(app).unwrap_or_else(|| VERSION.to_string());
        let err_len = std::fs::metadata(layout.service_log()).map(|m| m.len()).unwrap_or(0);
        lock(&self.run).log_mark = newest_log(&layout.logs).and_then(|l| std::fs::metadata(&l).ok().map(|m| (l, m.len())));
        {
            let _turn = lock(&self.agent_turn);
            self.set_stopped(app, false).map_err(|e| (e, "retry"))?;
            let agent = self.install_agent(layout, &version, app.settings().slskd_background).map_err(|e| (e, "retry"))?;
            if agent.new {
                if let Some(p) = layout.plist() {
                    self.created(p);
                }
            }
            if !agent.loaded_now {
                // It was running already: start it again so it reads the new slskd.yml.
                sys.kickstart(true).map_err(|e| (e, "retry"))?;
            } else if sys.agent().pid.is_none() {
                let _ = sys.kickstart(false);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(wait_secs(90));
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return fail("The setup was stopped.", "retry");
            }
            match app.slskd.test(app) {
                Ok(_) => return Ok(()),
                Err(e) if e.code == "slskd_unauthorized" => {
                    return fail("slskd answers but did not accept SONORA's key. Another slskd may be using its address: stop it and try again.", "stop_other");
                }
                Err(_) => {}
            }
            let agent = sys.agent();
            if agent.loaded && agent.pid.is_none() && (agent.last_exit.is_some_and(|c| c != 0) || agent.last_signal.is_some()) {
                if sys.quarantined(&layout.binary(&version)) {
                    // SONORA's own download carries no such mark; this one came from elsewhere.
                    return fail(
                        "macOS stopped slskd: it is marked as downloaded from the internet, and Apple does not know its developer. In System Settings, open Privacy & Security and choose Open Anyway for slskd, then try again. SONORA does not change macOS's security settings itself.",
                        "gatekeeper",
                    );
                }
                let errors = tail(&layout.service_log(), err_len, 4096);
                let last: Vec<&str> = errors.lines().filter(|l| !l.trim().is_empty()).rev().take(3).collect();
                let detail = last.into_iter().rev().collect::<Vec<_>>().join(" ");
                return fail(
                    if detail.is_empty() { "slskd stopped right after it started.".to_string() } else { format!("slskd stopped right after it started: {}", detail.chars().take(400).collect::<String>()) },
                    "retry",
                );
            }
            if Instant::now() > deadline {
                return fail("slskd did not answer within a minute and a half of starting.", "retry");
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn wait_signed_in(&self, app: &Arc<App>, layout: &Layout) -> Result<(), Fail> {
        let deadline = Instant::now() + Duration::from_secs(wait_secs(60));
        let log_start = lock(&self.run).log_mark.clone();
        // slskd signs in by itself after it starts, but not again after
        // Soulseek signed it out (the account signed in elsewhere): ask.
        if let Ok(v) = app.slskd.test(app) {
            if v.get("loggedIn").and_then(Value::as_bool) != Some(true) {
                let _ = app.slskd.connect(app);
            }
        }
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return fail("The setup was stopped.", "retry");
            }
            if let Ok(v) = app.slskd.test(app) {
                if v.get("loggedIn").and_then(Value::as_bool) == Some(true) {
                    lock(&self.run).signed_in_as = v.get("username").and_then(Value::as_str).map(str::to_string);
                    return Ok(());
                }
            }
            // What slskd wrote since the setup started it.
            let log = match (&log_start, newest_log(&layout.logs)) {
                (Some((path, len)), Some(now)) if *path == now => tail(&now, *len, 256 * 1024),
                (_, Some(now)) => tail(&now, 0, 256 * 1024),
                _ => String::new(),
            };
            match reason_in(&log) {
                Some(Reason::WrongPassword) | Some(Reason::NoAccount) => {
                    return fail("Soulseek did not accept the user name or password. Check them (they are the ones you use in the Soulseek app) and try again.", "password")
                }
                Some(Reason::SignedInElsewhere) => {
                    let qt = self.system().app_running(SOULSEEK_QT);
                    return fail(
                        if qt { "The Soulseek app is signed in with the same account, so Soulseek signed slskd out. Quit the Soulseek app, then try again." } else { "Another program signed in with the same Soulseek account, so Soulseek signed slskd out. Quit it, then try again." },
                        if qt { "quit_soulseek_qt" } else { "retry" },
                    );
                }
                _ => {}
            }
            if Instant::now() > deadline {
                let why = reason_in(&log).map(|r| r.message()).unwrap_or_else(|| "It may still be connecting.".into());
                return fail(format!("slskd runs, but it has not signed in to Soulseek yet. {why}"), "network");
            }
            std::thread::sleep(Duration::from_millis(700));
        }
    }

    // ------------------------------------------------------------ running

    /// Makes sure SONORA's slskd runs (when it was set up here): loads the
    /// launch agent if it is not, starts slskd if it stopped. Quick; never
    /// asks the person anything. slskd stays off when the person stopped it,
    /// or turned it off (its launch agent is gone).
    pub fn ensure(&self, app: &App) -> Result<(), String> {
        let _turn = lock(&self.agent_turn);
        if !self.layout(app).plist().is_some_and(|p| p.is_file()) {
            return Ok(());
        }
        self.run_agent(app)
    }

    /// Installs (or updates) the launch agent and starts slskd if it is not
    /// running. The caller holds `agent_turn`.
    fn run_agent(&self, app: &App) -> Result<(), String> {
        if self.quitting.load(Ordering::Relaxed) || !self.configured(app) || lock(&self.run).active || self.is_stopped(app) {
            return Ok(());
        }
        let layout = self.layout(app);
        let Some(version) = self.installed_version(app) else { return Ok(()) };
        let background = app.settings().slskd_background;
        let sys = self.system();
        if !self.others(app).is_empty() {
            // Another slskd holds the ports; the Setup page offers to stop it.
            return Ok(());
        }
        self.install_agent(&layout, &version, background)?;
        let agent = sys.agent();
        if agent.pid.is_none() {
            sys.kickstart(false)?;
            logging::log_fields("info", "slskd", "started slskd", obj! { "background" => background });
        }
        Ok(())
    }

    /// When SONORA quits: slskd keeps running only in the background mode.
    pub fn on_quit(&self, app: &App) {
        self.quitting.store(true, Ordering::Relaxed);
        let _turn = lock(&self.agent_turn);
        if self.configured(app) && !app.settings().slskd_background && self.system().agent().pid.is_some() {
            let _ = self.system().unload();
        }
    }

    pub fn action(&self, app: &App, what: &str) -> Result<(), String> {
        if !self.configured(app) {
            return Err("slskd was not set up by SONORA.".into());
        }
        let sys = self.system();
        let _turn = lock(&self.agent_turn);
        match what {
            "start" => {
                self.set_stopped(app, false)?;
                self.run_agent(app)
            }
            "stop" => {
                // It stays stopped: the launch agent no longer starts it at
                // login (it is kept, so Start brings it back as it was).
                self.set_stopped(app, true)?;
                if let Some(version) = self.installed_version(app) {
                    let layout = self.layout(app);
                    if layout.plist().is_some_and(|p| p.is_file()) {
                        self.install_agent(&layout, &version, false)?;
                    }
                }
                sys.unload()
            }
            "restart" => {
                self.set_stopped(app, false)?;
                if !sys.agent().loaded {
                    return self.run_agent(app);
                }
                let Some(version) = self.installed_version(app) else { return sys.kickstart(true) };
                // Back to how it starts (a stopped agent does not start at login).
                let agent = self.install_agent(&self.layout(app), &version, app.settings().slskd_background)?;
                if agent.loaded_now {
                    // Written anew and loaded again: launchd started it, or SONORA does.
                    if sys.agent().pid.is_none() {
                        sys.kickstart(false)?;
                    }
                    Ok(())
                } else {
                    sys.kickstart(true)
                }
            }
            _ => Err(format!("unknown action {what}")),
        }
    }

    /// Runs slskd in the background (from login on) or only while SONORA is open.
    pub fn set_background(&self, app: &App, on: bool) -> Result<(), String> {
        let mut s = app.settings();
        s.slskd_background = on;
        s.save(&app.db.write()).map_err(|e| e.to_string())?;
        *app.scans.shared.settings.write().unwrap_or_else(|e| e.into_inner()) = s;
        // Turned off (no launch agent) or stopped: it stays so; the choice
        // applies when it is started.
        let _turn = lock(&self.agent_turn);
        let layout = self.layout(app);
        if self.configured(app) && !self.is_stopped(app) && layout.plist().is_some_and(|p| p.is_file()) {
            if let Some(version) = self.installed_version(app) {
                self.install_agent(&layout, &version, on)?;
                self.run_agent(app)?;
            }
        }
        Ok(())
    }

    /// Turns the background service off and removes the launch agent. The
    /// slskd folder, its settings and the downloads stay.
    pub fn remove_agent(&self, app: &App) -> Result<Value, String> {
        let _turn = lock(&self.agent_turn);
        let layout = self.layout(app);
        self.system().unload()?;
        if let Some(p) = layout.plist() {
            if p.is_file() {
                std::fs::remove_file(&p).map_err(|e| format!("{} could not be removed: {e}", p.display()))?;
            }
        }
        logging::log("info", "slskd", "removed slskd's launch agent");
        Ok(obj! { "kept" => layout.root.to_string_lossy().into_owned() })
    }

    /// The key in slskd.yml differs from the one SONORA keeps: when slskd.yml
    /// has a usable key, SONORA takes it (no file changes). True when it did.
    pub fn adopt_key(&self, app: &App) -> bool {
        // Only when SONORA talks to its own slskd: the key of another one
        // (on a NAS, say) is never replaced with this one's.
        if !self.configured(app) || app.settings().slskd_url.trim_end_matches('/') != managed_url() {
            return false;
        }
        let Some(found) = read_found(&self.layout(app).config) else { return false };
        let Some(k) = found.api_key.filter(|k| crate::slskd::check_key(k).is_ok()) else { return false };
        if app.slskd.key(app).ok().flatten().as_deref() == Some(k.as_str()) {
            return false;
        }
        let ok = app.slskd.set_key(app, Some(&k)).is_ok();
        if ok {
            logging::log("info", "slskd", "took over the API key from slskd.yml");
        }
        ok
    }

    /// Puts SONORA's key into slskd.yml (after a backup) and restarts slskd.
    pub fn repair(&self, app: &App) -> Result<(), String> {
        if !self.configured(app) {
            return Err("slskd was not set up by SONORA.".into());
        }
        let layout = self.layout(app);
        let key = match app.slskd.key(app).ok().flatten().filter(|k| crate::slskd::check_key(k).is_ok()) {
            Some(k) => k,
            None => {
                let k = crate::secrets::random_key(32)?;
                app.slskd.set_key(app, Some(&k)).map_err(|e| e.message)?;
                k
            }
        };
        if !change_value(&layout.config, &["web", "authentication", "api_keys", "sonora", "key"], &key)? {
            // No key line at all: write the settings again from what is there.
            let f = read_found(&layout.config).ok_or("slskd.yml could not be read.")?;
            let settings = Settings {
                username: f.username.unwrap_or_default(),
                password: f.password.unwrap_or_default(),
                downloads: f.downloads.unwrap_or_else(|| app.settings().soulseek_folder.unwrap_or_default()),
                incomplete: layout.incomplete.to_string_lossy().into_owned(),
                shares: f.shares,
                port: test_port().unwrap_or(PORT),
                web_password: f.web_password.map(Ok).unwrap_or_else(|| crate::secrets::random_key(20))?,
                api_key: key,
            };
            backup(&layout.config).map_err(|e| e.to_string())?;
            write_private(&layout.config, &render(&settings)).map_err(|e| e.to_string())?;
        }
        logging::log("info", "slskd", "put SONORA's key into slskd.yml");
        self.action(app, "restart")
    }

    /// A new Soulseek password: into slskd.yml (after a backup), then slskd starts again.
    pub fn change_password(&self, app: &App, password: &str) -> Result<(), String> {
        if !self.configured(app) {
            return Err("slskd was not set up by SONORA.".into());
        }
        if password.is_empty() || password.chars().any(char::is_control) {
            return Err("Enter the password (no line breaks).".into());
        }
        let layout = self.layout(app);
        if !change_value(&layout.config, &["soulseek", "password"], password)? {
            return Err("slskd.yml has no Soulseek password line. Run the setup again.".into());
        }
        logging::log("info", "slskd", "changed the Soulseek password in slskd.yml");
        self.action(app, "restart")
    }

    /// Stops another slskd (asked for by the person), so SONORA's can start.
    pub fn stop_other(&self, app: &App, pid: u32) -> Result<(), String> {
        if !self.others(app).iter().any(|p| p.pid == pid) {
            return Err("That slskd is not running any more.".into());
        }
        self.system().terminate(pid)?;
        logging::log_fields("info", "slskd", "stopped another slskd", obj! { "pid" => pid as i64 });
        // Give it a moment to let go of its ports.
        for _ in 0..20 {
            if !self.others(app).iter().any(|p| p.pid == pid) {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    }

    /// Quits the Soulseek app (asked for by the person).
    pub fn quit_soulseek_qt(&self) -> Result<(), String> {
        let sys = self.system();
        sys.quit_app(SOULSEEK_QT)?;
        for _ in 0..40 {
            if !sys.app_running(SOULSEEK_QT) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err("The Soulseek app is still open. Quit it yourself, then try again.".into())
    }

    /// The password of slskd's web page (for the person to copy).
    pub fn web_password(&self, app: &App) -> Option<String> {
        read_found(&self.layout(app).config).and_then(|f| f.web_password)
    }

    /// Looks after SONORA's slskd while SONORA runs (on its own thread):
    /// starts it again when it stopped, unless the person stopped it or
    /// turned it off, and when the computer wakes from sleep asks slskd to
    /// sign in again at once rather than after its own wait between
    /// attempts, which grows to five minutes. Never asks the person
    /// anything, and does nothing where SONORA did not set slskd up.
    pub fn watch(&self, app: &std::sync::Weak<App>) {
        let mut last = (Instant::now(), SystemTime::now());
        let mut since_look = Duration::ZERO;
        let pause = |d: Duration| {
            let until = Instant::now() + d;
            while Instant::now() < until {
                if self.quitting.load(Ordering::Relaxed) {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            true
        };
        loop {
            if !pause(Duration::from_millis(500)) {
                return;
            }
            // Sleep shows as a jump of the wall clock between two short looks.
            let now = (Instant::now(), SystemTime::now());
            let woke = slept(now.0 - last.0, now.1.duration_since(last.1).ok());
            since_look += now.0 - last.0;
            last = now;
            if !woke && since_look < WATCH_EVERY {
                continue;
            }
            since_look = Duration::ZERO;
            // SONORA's core is gone (a test that is over, say): so is the watcher.
            let Some(app) = app.upgrade() else { return };
            let app = &*app;
            if !self.configured(app) || crate::sources::accepted_at(&app.db.read()).is_none() {
                continue;
            }
            if woke {
                logging::log("info", "slskd", "the computer woke from sleep");
                // The network may need a few seconds after waking.
                if !pause(Duration::from_secs(5)) {
                    return;
                }
                last = (Instant::now(), SystemTime::now());
            }
            if let Err(e) = self.ensure(app) {
                logging::log_fields("warn", "slskd", "could not start slskd", obj! { "error" => e });
            }
            if woke {
                self.sign_in_again(app);
            }
        }
    }

    /// Asks slskd to connect to Soulseek now, when it runs, is not signed in,
    /// and nothing but the connection stands in the way (not a wrong
    /// password, nor the account signed in elsewhere). The watcher does this
    /// after the computer wakes.
    pub fn sign_in_again(&self, app: &App) {
        if self.is_stopped(app) || self.system().agent().pid.is_none() {
            return;
        }
        let Ok(info) = app.slskd.test(app) else { return };
        if info.get("loggedIn").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let layout = self.layout(app);
        let reason = newest_log(&layout.logs).and_then(|l| reason_in(&tail(&l, 0, 256 * 1024)));
        if matches!(reason, Some(Reason::WrongPassword | Reason::NoAccount | Reason::SignedInElsewhere)) {
            return;
        }
        match app.slskd.connect(app) {
            Ok(()) => logging::log("info", "slskd", "asked slskd to sign in again"),
            Err(e) => logging::log_fields("warn", "slskd", "could not ask slskd to sign in again", obj! { "code" => e.code }),
        }
    }
}

/// Where SONORA's own slskd answers.
pub fn managed_url() -> String {
    format!("http://127.0.0.1:{}", test_port().unwrap_or(PORT))
}

/// End-to-end tests run their stand-in slskd on another port.
fn test_port() -> Option<u16> {
    std::env::var("SONORA_SLSKD_TEST_PORT").ok().and_then(|p| p.parse().ok())
}

/// Waiting times, shortened for tests by `SONORA_SLSKD_WAIT_SECONDS`.
fn wait_secs(default: u64) -> u64 {
    std::env::var("SONORA_SLSKD_WAIT_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// macOS, run by CI only (`--ignored`, with SONORA_LAUNCHD_TEST=1): the
    /// launch agent SONORA writes is accepted by launchd, which runs the
    /// program in it, starts it again when asked and after a crash, and stops
    /// it; SONORA reads launchd's answers. It adds and removes a launch agent
    /// for the user running it, so it refuses to run where one is there.
    #[test]
    #[ignore]
    fn launchd_takes_the_launch_agent() {
        if !cfg!(target_os = "macos") || std::env::var("SONORA_LAUNCHD_TEST").as_deref() != Ok("1") {
            eprintln!("skipped: macOS with SONORA_LAUNCHD_TEST=1 only");
            return;
        }
        let service = Service::with_system(Arc::new(MacSystem));
        let sys = service.system();
        let root = std::env::temp_dir().join(format!("sonora-launchd-{}", std::process::id()));
        let layout = Layout::new(&root, sys.agents_dir());
        let plist_path = layout.plist().unwrap();
        assert!(!plist_path.exists(), "a launch agent for slskd is there already: {}", plist_path.display());
        // A stand-in for slskd: notes how it was started, then stays up.
        std::fs::create_dir_all(layout.app(VERSION)).unwrap();
        std::fs::create_dir_all(&layout.data).unwrap();
        let bin = layout.binary(VERSION);
        std::fs::write(&bin, "#!/bin/sh\necho \"$@\" > \"$(dirname \"$0\")/args.txt\"\nexec /bin/sleep 600\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let args = layout.app(VERSION).join("args.txt");
        let wait = |what: &str, secs: u64, f: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(secs);
            while !f() {
                assert!(Instant::now() < deadline, "{what}: launchd says {:?}", sys.agent());
                std::thread::sleep(Duration::from_millis(250));
            }
        };
        let clean = || {
            let _ = sys.unload();
            let _ = std::fs::remove_file(&plist_path);
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // In the background: launchd starts it as soon as it loads the agent.
            let agent = service.install_agent(&layout, VERSION, true).unwrap();
            assert!(agent.new && agent.loaded_now);
            wait("slskd started", 20, &|| sys.agent().pid.is_some() && args.exists());
            assert_eq!(std::fs::read_to_string(&args).unwrap().trim(), format!("--app-dir {} --no-logo", layout.data.display()));
            assert!(layout.service_log().exists(), "launchd made the error log");
            eprintln!("launchd ran it");
            // Restart.
            let first = sys.agent().pid.unwrap();
            sys.kickstart(true).unwrap();
            wait("slskd started again", 20, &|| sys.agent().pid.is_some_and(|p| p != first));
            // After a crash, launchd starts it again (after its 30 s throttle at most).
            let second = sys.agent().pid.unwrap();
            run("/bin/kill", &["-KILL", &second.to_string()]).unwrap();
            wait("slskd started again after a crash", 60, &|| sys.agent().pid.is_some_and(|p| p != second));
            // Stopped: unloaded, and not running.
            sys.unload().unwrap();
            wait("slskd stopped", 20, &|| !sys.agent().loaded);
            // Only while SONORA is open: loading does not start it; SONORA does.
            let agent = service.install_agent(&layout, VERSION, false).unwrap();
            assert!(agent.loaded_now);
            std::thread::sleep(Duration::from_secs(2));
            assert!(sys.agent().pid.is_none(), "it does not start at load");
            sys.kickstart(false).unwrap();
            wait("SONORA started slskd", 20, &|| sys.agent().pid.is_some());
            eprintln!("launchd ran it in both modes");
        }));
        clean();
        let _ = std::fs::remove_dir_all(&root);
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn sleep_is_told_from_the_clocks() {
        let s = Duration::from_secs;
        assert!(!slept(s(20), Some(s(20))));
        assert!(!slept(s(20), Some(s(45))), "a busy moment is not sleep");
        assert!(slept(s(20), Some(s(20 + 31))));
        assert!(slept(s(1), Some(s(3600))));
        assert!(!slept(s(20), None), "the clock went back");
    }

    #[test]
    fn pinned_releases() {
        let x = pinned("osx-x64").unwrap();
        assert_eq!(x.url, "https://github.com/slskd/slskd/releases/download/0.26.1/slskd-0.26.1-osx-x64.zip");
        assert_eq!(x.sha256.len(), 64);
        assert!(pinned("osx-arm64").is_some());
        assert!(pinned("win-x64").is_none());
    }

    #[test]
    fn launchd_answers() {
        let s = parse_launchctl_print("gui/501/app.sonora.analyzer.slskd = {\n\tactive count = 1\n\tstate = running\n\tpid = 4711\n\tlast exit code = 0\n}\n");
        assert_eq!(s, AgentState { loaded: true, pid: Some(4711), last_exit: Some(0), last_signal: None });
        let s = parse_launchctl_print("gui/501/x = {\n\tstate = not running\n\tlast exit code = (never exited)\n}");
        assert_eq!(s, AgentState { loaded: true, pid: None, last_exit: None, last_signal: None });
        let s = parse_launchctl_print("gui/501/x = {\n\tstate = not running\n\tlast exit code = 78: EX_CONFIG\n}");
        assert_eq!(s.last_exit, Some(78));
        let s = parse_launchctl_print("gui/501/x = {\n\tstate = not running\n\tlast terminating signal = Killed: 9\n}");
        assert_eq!(s.last_signal, Some(9));
    }

    #[test]
    fn launch_agent_runs_slskd_with_its_folder() {
        let home = std::env::temp_dir().join("me");
        let layout = Layout::new(&home.join("Library").join("Application Support").join("app.sonora.analyzer"), Some(home.join("Library").join("LaunchAgents")));
        let p = plist(&layout, "0.26.1", true);
        let bin = layout.root.join("app-0.26.1").join("slskd");
        assert!(p.contains("<string>app.sonora.analyzer.slskd</string>"));
        assert!(p.contains(&format!("<string>{}</string>", xml(&bin.to_string_lossy()))));
        assert!(p.contains(&format!("<string>--app-dir</string>\n\t\t<string>{}</string>", xml(&layout.root.join("data").to_string_lossy()))));
        assert!(p.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(p.contains("<key>SuccessfulExit</key>\n\t\t<false/>"));
        assert!(p.contains("<string>/dev/null</string>"));
        assert_eq!(layout.plist().unwrap(), home.join("Library").join("LaunchAgents").join("app.sonora.analyzer.slskd.plist"));
        let fg = plist(&layout, "0.26.1", false);
        assert!(fg.contains("<key>RunAtLoad</key>\n\t<false/>") && fg.contains("<key>KeepAlive</key>\n\t<false/>"));
        // Paths are escaped for XML.
        let odd = Layout::new(&home.join("a&b <c>"), Some(home.join("agents")));
        assert!(plist(&odd, "1.0", true).contains("a&amp;b &lt;c&gt;"));
    }

    #[test]
    fn settings_file_round_trips() {
        let s = Settings {
            username: "dj 'night'".into(),
            password: "p@ss: #1".into(),
            downloads: "/Users/me/Music/Soulseek".into(),
            incomplete: "/x/incomplete".into(),
            shares: vec!["/Users/me/Music/Shared".into()],
            port: 5030,
            web_password: "web".into(),
            api_key: "ABCDEFGHIJKLMNOPQRSTUVWXYZ012345".into(),
        };
        let text = render(&s);
        let dir = std::env::temp_dir().join(format!("sonora-yml-{}", std::process::id()));
        let path = dir.join("slskd.yml");
        write_private(&path, &text).unwrap();
        let f = read_found(&path).unwrap();
        assert_eq!(f.username.as_deref(), Some("dj 'night'"));
        assert_eq!(f.password.as_deref(), Some("p@ss: #1"));
        assert_eq!(f.downloads.as_deref(), Some("/Users/me/Music/Soulseek"));
        assert_eq!(f.shares, vec!["/Users/me/Music/Shared".to_string()]);
        assert_eq!(f.api_key.as_deref(), Some("ABCDEFGHIJKLMNOPQRSTUVWXYZ012345"));
        let n = yaml::parse(&text);
        assert_eq!(n.str_at(&["web", "ip_address"]), Some("127.0.0.1"));
        assert_eq!(n.str_at(&["web", "https", "disabled"]), Some("true"));
        assert_eq!(n.str_at(&["flags", "no_version_check"]), Some("true"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "only the person's account reads it");
        }
        // One value changes, with a backup of the file as it was.
        assert!(change_value(&path, &["soulseek", "password"], "new one").unwrap());
        assert_eq!(read_found(&path).unwrap().password.as_deref(), Some("new one"));
        let backups: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".bak")).collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(read_found(&backups[0].path()).unwrap().password.as_deref(), Some("p@ss: #1"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sign_in_problems_from_the_log() {
        let log = "[2026-10-20T17:55:04.000 INF] Configuration complete\n\
[2026-10-20T17:55:04.100 WRN] Not connecting to the Soulseek server; username and/or password invalid.  Specify valid credentials\n";
        assert_eq!(reason_in(log), Some(Reason::NoAccount));
        let log2 = format!("{log}[2026-10-20T17:56:00.000 INF] Logged in to the Soulseek server as dj\n");
        assert_eq!(reason_in(&log2), None, "a later sign-in clears it");
        assert_eq!(reason_in("[x ERR] Disconnected from the Soulseek server: invalid username or password\n"), Some(Reason::WrongPassword));
        assert_eq!(reason_in("[x ERR] Disconnected from the Soulseek server: another client logged in using the same username\n"), Some(Reason::SignedInElsewhere));
        assert_eq!(reason_in("[x ERR] Failed to reconnect: Connection refused\n"), Some(Reason::Network("Connection refused".into())));
        assert_eq!(reason_in("[x INF] Disconnected from the Soulseek server: the client is shutting down\n"), None);
    }

    #[test]
    fn backup_names() {
        assert_eq!(stamp(SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_489_524)), "20261020-094524");
        assert_eq!(stamp(SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400)), "20000229-000000");
        assert_eq!(stamp(SystemTime::UNIX_EPOCH), "19700101-000000");
    }

    #[test]
    fn requests_are_checked() {
        let music = std::env::temp_dir().join("Music").join("Soulseek");
        let dl = music.to_string_lossy().into_owned();
        let adopt = std::env::temp_dir().join("slskd.yml").to_string_lossy().into_owned();
        let ok = obj! { "username" => " dj ", "password" => "pw", "downloads" => dl.as_str() };
        let r = Request::from_json(&ok).unwrap();
        assert_eq!(r.username, "dj");
        assert_eq!(r.password.as_deref(), Some("pw"));
        assert_eq!(r.downloads, music);
        assert!(Request::from_json(&obj! { "password" => "pw", "downloads" => dl.as_str() }).is_err());
        assert!(Request::from_json(&obj! { "username" => "dj", "downloads" => dl.as_str() }).is_err(), "a password, or settings to take it from");
        assert!(Request::from_json(&obj! { "username" => "dj", "password" => "a\nb", "downloads" => dl.as_str() }).is_err());
        assert!(Request::from_json(&obj! { "username" => "dj", "password" => "pw", "downloads" => "relative/path" }).is_err());
        assert!(Request::from_json(&obj! { "username" => "dj", "adopt" => adopt.as_str(), "downloads" => dl.as_str() }).is_ok());
    }
}
