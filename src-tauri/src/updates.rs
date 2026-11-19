//! Signed self-update (tauri-plugin-updater). Written to be copied to another Tauri 2 app:
//! change the values under "Per app" and follow the checklist in docs/updates.md.
//!
//! What it does
//! - Checks the feed 10 s after launch, then once a day while the app runs. The day is measured
//!   on the wall clock, so a Mac that slept through the day checks soon after it wakes.
//!   `PUBLIK_NO_AUTO_UPDATE=1` turns the automatic checks off (CI, managed machines).
//! - "Check for Updates…" in the tray menu runs the same check by hand.
//! - Every package is checked against the public key in tauri.conf.json before it is used, and
//!   its signature must name the version the feed announces (`requireSignedVersion`).
//! - macOS: the new .app is downloaded, verified and swapped in place in the background. Then the
//!   app asks to restart. When this user cannot replace the app (installed by another user), the
//!   swap waits for "Install and Restart", because it then asks for an admin password.
//!   Windows: the installer is downloaded and verified in the background. Then the app asks, and
//!   "Restart and Install" runs the installer in passive mode (a progress bar, no questions): it
//!   closes the app, updates it and opens it again. Nothing restarts without a yes.
//! - A background check that fails writes to the log (stderr, `[whimpr] updates:`) only. A manual
//!   check shows the error.
//!
//! Build-time override: `PUBLIK_UPDATE_ENDPOINT` (one URL or a comma-separated list, read with
//! `option_env!`) replaces the endpoints in tauri.conf.json, so a test build can use a test feed.
//!
//! Test hook: `PUBLIK_UPDATE_SELF_TEST=version|check|install` starts the app with no window and
//! no prompt, runs the same check/download/verify/install code, writes one JSON line to stdout
//! and appends it to the file named by `PUBLIK_UPDATE_SELF_TEST_OUT`, then exits.
//! Exit codes: 0 = done (installed, up to date, or version), 2 = the update failed or was refused.
//! `prompt` runs the background check as it is and leaves the restart question open (the test
//! looks at it and closes the app).

use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

// ---- Per app ----------------------------------------------------------------------------------
const APP_NAME: &str = "WhimprFlow";
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the background loop looks at the wall clock.
const TICK: Duration = Duration::from_secs(30 * 60);
// ------------------------------------------------------------------------------------------------

const NO_AUTO_ENV: &str = "PUBLIK_NO_AUTO_UPDATE";
const SELF_TEST_ENV: &str = "PUBLIK_UPDATE_SELF_TEST";
const SELF_TEST_OUT_ENV: &str = "PUBLIK_UPDATE_SELF_TEST_OUT";
/// Set at build time only. A running app never reads its endpoint from the environment.
const ENDPOINT_OVERRIDE: Option<&str> = option_env!("PUBLIK_UPDATE_ENDPOINT");

/// An update that is downloaded and verified.
struct Ready {
    version: String,
    update: Update,
    /// The verified package while it is not installed yet: always on Windows (the installer runs
    /// on "Restart and Install"), and on macOS when this user cannot replace the app in place.
    /// None: installed in the background, a restart is all that is left.
    package: Option<Vec<u8>>,
}

static READY: Mutex<Option<Ready>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);
static ASKING: AtomicBool = AtomicBool::new(false);

enum Found {
    UpToDate(String),
    Ready(String),
}

/// The plugins the updater needs. Call on the Builder before `.setup`.
pub fn plugins<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.plugin(tauri_plugin_updater::Builder::new().build()).plugin(tauri_plugin_dialog::init())
}

/// Start the automatic checks: 10 s after launch, then once a day. Call from `setup`.
pub fn start(app: &AppHandle) {
    if std::env::var(NO_AUTO_ENV).is_ok_and(|v| !v.is_empty() && v != "0") {
        eprintln!("[whimpr] updates: automatic checks are off ({NO_AUTO_ENV})");
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(daily_checks(SystemTime::now, move || check(app.clone(), false)));
}

/// The schedule of the automatic checks: the first one FIRST_CHECK_AFTER after launch, then one
/// each time a day has passed on `clock` (the wall clock in the app), looked at every TICK. Runs
/// until the app exits. The clock is a parameter so the test can move it.
async fn daily_checks<C, F, Fut>(clock: C, mut run_check: F)
where
    C: Fn() -> SystemTime,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    tokio::time::sleep(FIRST_CHECK_AFTER).await;
    let mut last: Option<SystemTime> = None;
    loop {
        // A clock that moved back gives an error here; that counts as due.
        let now = clock();
        if last.map_or(true, |t| now.duration_since(t).map_or(true, |e| e >= CHECK_EVERY)) {
            last = Some(now);
            run_check().await;
        }
        tokio::time::sleep(TICK).await;
    }
}

