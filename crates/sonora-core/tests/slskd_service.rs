//! Setting up slskd and keeping it running, against a stand-in system (no
//! launchd, no download from GitHub) and a stand-in slskd that reads the
//! settings file SONORA wrote, as slskd does.

use sonora_core::net::{Http, NetError, Request, Response};
use sonora_core::slskd_service::{read_found, Process, TestSystem};
use sonora_core::{App, AppConfig};
use sonora_json::{obj, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A stand-in slskd: accepts the API key that is in its slskd.yml, signs in
/// when the password there is "right", and writes what slskd writes in its
/// log when it cannot.
struct FakeSlskd {
    config: PathBuf,
    logs: PathBuf,
    /// Another program signs in with the same account.
    kicked: Mutex<bool>,
    /// The Soulseek server cannot be reached (the computer just woke, say).
    offline: Mutex<bool>,
    /// Answers only while this stand-in system says slskd runs.
    follows: Mutex<Option<Arc<TestSystem>>>,
    requests: Mutex<Vec<String>>,
}

impl FakeSlskd {
    fn log(&self, line: &str) {
        use std::io::Write;
        std::fs::create_dir_all(&self.logs).unwrap();
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.logs.join("slskd-20261020.log")).unwrap();
        writeln!(f, "[2026-10-20T10:00:00.000 ERR] {line}").unwrap();
    }
}

impl Http for FakeSlskd {
    fn send(&self, req: &Request) -> Result<Response, NetError> {
        let Some(found) = read_found(&self.config) else { return Err(NetError::Unreachable("not running".into())) };
        if self.follows.lock().unwrap().as_ref().is_some_and(|sys| !sys.state.lock().unwrap().running) {
            return Err(NetError::Unreachable("not running".into()));
        }
        let path = req.url.split("/api/v0/").nth(1).unwrap_or("").to_string();
        if req.url.starts_with("http://nas.local:5030/") {
            // Another slskd, which does not take the key SONORA sends it.
            return Ok(Response { status: 401, body: String::new() });
        }
        assert!(req.url.starts_with("http://127.0.0.1:5030/"), "SONORA talks to its own slskd: {}", req.url);
        self.requests.lock().unwrap().push(format!("{} {path}", req.method));
        let key = req.headers.iter().find(|(k, _)| k == "X-API-Key").map(|(_, v)| v.clone());
        if found.api_key.is_none() || key != found.api_key {
            return Ok(Response { status: 401, body: String::new() });
        }
        if path == "server" && req.method == "PUT" {
            return Ok(Response { status: 200, body: String::new() });
        }
        if path == "application" {
            let kicked = *self.kicked.lock().unwrap();
            let offline = *self.offline.lock().unwrap();
            let ok = found.password.as_deref() == Some("right") && !kicked && !offline;
            if !ok {
                self.log(if kicked {
                    "Disconnected from the Soulseek server: another client logged in using the same username"
                } else if offline {
                    "Failed to reconnect: Network is unreachable"
                } else {
                    "Disconnected from the Soulseek server: invalid username or password"
                });
            }
            let body = obj! {
                "version" => obj! { "current" => "0.26.1" },
                "server" => obj! { "isConnected" => ok, "isLoggedIn" => ok },
                "user" => obj! { "username" => found.username.clone() },
            };
            return Ok(Response { status: 200, body: body.to_string_compact() });
        }
        Ok(Response { status: 404, body: String::new() })
    }
}

struct Env {
    root: PathBuf,
    app: Arc<App>,
    sys: Arc<TestSystem>,
    fake: Arc<FakeSlskd>,
    home: PathBuf,
}

