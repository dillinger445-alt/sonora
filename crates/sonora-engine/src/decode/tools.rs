//! Discovery and invocation of the FFmpeg/ffprobe sidecar binaries.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Locations of external decoding tools. Missing tools degrade gracefully:
/// WAV/AIFF still decode natively and other formats fail with a clear error.
#[derive(Clone, Debug, Default)]
pub struct ToolPaths {
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    /// First line of `ffmpeg -version`, when known.
    pub ffmpeg_version: Option<String>,
}

impl ToolPaths {
    /// No external tools (native decoders only).
    pub fn none() -> Self {
        ToolPaths::default()
    }

    /// Searches, in order: explicit overrides (`SONORA_FFMPEG`/`SONORA_FFPROBE`
    /// env vars), `extra_dirs` (e.g. the app's sidecar directory or a
    /// user-configured folder), the directory of the running executable
    /// (where Tauri places sidecars), `PATH`, and common package-manager
    /// locations (macOS GUI apps do not inherit the shell `PATH`).
    pub fn discover(extra_dirs: &[PathBuf]) -> Self {
        let mut dirs: Vec<PathBuf> = extra_dirs.to_vec();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                dirs.push(dir.to_path_buf());
                // macOS bundle: Contents/MacOS/<exe> -> Contents/Resources
                dirs.push(dir.join("../Resources"));
            }
        }
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        dirs.extend(platform_fallback_dirs());

        let ffmpeg = env_tool("SONORA_FFMPEG").or_else(|| find_in(&dirs, "ffmpeg"));
        let ffprobe = env_tool("SONORA_FFPROBE")
            .or_else(|| ffmpeg.as_ref().and_then(|p| sibling(p, "ffprobe")))
            .or_else(|| find_in(&dirs, "ffprobe"));
        let mut tools = ToolPaths { ffmpeg, ffprobe, ffmpeg_version: None };
        tools.ffmpeg_version = tools.ffmpeg.as_ref().and_then(|p| tool_version(p));
        if tools.ffmpeg_version.is_none() {
            // Found a file that does not run (wrong architecture, not executable...).
            tools.ffmpeg = None;
        }
        if let Some(probe) = &tools.ffprobe {
            if tool_version(probe).is_none() {
                tools.ffprobe = None;
            }
        }
        tools
    }

    pub fn has_ffmpeg(&self) -> bool {
        self.ffmpeg.is_some()
    }
}

fn exe_name(base: &str) -> String {
    format!("{base}{}", std::env::consts::EXE_SUFFIX)
}

fn env_tool(var: &str) -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os(var)?);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

fn sibling(path: &Path, base: &str) -> Option<PathBuf> {
    let candidate = path.parent()?.join(exe_name(base));
    candidate.is_file().then_some(candidate)
}

fn find_in(dirs: &[PathBuf], base: &str) -> Option<PathBuf> {
    let name = exe_name(base);
    dirs.iter().map(|d| d.join(&name)).find(|p| p.is_file())
}

fn platform_fallback_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(target_os = "macos") {
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin"] {
            v.push(PathBuf::from(d));
        }
    } else if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            v.push(PathBuf::from(&local).join("Microsoft").join("WinGet").join("Links"));
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            v.push(PathBuf::from(profile).join("scoop").join("shims"));
        }
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            v.push(PathBuf::from(pf).join("ffmpeg").join("bin"));
        }
        v.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin"));
        v.push(PathBuf::from(r"C:\ffmpeg\bin"));
    } else {
        for d in ["/usr/local/bin", "/usr/bin", "/snap/bin"] {
            v.push(PathBuf::from(d));
        }
    }
    v
}

/// Prepares a command so it never opens a console window on Windows and
/// never reads from our stdin.
pub fn quiet_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Returns the first line of `<tool> -version`, or `None` if it cannot run.
pub fn tool_version(path: &Path) -> Option<String> {
    let mut cmd = quiet_command(path);
    cmd.arg("-version");
    let out = run_with_timeout(cmd, Duration::from_secs(10)).ok()?;
    if !out.success {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty())
}

pub struct ProcessOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

/// Runs a command to completion, draining stdout/stderr concurrently so large
/// outputs cannot deadlock, and kills it after `timeout`.
pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> std::io::Result<ProcessOutput> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child: Child = cmd.spawn()?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stderr).take(1 << 20).read_to_end(&mut buf);
        // Keep draining so the child never blocks on a full pipe.
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        buf
    });
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            timed_out = true;
            break child.wait().ok();
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok(ProcessOutput { success: !timed_out && status.map(|s| s.success()).unwrap_or(false), stdout, stderr, timed_out })
}