/// "Check for Updates…": the same check, with its result shown.
pub fn check_now(app: &AppHandle) {
    // An update that is already downloaded (the person chose "Later") is offered again at once.
    let ready = READY.lock().unwrap().as_ref().map(|r| r.version.clone());
    if let Some(version) = ready {
        ask_to_restart(app, &version);
        return;
    }
    tauri::async_runtime::spawn(check(app.clone(), true));
}

async fn check(app: AppHandle, manual: bool) {
    if BUSY.swap(true, Ordering::SeqCst) {
        if manual {
            tell(&app, &format!("{APP_NAME} is already checking for updates."), false);
        }
        return;
    }
    let found = fetch(&app).await;
    BUSY.store(false, Ordering::SeqCst);
    match found {
        Ok(Found::Ready(version)) => {
            eprintln!("[whimpr] updates: {APP_NAME} {version} is ready");
            ask_to_restart(&app, &version);
        }
        Ok(Found::UpToDate(current)) => {
            eprintln!("[whimpr] updates: {current} is the latest version");
            if manual {
                tell(&app, &format!("{APP_NAME} {current} is the latest version."), false);
            }
        }
        Err(e) => {
            eprintln!("[whimpr] updates: the check failed: {e}");
            if manual {
                tell(&app, &format!("{APP_NAME} could not check for updates.\n\n{e}"), true);
            }
        }
    }
}

/// The updater, with the build-time endpoint override applied.
fn updater(app: &AppHandle) -> Result<Updater, String> {
    let mut builder = app.updater_builder();
    if let Some(list) = ENDPOINT_OVERRIDE.map(str::trim).filter(|s| !s.is_empty()) {
        let urls = list
            .split(',')
            .map(|u| tauri::Url::parse(u.trim()).map_err(|e| format!("PUBLIK_UPDATE_ENDPOINT {u}: {e}")))
            .collect::<Result<Vec<_>, _>>()?;
        builder = builder.endpoints(urls).map_err(|e| e.to_string())?;
    }
    builder.build().map_err(|e| e.to_string())
}

/// Check, download and verify; on macOS also install when this user can replace the app. The one
/// code path behind the automatic check, the menu item and the self-test.
async fn fetch(app: &AppHandle) -> Result<Found, String> {
    let Some(update) = updater(app)?.check().await.map_err(|e| e.to_string())? else {
        return Ok(Found::UpToDate(app.package_info().version.to_string()));
    };
    let version = update.version.clone();
    if READY.lock().unwrap().as_ref().is_some_and(|r| r.version == version) {
        return Ok(Found::Ready(version));
    }
    // `download` verifies the signature (and the signed version) before it returns the bytes.
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    let package = if can_replace_in_place() {
        // Verifies again, then swaps the new bundle in place. The running app is not touched.
        update.install(&bytes).map_err(|e| e.to_string())?;
        None
    } else {
        Some(bytes)
    };
    #[cfg(not(target_os = "macos"))]
    let package = Some(bytes);
    *READY.lock().unwrap() = Some(Ready { version: version.clone(), update, package });
    Ok(Found::Ready(version))
}

/// macOS: true when this user can swap the app bundle without an admin password (write access
/// to the bundle and to its folder). Otherwise the plugin would ask for one, which must not
/// happen in a background check.
#[cfg(target_os = "macos")]
fn can_replace_in_place() -> bool {
    use std::os::raw::{c_char, c_int};
    use std::os::unix::ffi::OsStrExt;
    extern "C" {
        fn access(path: *const c_char, mode: c_int) -> c_int;
    }
    const W_OK: c_int = 2;
    let writable = |p: &std::path::Path| {
        std::ffi::CString::new(p.as_os_str().as_bytes()).is_ok_and(|c| unsafe { access(c.as_ptr(), W_OK) } == 0)
    };
    // .../WhimprFlow.app/Contents/MacOS/<binary>
    let Ok(exe) = std::env::current_exe() else { return false };
    match exe.ancestors().nth(3) {
        Some(bundle) if bundle.extension().is_some_and(|e| e == "app") => writable(bundle) && bundle.parent().is_some_and(writable),
        _ => false,
    }
}

fn pending_install() -> bool {
    READY.lock().unwrap().as_ref().is_some_and(|r| r.package.is_some())
}