/// A zip like slskd's release: the program and its web page.
fn archive(dir: &Path) -> Option<PathBuf> {
    let src = dir.join("release");
    std::fs::create_dir_all(src.join("wwwroot")).unwrap();
    std::fs::write(src.join("slskd"), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::write(src.join("wwwroot").join("index.html"), "<html></html>").unwrap();
    let zip = dir.join("slskd-0.26.1-test.zip");
    let ok = std::process::Command::new("zip").arg("-q").arg("-r").arg(&zip).arg(".").current_dir(&src).status().map(|s| s.success()).unwrap_or(false);
    ok.then_some(zip)
}

fn env(name: &str) -> Option<Env> {
    std::env::set_var("SONORA_SOURCE_POLL_SECONDS", "0");
    std::env::set_var("SONORA_SECRET_STORE", "file");
    std::env::set_var("SONORA_SLSKD_WAIT_SECONDS", "4");
    let root = std::env::temp_dir().join(format!("sonora-service-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let Some(zip) = archive(&root) else {
        eprintln!("zip is not available: skipping");
        return None;
    };
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let data = root.join("data");
    let app = App::open(AppConfig { data_dir: data.clone(), tool_dirs: vec![], app_version: "test".into() }).unwrap();
    let sys = Arc::new(TestSystem::new(&home, &home.join("Library").join("LaunchAgents"), Some(&zip)));
    app.service.set_system(sys.clone());
    let fake = Arc::new(FakeSlskd {
        config: data.join("slskd").join("data").join("slskd.yml"),
        logs: data.join("slskd").join("data").join("logs"),
        kicked: Mutex::new(false),
        offline: Mutex::new(false),
        follows: Mutex::new(None),
        requests: Mutex::new(Vec::new()),
    });
    app.slskd.set_http(fake.clone());
    call(&app, "sources.accept", &obj! {});
    Some(Env { root, app, sys, fake, home })
}

fn call(app: &Arc<App>, method: &str, params: &Value) -> Value {
    let out = app.call(method, &params.to_string_compact()).unwrap_or_else(|e| panic!("{method}: {e}"));
    sonora_json::parse(&out).unwrap()
}

fn call_err(app: &Arc<App>, method: &str, params: &Value) -> (String, String) {
    let e = app.call(method, &params.to_string_compact()).expect_err(method);
    (e.code, e.message)
}

/// Waits for the setup to stop, and returns its state.
fn wait_setup(app: &Arc<App>) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let st = call(app, "slskd.managed", &obj! {});
        let setup = st.get("setup").cloned().unwrap_or(Value::Null);
        if setup.get("active").and_then(Value::as_bool) == Some(false) {
            return setup;
        }
        assert!(Instant::now() < deadline, "the setup did not finish: {setup:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn failed_step(setup: &Value) -> (String, String) {
    let e = setup.get("error").unwrap_or(&Value::Null);
    (e.get("step").and_then(Value::as_str).unwrap_or("").to_string(), e.get("fix").and_then(Value::as_str).unwrap_or("").to_string())
}

#[test]
fn one_button_setup_from_download_to_signed_in() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let Some(e) = env("flow") else { return };
    let app = &e.app;

    // What the screen offers: the download and SONORA's folder.
    let plan = call(app, "slskd.setupPlan", &obj! {});
    assert_eq!(plan.get("version").and_then(Value::as_str), Some("0.26.1"));
    assert!(plan.get("download").and_then(|d| d.get("size")).and_then(Value::as_i64).unwrap() > 0);
    assert!(plan.get("found").unwrap().is_null(), "no settings on this computer yet");
    let downloads = e.home.join("Music").join("Soulseek");
    assert_eq!(plan.get("downloads").and_then(Value::as_str), Some(downloads.to_str().unwrap()));

    // A wrong password: the setup gets as far as signing in and says why.
    let req = obj! { "username" => "dj", "password" => "wrong", "downloads" => downloads.to_str().unwrap() };
    call(app, "slskd.setupStart", &req);
    let setup = wait_setup(app);
    assert_eq!(failed_step(&setup), ("signin".to_string(), "password".to_string()), "{setup:?}");
    assert!(downloads.is_dir(), "the download folder was made");

    // The right one: the settings are written again and slskd starts again with them.
    call(app, "slskd.setupRetry", &obj! { "password" => "right" });
    let setup = wait_setup(app);
    assert!(setup.get("error").unwrap().is_null(), "{setup:?}");
    assert_eq!(setup.get("finished").and_then(Value::as_bool), Some(true));
    assert_eq!(setup.get("signedInAs").and_then(Value::as_str), Some("dj"));
    assert_eq!(setup.get("passwordKept").and_then(Value::as_bool), Some(false), "the password is not kept once it is in slskd.yml");

    // What SONORA wrote: slskd.yml (the person's account only), a launch
    // agent that starts slskd at login, and its own settings.
    let found = read_found(&e.fake.config).unwrap();
    assert_eq!(found.username.as_deref(), Some("dj"));
    assert_eq!(found.password.as_deref(), Some("right"));
    assert_eq!(found.downloads.as_deref(), downloads.to_str());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&e.fake.config).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let plist = e.home.join("Library").join("LaunchAgents").join("app.sonora.analyzer.slskd.plist");
    let text = std::fs::read_to_string(&plist).unwrap();
    assert!(text.contains("<key>RunAtLoad</key>\n\t<true/>"));
    let settings = call(app, "settings.get", &obj! {});
    assert_eq!(settings.get("slskdUrl").and_then(Value::as_str), Some("http://127.0.0.1:5030"));
    assert_eq!(settings.get("soulseekFolder").and_then(Value::as_str), downloads.to_str());
    let st = call(app, "slskd.status", &obj! {});
    assert_eq!(st.get("keySaved").and_then(Value::as_bool), Some(true));
    let calls = e.sys.state.lock().unwrap().calls.clone();
    assert!(calls.iter().any(|c| c.starts_with("fetch ")) && calls.iter().any(|c| c == "unpack"), "{calls:?}");
    assert!(calls.iter().any(|c| c.starts_with("load ")), "{calls:?}");
    let t = call(app, "slskd.test", &obj! {});
    assert_eq!(t.get("loggedIn").and_then(Value::as_bool), Some(true));
    let managed = call(app, "slskd.managed", &obj! {});
    assert_eq!(managed.get("configured").and_then(Value::as_bool), Some(true));
    assert_eq!(managed.get("installed").and_then(Value::as_str), Some("0.26.1"));
    assert_eq!(managed.get("agent").and_then(|a| a.get("running")).and_then(Value::as_bool), Some(true));
    assert!(managed.get("keyProblem").unwrap().is_null());
    let key_before = found.api_key.clone().unwrap();

    // The person changes slskd's settings themselves (slots, a second
    // shared folder, another key, a comment) and runs the setup again.
    let shared = [e.root.join("Shared A"), e.root.join("Shared B")];
    for d in &shared {
        std::fs::create_dir_all(d).unwrap();
    }
    let yml = std::fs::read_to_string(&e.fake.config).unwrap();
    let yml = yml.replace("shares:\n  directories: []\n", &format!("shares:\n  directories:\n    - '{}'\n    - '{}'\n", shared[0].display(), shared[1].display()));
    let yml = yml.replace("      sonora:\n", "      phone:\n        key: 'phonekey0123456789'\n        role: readonly\n      sonora:\n");
    let yml = format!("{yml}# my own notes\nglobal:\n  upload:\n    slots: 7\n");
    std::fs::write(&e.fake.config, &yml).unwrap();

    // Running the setup again changes the setup in place: the same key, one
    // launch agent, a backup of the settings as they were, nothing downloaded
    // again, and the person's other settings as they were.
    e.sys.state.lock().unwrap().calls.clear();
    call(app, "slskd.setupStart", &obj! { "username" => "dj", "password" => "right", "downloads" => downloads.to_str().unwrap(), "share" => shared[0].to_str().unwrap() });
    let setup = wait_setup(app);
    assert!(setup.get("error").unwrap().is_null(), "{setup:?}");
    assert_eq!(read_found(&e.fake.config).unwrap().api_key.as_deref(), Some(key_before.as_str()));
    let after = std::fs::read_to_string(&e.fake.config).unwrap();
    assert!(after.contains("phonekey0123456789") && after.contains("slots: 7") && after.contains("# my own notes"), "{after}");
    assert_eq!(read_found(&e.fake.config).unwrap().shares, vec![shared[0].to_string_lossy().into_owned(), shared[1].to_string_lossy().into_owned()], "both shares stay");
    // Sharing turned off in the setup: nothing is shared any more; the rest stays.
    call(app, "slskd.setupStart", &obj! { "username" => "dj", "password" => "right", "downloads" => downloads.to_str().unwrap() });
    assert!(wait_setup(app).get("error").unwrap().is_null());
    assert!(read_found(&e.fake.config).unwrap().shares.is_empty());
    assert!(std::fs::read_to_string(&e.fake.config).unwrap().contains("slots: 7"));
    let calls = e.sys.state.lock().unwrap().calls.clone();
    assert!(!calls.iter().any(|c| c.starts_with("fetch ")), "already downloaded and checked: {calls:?}");
    assert!(calls.iter().any(|c| c == "restart"), "slskd starts again to read its new settings: {calls:?}");
    let agents: Vec<_> = std::fs::read_dir(plist.parent().unwrap()).unwrap().flatten().collect();
    assert_eq!(agents.len(), 1);
    let backups = std::fs::read_dir(e.fake.config.parent().unwrap()).unwrap().flatten().filter(|f| f.file_name().to_string_lossy().ends_with(".bak")).count();
    assert!(backups >= 1);

    // SONORA's key and the one in slskd.yml differ (the setup ran on another
    // copy of SONORA, say): SONORA takes the one slskd uses, without being asked.
    call(app, "slskd.configure", &obj! { "apiKey" => "someotherkey0123456789" });
    let t = call(app, "slskd.test", &obj! {});
    assert_eq!(t.get("loggedIn").and_then(Value::as_bool), Some(true));
    assert_eq!(app.slskd.key(app).unwrap().as_deref(), Some(key_before.as_str()));
    // But never the key of another slskd SONORA was pointed at (a NAS, say).
    call(app, "slskd.configure", &obj! { "url" => "http://nas.local:5030", "apiKey" => "naskey0123456789abc" });
    let _ = app.call("slskd.test", "{}");
    assert_eq!(app.slskd.key(app).unwrap().as_deref(), Some("naskey0123456789abc"));
    call(app, "slskd.configure", &obj! { "url" => "http://127.0.0.1:5030", "apiKey" => key_before.as_str() });

    // slskd.yml lost its key line: Repair puts SONORA's key back.
    let yml = std::fs::read_to_string(&e.fake.config).unwrap();
    let without: String = yml.lines().filter(|l| !l.trim_start().starts_with("key:")).map(|l| format!("{l}\n")).collect();
    std::fs::write(&e.fake.config, without).unwrap();
    assert_eq!(call(app, "slskd.managed", &obj! {}).get("keyProblem").and_then(Value::as_str), Some("missing"));
    assert_eq!(call_err(app, "slskd.test", &obj! {}).0, "slskd_unauthorized");
    call(app, "slskd.repair", &obj! {});
    assert_eq!(read_found(&e.fake.config).unwrap().api_key.as_deref(), Some(key_before.as_str()));
    assert_eq!(call(app, "slskd.test", &obj! {}).get("loggedIn").and_then(Value::as_bool), Some(true));

    // A new password goes into slskd.yml, and slskd starts again.
    e.sys.state.lock().unwrap().calls.clear();
    call(app, "slskd.password", &obj! { "password" => "right" });
    assert!(e.sys.state.lock().unwrap().calls.iter().any(|c| c == "restart"));
    assert_eq!(call_err(app, "slskd.password", &obj! { "password" => "two\nlines" }).0, "slskd_service");

    // Only while SONORA is open: the agent no longer starts slskd at login.
    call(app, "slskd.background", &obj! { "enabled" => false });
    assert!(std::fs::read_to_string(&plist).unwrap().contains("<key>RunAtLoad</key>\n\t<false/>"));
    call(app, "slskd.background", &obj! { "enabled" => true });
    assert!(std::fs::read_to_string(&plist).unwrap().contains("<key>RunAtLoad</key>\n\t<true/>"));

    // Stopped by the person: it stays stopped until they start it, also
    // when SONORA opens again (the choice is saved) and at login (the launch
    // agent no longer starts it).
    call(app, "slskd.service", &obj! { "action" => "stop" });
    assert_eq!(call(app, "settings.get", &obj! {}).get("slskdStopped").and_then(Value::as_bool), Some(true));
    assert!(std::fs::read_to_string(&plist).unwrap().contains("<key>RunAtLoad</key>\n\t<false/>"));
    call(app, "slskd.background", &obj! { "enabled" => true });
    assert!(std::fs::read_to_string(&plist).unwrap().contains("<key>RunAtLoad</key>\n\t<false/>"), "the background switch does not start a stopped slskd");
    call(app, "slskd.managed", &obj! { "ensure" => true });
    assert_eq!(call(app, "slskd.managed", &obj! {}).get("agent").and_then(|a| a.get("running")).and_then(Value::as_bool), Some(false));
    call(app, "slskd.service", &obj! { "action" => "start" });
    assert_eq!(call(app, "slskd.managed", &obj! {}).get("agent").and_then(|a| a.get("running")).and_then(Value::as_bool), Some(true));
    assert_eq!(call(app, "settings.get", &obj! {}).get("slskdStopped").and_then(Value::as_bool), Some(false));
    assert!(std::fs::read_to_string(&plist).unwrap().contains("<key>RunAtLoad</key>\n\t<true/>"), "it starts at login again");

    // After the computer wakes, slskd is asked to sign in again at once, but
    // only when the connection is what stands in the way.
    let puts = |f: &FakeSlskd| f.requests.lock().unwrap().iter().filter(|r| r.as_str() == "PUT server").count();
    let before = puts(&e.fake);
    e.app.service.sign_in_again(app);
    assert_eq!(puts(&e.fake), before, "signed in already");
    *e.fake.offline.lock().unwrap() = true;
    e.app.service.sign_in_again(app);
    assert_eq!(puts(&e.fake), before + 1, "not signed in: the Soulseek server could not be reached");
    *e.fake.offline.lock().unwrap() = false;
    *e.fake.kicked.lock().unwrap() = true;
    e.app.service.sign_in_again(app);
    assert_eq!(puts(&e.fake), before + 1, "signed in elsewhere: asking again would sign the other one out");
    *e.fake.kicked.lock().unwrap() = false;

    // Turned off for good: the launch agent goes, everything else stays.
    let r = call(app, "slskd.removeAgent", &obj! {});
    assert!(!plist.exists());
    assert!(e.fake.config.is_file());
    assert!(r.get("kept").and_then(Value::as_str).is_some());
    // And it stays off: opening SONORA or its Soulseek page, or the
    // background switch, does not bring it back; Start does.
    let running = |app: &Arc<App>| call(app, "slskd.managed", &obj! {}).get("agent").and_then(|a| a.get("running")).and_then(Value::as_bool);
    call(app, "slskd.managed", &obj! { "ensure" => true });
    call(app, "slskd.background", &obj! { "enabled" => false });
    call(app, "slskd.background", &obj! { "enabled" => true });
    assert!(!plist.exists());
    assert_eq!(running(app), Some(false));
    call(app, "slskd.service", &obj! { "action" => "start" });
    assert!(plist.exists());
    assert_eq!(running(app), Some(true));
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn what_stands_in_the_way_is_named_and_cleared_with_the_persons_ok() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let Some(e) = env("conflicts") else { return };
    let app = &e.app;
    let downloads = e.root.join("Soulseek downloads");

    // Settings left by the earlier setup script on the Desktop are taken over:
    // the account, the password (not shown) and the download folder.
    let desktop = e.home.join("Desktop").join("slskd").join("data");
    std::fs::create_dir_all(&desktop).unwrap();
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(
        desktop.join("slskd.yml"),
        format!("soulseek:\n  username: 'dj'\n  password: 'right'\ndirectories:\n  downloads: '{}'\nshares:\n  directories: []\n", downloads.display()),
    )
    .unwrap();
    let plan = call(app, "slskd.setupPlan", &obj! {});
    let found = plan.get("found").unwrap();
    assert_eq!(found.get("username").and_then(Value::as_str), Some("dj"));
    assert_eq!(found.get("hasPassword").and_then(Value::as_bool), Some(true));
    assert!(found.get("password").is_none(), "the password itself never leaves the core");
    assert_eq!(plan.get("downloads").and_then(Value::as_str), downloads.to_str());

    // Another slskd holds the ports, and the Soulseek app is signed in.
    {
        let mut st = e.sys.state.lock().unwrap();
        st.processes.push(Process { pid: 999, path: "/Users/me/Desktop/slskd/slskd".into() });
        st.soulseek_qt = true;
    }
    *e.fake.kicked.lock().unwrap() = true;
    let adopt = desktop.join("slskd.yml");
    call(app, "slskd.setupStart", &obj! { "username" => "dj", "downloads" => downloads.to_str().unwrap(), "adopt" => adopt.to_str().unwrap() });
    let setup = wait_setup(app);
    assert_eq!(failed_step(&setup), ("start".to_string(), "stop_other".to_string()), "{setup:?}");
    let st = call(app, "slskd.managed", &obj! {});
    assert_eq!(st.get("others").and_then(Value::as_array).map(Vec::len), Some(1));
    assert_eq!(st.get("soulseekQt").and_then(Value::as_bool), Some(true));

    // The person stops it; the setup goes on from where it stopped.
    call(app, "slskd.stopOther", &obj! { "pid" => 999 });
    assert!(e.sys.state.lock().unwrap().calls.iter().any(|c| c == "terminate 999"));

    // macOS stops a program marked as downloaded from the internet (SONORA's
    // own download is not): the step says where the person can allow it,
    // and SONORA changes nothing there itself.
    *e.fake.follows.lock().unwrap() = Some(e.sys.clone());
    e.sys.state.lock().unwrap().quarantined = true;
    call(app, "slskd.setupRetry", &obj! {});
    let setup = wait_setup(app);
    assert_eq!(failed_step(&setup), ("start".to_string(), "gatekeeper".to_string()), "{setup:?}");
    let message = setup.get("error").and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("").to_string();
    assert!(message.contains("Privacy & Security") && message.contains("Open Anyway"), "{message}");
    // The person allowed it in System Settings.
    e.sys.state.lock().unwrap().quarantined = false;
    call(app, "slskd.setupRetry", &obj! {});
    let setup = wait_setup(app);
    assert_eq!(failed_step(&setup), ("signin".to_string(), "quit_soulseek_qt".to_string()), "{setup:?}");

    // The person lets SONORA quit the Soulseek app.
    call(app, "slskd.quitSoulseekQt", &obj! {});
    *e.fake.kicked.lock().unwrap() = false;
    call(app, "slskd.setupRetry", &obj! {});
    let setup = wait_setup(app);
    assert!(setup.get("error").unwrap().is_null(), "{setup:?}");
    assert_eq!(read_found(&e.fake.config).unwrap().password.as_deref(), Some("right"), "the password came from the Desktop settings");
    // Those settings are left as they were.
    assert!(std::fs::read_to_string(desktop.join("slskd.yml")).unwrap().contains("username: 'dj'"));
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn a_download_that_does_not_match_is_not_used() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let Some(e) = env("checksum") else { return };
    let app = &e.app;
    // The pinned checksum is not the archive's.
    let zip = e.root.join("slskd-0.26.1-test.zip");
    let mut sys = TestSystem::new(&e.home, &e.home.join("Library").join("LaunchAgents"), Some(&zip));
    sys.build.as_mut().unwrap().sha256 = "0".repeat(64);
    let sys = Arc::new(sys);
    app.service.set_system(sys.clone());
    let downloads = e.root.join("dl");
    call(app, "slskd.setupStart", &obj! { "username" => "dj", "password" => "right", "downloads" => downloads.to_str().unwrap() });
    let setup = wait_setup(app);
    assert_eq!(failed_step(&setup), ("verify".to_string(), "retry".to_string()), "{setup:?}");
    let archives = e.app.config.data_dir.join("slskd").join("downloads");
    assert_eq!(std::fs::read_dir(&archives).unwrap().count(), 0, "the archive that did not match is gone");
    assert!(!e.app.config.data_dir.join("slskd").join("app-0.26.1").exists(), "nothing was unpacked");

    // A failed download says why and can be tried again.
    sys.state.lock().unwrap().fetch_error = Some("the computer could not find github.com".into());
    call(app, "slskd.setupRetry", &obj! {});
    let setup = wait_setup(app);
    assert_eq!(failed_step(&setup), ("download".to_string(), "network".to_string()), "{setup:?}");
    assert!(setup.get("error").and_then(|x| x.get("message")).and_then(Value::as_str).unwrap().contains("github.com"));

    // The person asks to remove what the setup made: only that goes.
    let created = setup.get("created").and_then(Value::as_array).cloned().unwrap_or_default();
    assert!(!created.is_empty());
    let r = call(app, "slskd.setupDiscard", &obj! {});
    assert!(!r.get("removed").and_then(Value::as_array).unwrap().is_empty());
    assert!(!e.app.config.data_dir.join("slskd").exists());
    assert!(e.app.config.data_dir.join("sonora.db").exists(), "SONORA's own data stays");
    let _ = std::fs::remove_dir_all(&e.root);
}

#[test]
fn removing_what_a_setup_made_puts_settings_back_and_repair_keeps_the_rest_of_slskd_yml() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let Some(e) = env("undo") else { return };
    let app = &e.app;

    // B22: the person had another slskd (on a NAS) and watched a folder.
    let earlier = e.root.join("Earlier downloads");
    std::fs::create_dir_all(&earlier).unwrap();
    call(app, "slskd.configure", &obj! { "url" => "http://nas.local:5030" });
    call(app, "sources.configure", &obj! { "folder" => earlier.to_str().unwrap() });
    let downloads = e.home.join("Music").join("Soulseek");
    call(app, "slskd.setupStart", &obj! { "username" => "dj", "password" => "wrong", "downloads" => downloads.to_str().unwrap() });
    let setup = wait_setup(app);
    assert_eq!(failed_step(&setup).0, "signin", "{setup:?}");
    let s = call(app, "settings.get", &obj! {});
    assert_eq!(s.get("slskdUrl").and_then(Value::as_str), Some("http://127.0.0.1:5030"), "the setup pointed SONORA at its own slskd");
    call(app, "slskd.setupDiscard", &obj! {});
    let s = call(app, "settings.get", &obj! {});
    assert_eq!(s.get("slskdUrl").and_then(Value::as_str), Some("http://nas.local:5030"), "the earlier address is back");
    assert_eq!(s.get("soulseekFolder").and_then(Value::as_str), earlier.to_str(), "the earlier folder is back");
    assert!(!downloads.exists(), "the empty download folder it made is gone");

    // B23: after a working setup, the person renames SONORA's key entry and
    // adds settings of their own; Repair puts the key back and keeps them.
    call(app, "slskd.setupStart", &obj! { "username" => "dj", "password" => "right", "downloads" => downloads.to_str().unwrap() });
    let setup = wait_setup(app);
    assert!(setup.get("error").unwrap().is_null(), "{setup:?}");
    let yml = std::fs::read_to_string(&e.fake.config).unwrap().replace("      sonora:\n", "      mine:\n");
    let yml = format!("{yml}# my own notes\nglobal:\n  upload:\n    slots: 3\n");
    std::fs::write(&e.fake.config, &yml).unwrap();
    assert!(read_found(&e.fake.config).unwrap().api_key.is_none());
    let _ = app.call("slskd.repair", "{}");
    let after = std::fs::read_to_string(&e.fake.config).unwrap();
    assert!(read_found(&e.fake.config).unwrap().api_key.is_some(), "{after}");
    assert!(after.contains("# my own notes\nglobal:\n  upload:\n    slots: 3\n"), "{after}");
    assert!(after.contains("      mine:\n"), "{after}");
    let _ = std::fs::remove_dir_all(&e.root);
}