fn ask_to_restart(app: &AppHandle, version: &str) {
    if ASKING.swap(true, Ordering::SeqCst) {
        return;
    }
    let (message, yes) = if cfg!(windows) {
        (format!("{APP_NAME} {version} is ready. {APP_NAME} closes, installs it and opens again."), "Restart and Install")
    } else if pending_install() {
        (format!("{APP_NAME} {version} is ready. {APP_NAME} installs it and restarts. macOS may ask for an administrator password."), "Install and Restart")
    } else {
        (format!("{APP_NAME} {version} is installed. Restart {APP_NAME} to use it."), "Restart")
    };
    let handle = app.clone();
    app.dialog()
        .message(message)
        .title(format!("{APP_NAME} update"))
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(yes.into(), "Later".into()))
        .show(move |restart| {
            ASKING.store(false, Ordering::SeqCst);
            if restart {
                if let Err(e) = restart_into_update(&handle) {
                    eprintln!("[whimpr] updates: {e}");
                    tell(&handle, &format!("{APP_NAME} could not install the update.\n\n{e}"), true);
                }
            }
        });
}

/// Installs a package that is not installed yet, then restarts into the new version.
/// Windows: the installer closes and reopens the app, so this does not return on success.
fn restart_into_update(app: &AppHandle) -> Result<(), String> {
    if pending_install() {
        install_pending(app)?;
    }
    app.restart();
}

/// Installs the downloaded package. Windows: runs the installer on the main thread (so the tray
/// icon is removed first); the process then exits, so this returns only on an error.
fn install_pending(app: &AppHandle) -> Result<(), String> {
    let Some(Ready { update, package: Some(bytes), .. }) = READY.lock().unwrap().take() else {
        return Err("no update is ready".into());
    };
    #[cfg(windows)]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(update.install(&bytes).map_err(|e| e.to_string()));
        })
        .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        update.install(&bytes).map_err(|e| e.to_string())
    }
}

fn tell(app: &AppHandle, message: &str, error: bool) {
    app.dialog()
        .message(message)
        .title(format!("{APP_NAME} update"))
        .kind(if error { MessageDialogKind::Error } else { MessageDialogKind::Info })
        .show(|_| {});
}

/// The self-test mode, when `PUBLIK_UPDATE_SELF_TEST` is set.
pub fn self_test_mode() -> Option<String> {
    std::env::var(SELF_TEST_ENV).ok().filter(|m| !m.is_empty())
}

/// Runs the self-test instead of the app: no window and no prompt (except in `prompt` mode),
/// one JSON report, then exit.
pub fn run_self_test(app: &AppHandle, mode: String) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut report = json!({
            "self_test": mode,
            "version": app.package_info().version.to_string(),
            "pid": std::process::id(),
            "exe": std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
            "endpoint_override": ENDPOINT_OVERRIDE.is_some(),
        });
        #[cfg(target_os = "macos")]
        {
            report["in_place"] = can_replace_in_place().into();
        }
        let code = match mode.as_str() {
            "version" => {
                report["result"] = "version".into();
                0
            }
            "check" => match updater(&app) {
                Err(e) => fail(&mut report, e),
                Ok(u) => match u.check().await {
                    Ok(Some(update)) => {
                        report["result"] = "available".into();
                        report["new_version"] = update.version.into();
                        0
                    }
                    Ok(None) => {
                        report["result"] = "none".into();
                        0
                    }
                    Err(e) => fail(&mut report, e.to_string()),
                },
            },
            "prompt" => match fetch(&app).await {
                Ok(Found::Ready(version)) => {
                    report["result"] = "asking".into();
                    report["new_version"] = version.clone().into();
                    report["install_waits_for_yes"] = pending_install().into();
                    emit(&report);
                    ask_to_restart(&app, &version);
                    return;
                }
                Ok(Found::UpToDate(_)) => {
                    report["result"] = "none".into();
                    0
                }
                Err(e) => fail(&mut report, e),
            },
            "install" => match fetch(&app).await {
                Ok(Found::UpToDate(_)) => {
                    report["result"] = "none".into();
                    0
                }
                Ok(Found::Ready(version)) => {
                    report["new_version"] = version.into();
                    install_without_prompt(&app, &mut report)
                }
                Err(e) => fail(&mut report, e),
            },
            other => fail(&mut report, format!("unknown mode {other:?} (version, check, install, prompt)")),
        };
        emit(&report);
        app.exit(code);
    });
}

/// The "yes" branch of the prompt, without the prompt and without the restart.
fn install_without_prompt(app: &AppHandle, report: &mut Value) -> i32 {
    if pending_install() {
        if cfg!(windows) {
            // Written first: on success the installer takes over and this process exits.
            report["result"] = "installing".into();
            emit(report);
        }
        if let Err(e) = install_pending(app) {
            return fail(report, e);
        }
    }
    report["result"] = "installed".into();
    0
}

fn fail(report: &mut Value, error: String) -> i32 {
    report["result"] = "error".into();
    report["error"] = error.into();
    2
}

fn emit(report: &Value) {
    let line = report.to_string();
    let _ = writeln!(std::io::stdout(), "{line}");
    if let Some(path) = std::env::var_os(SELF_TEST_OUT_ENV) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    const HOUR: Duration = Duration::from_secs(3600);

    /// Runs `daily_checks` on tokio's paused clock (fake timers: time moves only when the test
    /// waits, and at once). The wall clock follows the paused clock, plus `jump` seconds.
    /// `test` gets the start instant and the number of checks so far.
    fn with_fake_time<T, Fut>(jump: Arc<AtomicU64>, test: T)
    where
        T: FnOnce(tokio::time::Instant, Arc<AtomicUsize>) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
        rt.block_on(async move {
            let start = tokio::time::Instant::now();
            let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_000_000);
            let clock = move || base + start.elapsed() + Duration::from_secs(jump.load(Ordering::SeqCst));
            let runs = Arc::new(AtomicUsize::new(0));
            let counter = runs.clone();
            let schedule = tokio::spawn(super::daily_checks(clock, move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async {}
            }));
            test(start, runs).await;
            schedule.abort();
        });
    }

    /// The daily check is scheduled: the first check 10 s after launch, none again within the
    /// day, then one each time 24 hours have passed (seen at the next look at the clock).
    #[test]
    fn the_daily_check_is_scheduled_and_fires_every_24_hours() {
        with_fake_time(Arc::new(AtomicU64::new(0)), |start, runs| async move {
            let count = || runs.load(Ordering::SeqCst);
            tokio::time::sleep_until(start + Duration::from_secs(9)).await;
            assert_eq!(count(), 0, "no check before 10 s");
            tokio::time::sleep_until(start + Duration::from_secs(11)).await;
            assert_eq!(count(), 1, "the first check 10 s after launch");
            tokio::time::sleep_until(start + 23 * HOUR).await;
            assert_eq!(count(), 1, "no second check within the day");
            tokio::time::sleep_until(start + 24 * HOUR + super::FIRST_CHECK_AFTER + super::TICK + Duration::from_secs(60)).await;
            assert_eq!(count(), 2, "the second check after 24 hours");
            tokio::time::sleep_until(start + 47 * HOUR).await;
            assert_eq!(count(), 2, "no third check within the second day");
            tokio::time::sleep_until(start + 48 * HOUR + super::FIRST_CHECK_AFTER + super::TICK * 2 + Duration::from_secs(60)).await;
            assert_eq!(count(), 3, "the third check after 48 hours");
        });
    }

    /// A day that passed on the wall clock while the computer slept counts: the check comes at
    /// the next look at the clock (TICK), not after 24 more hours of running time.
    #[test]
    fn a_day_slept_through_brings_the_check_at_the_next_look_at_the_clock() {
        let jump = Arc::new(AtomicU64::new(0));
        let slept = jump.clone();
        with_fake_time(jump, |start, runs| async move {
            let count = || runs.load(Ordering::SeqCst);
            tokio::time::sleep_until(start + Duration::from_secs(11)).await;
            assert_eq!(count(), 1, "the first check 10 s after launch");
            slept.store(25 * 3600, Ordering::SeqCst);
            tokio::time::sleep_until(start + super::FIRST_CHECK_AFTER + super::TICK + Duration::from_secs(1)).await;
            assert_eq!(count(), 2, "the check at the next look at the clock");
            tokio::time::sleep_until(start + super::FIRST_CHECK_AFTER + super::TICK * 3 + Duration::from_secs(1)).await;
            assert_eq!(count(), 2, "then not again until another day has passed");
        });
    }

    /// The shipped config: the real feed over https, the key, the strict checks. Test builds
    /// change these with `--config` and PUBLIK_UPDATE_ENDPOINT, never in tauri.conf.json.
    #[test]
    fn the_shipped_config_points_at_the_publik_feed_with_strict_checks() {
        let conf: Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let updater = &conf["plugins"]["updater"];
        let endpoints = updater["endpoints"].as_array().unwrap();
        assert!(!endpoints.is_empty());
        for e in endpoints {
            assert!(e.as_str().unwrap().starts_with("https://publikhq.com/api/updates/"), "{e}");
        }
        assert_eq!(updater["requireSignedVersion"], true);
        assert!(updater.get("dangerousInsecureTransportProtocol").is_none());
        assert!(updater.get("allowDowngrades").is_none());
        // Off in the base config, so a source build (README, the publik install guide) works
        // without the update key. The release builds that are not re-signed later (the Windows
        // installer) turn it on with tauri.updater.conf.json; macOS packs after stapling.
        assert_ne!(conf["bundle"]["createUpdaterArtifacts"], true);
        let opt_in: Value = serde_json::from_str(include_str!("../tauri.updater.conf.json")).unwrap();
        assert_eq!(opt_in["bundle"]["createUpdaterArtifacts"], true);
        // base64 of "untrusted comment: minisign public key:", as `tauri signer generate` writes it.
        assert!(updater["pubkey"].as_str().unwrap().starts_with("dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6"));
    }
}
