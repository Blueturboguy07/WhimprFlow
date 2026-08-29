//! WhimprFlow Tauri shell.
//!
//! Runs as a macOS accessory (menu-bar) app: a tray item, a transparent
//! always-on-top Flow Bar overlay, and a hidden Hub window. This is the M0
//! skeleton — the sidecar supervisor, real state-machine bridge, and native
//! panel promotion arrive in later milestones. The overlay already listens for
//! `whimpr://flowbar/state`, so the tray demo items prove the event pipeline.

mod appctx;
mod autolearn;
mod diag;
mod hotkey;
mod local_llm;
mod paste;
#[cfg(target_os = "windows")]
mod win;

use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

const OVERLAY_LABEL: &str = "whimpr_bar";

/// The overlay's size in LOGICAL points — the same values it is built with.
///
/// Deliberately constants rather than reading `outer_size()`: that returns
/// physical pixels for whichever display the window is on *right now*, so
/// dividing it by the scale of the display we are moving TO gave a size that was
/// wrong by the ratio between them. Moving from a 1× screen to the 2× laptop
/// computed the pill as 160×70 instead of 320×132 and parked it under the Dock.
/// The window is not resizable, so its logical size never changes.
/// Size the overlay starts at: just the resting nub, nothing more.
const OVERLAY_IDLE_W_PT: f64 = 100.0;
const OVERLAY_IDLE_H_PT: f64 = 26.0;

/// The overlay's CURRENT logical size.
///
/// The window used to be a fixed 320×132 — big enough for the hover cluster —
/// with a tiny nub drawn inside it. That left a large invisible rectangle above
/// the Dock that swallowed clicks meant for the app underneath, and made the
/// hover UI open whenever the pointer entered anywhere in that rectangle rather
/// than the pill itself. The window is now resized to match whatever is drawn,
/// and this is what the placement maths measures.
static OVERLAY_SIZE: std::sync::Mutex<(f64, f64)> =
    std::sync::Mutex::new((OVERLAY_IDLE_W_PT, OVERLAY_IDLE_H_PT));

fn overlay_size() -> (f64, f64) {
    *OVERLAY_SIZE.lock().unwrap()
}
const HUB_LABEL: &str = "main";

#[derive(Clone, Serialize)]
struct BarStatePayload {
    state: &'static str,
}

/// Pick the display the pill should sit on.
///
/// "Active display" is approximated by the one under the mouse pointer, which is
/// where the user is working and, on a hold-to-talk app, where they just pressed
/// the key. Falls back to the primary display, which is what this used to always
/// do — and why the pill only ever appeared on the laptop screen.
/// Scale factor of the primary display, which is the unit `cursor_position` is
/// reported in on macOS (global points multiplied by the primary's scale).
fn primary_scale(w: &WebviewWindow) -> f64 {
    w.primary_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or(1.0)
}

/// A monitor's work area in LOGICAL points.
///
/// Measured from real values on a mixed-DPI setup: `position` comes back in
/// logical points while `size` comes back in physical pixels, so the size has to
/// be divided by that monitor's own scale to get a coherent rectangle.
fn work_area_logical(m: &tauri::Monitor) -> (f64, f64, f64, f64) {
    let a = m.work_area();
    whimpr_core::settings::work_area_points(
        m.position().y,
        a.position.x,
        a.position.y,
        a.size.width,
        a.size.height,
        m.scale_factor(),
    )
}

/// A monitor's FULL bounds in logical points, for the cursor hit-test.
fn monitor_logical_rect(m: &tauri::Monitor) -> (f64, f64, f64, f64) {
    let s = if m.scale_factor() > 0.0 { m.scale_factor() } else { 1.0 };
    let p = m.position();
    let sz = m.size();
    (
        p.x as f64,
        p.y as f64,
        sz.width as f64 / s,
        sz.height as f64 / s,
    )
}

/// One-shot dump of every display, so the geometry can be checked rather than
/// inferred from where the pill ended up.
fn log_monitor_table(w: &WebviewWindow) {
    let Ok(monitors) = w.available_monitors() else { return };
    eprintln!("[whimpr] --- displays (primary scale {:.1}) ---", primary_scale(w));
    for m in monitors {
        let p = m.position();
        let s = m.size();
        let a = m.work_area();
        eprintln!(
            "[whimpr]   \"{}\" scale {:.1} pos({},{}) size({}x{}) work(pos {},{} size {}x{})",
            m.name().cloned().unwrap_or_else(|| "?".into()),
            m.scale_factor(),
            p.x, p.y, s.width, s.height,
            a.position.x, a.position.y, a.size.width, a.size.height
        );
    }
}

fn pill_monitor(w: &WebviewWindow, follow_active: bool) -> Option<tauri::Monitor> {
    if follow_active {
        if let Ok(cursor) = w.app_handle().cursor_position() {
            // Everything is compared in LOGICAL points. `cursor_position` arrives
            // as logical × the primary's scale, so divide that out first;
            // `monitor_from_point` was tried and returned a different display on
            // alternating ticks, which is what made the pill flap.
            let ps = primary_scale(w);
            let (lx, ly) = (cursor.x / ps, cursor.y / ps);

            if let Ok(monitors) = w.available_monitors() {
                let hit = monitors.into_iter().find(|m| {
                    let (x0, y0, wl, hl) = monitor_logical_rect(m);
                    lx >= x0 && lx < x0 + wl && ly >= y0 && ly < y0 + hl
                });
                if hit.is_some() {
                    return hit;
                }
            }
            eprintln!(
                "[whimpr] cursor ({:.0},{:.0}) -> logical ({:.0},{:.0}) matched no monitor",
                cursor.x, cursor.y, lx, ly
            );
        } else {
            eprintln!("[whimpr] cursor position unavailable — staying on the primary");
        }
    }
    // current_monitor() can be None before the window maps; fall back sensibly.
    w.primary_monitor()
        .ok()
        .flatten()
        .or_else(|| w.current_monitor().ok().flatten())
        .or_else(|| w.available_monitors().ok().and_then(|m| m.into_iter().next()))
}

/// Anchor the overlay bottom-center of the target display's **work area**.
///
/// The work area excludes the Dock and the menu bar. The previous version used
/// the full monitor rect minus a fixed 40pt inset, which put the pill squarely
/// underneath the Dock on any Mac with the Dock on the bottom.
/// Where the pill *should* be right now, in physical pixels.
///
/// Split out from `position_overlay` so the watcher can compare it against the
/// window's actual position every tick. Change-detection on "which monitor" or
/// "what work area" turned out to be too clever: any state it failed to predict
/// left the pill stranded until the next dictation. Comparing against the real
/// position instead means it corrects itself no matter how it got out of place.
fn desired_overlay_position(
    w: &WebviewWindow,
    settings: &whimpr_core::Settings,
) -> Option<tauri::LogicalPosition<f64>> {
    let following = settings.pill_follows_active_display;
    let monitor = pill_monitor(w, following)?;
    let (ax, ay, aw, ah) = work_area_logical(&monitor);
    let (ww, wh) = overlay_size();

    let (x, y) = match settings.pill_pos {
        // Still clamped, so a stale pinned position can't hide under the Dock.
        Some((px, py)) if !following => (
            (px as f64).clamp(ax, (ax + aw - ww).max(ax)),
            (py as f64).clamp(ay, (ay + ah - wh).max(ay)),
        ),
        _ => whimpr_core::settings::pill_placement(
            (ax, ay, aw, ah),
            ww,
            wh,
            settings.pill_bottom_inset,
        ),
    };

    Some(tauri::LogicalPosition::new(x, y))
}

fn position_overlay(w: &WebviewWindow) {
    let settings = hotkey::current_settings();

    let Some(pos) = desired_overlay_position(w, &settings) else {
        eprintln!("[whimpr] no monitor found — overlay stays at default position");
        return;
    };
    let _ = w.set_position(pos);
    eprintln!("[whimpr] overlay placed at logical ({:.0},{:.0})", pos.x, pos.y);
}

/// Show or hide the pill for the current dictation state, honouring the
/// "show at all times" setting. Also re-anchors at the start of a session so the
/// pill lands on whichever display the user is currently on.
pub(crate) fn sync_pill_visibility(app: &tauri::AppHandle, state: &str) {
    let Some(w) = app.get_webview_window(OVERLAY_LABEL) else {
        return;
    };
    let settings = hotkey::current_settings();
    let idle = state == "idle";

    if state == "recording" {
        position_overlay(&w);
    }
    if settings.show_pill_always || !idle {
        let _ = w.show();
    } else {
        let _ = w.hide();
    }
}

/// Resize the overlay to fit what the pill is currently drawing, then re-anchor
/// it so it stays bottom-centred above the Dock as it grows and shrinks.
///
/// Called from the UI because only the UI knows how big the content is. Sizes
/// are logical points.
#[tauri::command]
fn set_overlay_size(app: tauri::AppHandle, width: f64, height: f64) {
    // Guard against a nonsense size leaving an unclickable or screen-filling
    // window if the UI ever miscalculates.
    let w = width.clamp(40.0, 900.0);
    let h = height.clamp(16.0, 400.0);

    {
        let mut cur = OVERLAY_SIZE.lock().unwrap();
        if (cur.0 - w).abs() < 0.5 && (cur.1 - h).abs() < 0.5 {
            return; // already this size
        }
        *cur = (w, h);
    }

    if let Some(win) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = win.set_size(tauri::LogicalSize::new(w, h));
        // Re-anchor immediately: growing upward from a bottom-anchored window
        // means the position has to change too, or the pill appears to slide
        // down as it expands.
        position_overlay(&win);
    }
}

// ── Pill controls ────────────────────────────────────────────────────────────
/// Discard the in-flight dictation (the pill's ✕).
#[tauri::command]
fn pill_cancel() {
    hotkey::ui_cancel();
}

/// Finish now and insert what has been said (the pill's ■).
#[tauri::command]
fn pill_stop() {
    hotkey::ui_stop();
}

/// Begin a hands-free dictation (clicking the idle pill).
#[tauri::command]
fn pill_start() {
    hotkey::ui_start();
}

// ── Transforms ───────────────────────────────────────────────────────────────
#[tauri::command]
fn get_transforms() -> Vec<whimpr_core::Transform> {
    hotkey::transforms()
}

#[tauri::command]
fn set_transform_enabled(id: String, enabled: bool) {
    hotkey::transform_set_enabled(&id, enabled);
}

// ── Snippets ─────────────────────────────────────────────────────────────────
#[tauri::command]
fn get_snippets() -> Vec<whimpr_core::Snippet> {
    hotkey::snippets()
}

#[tauri::command]
fn add_snippet(trigger: String, expansion: String) {
    hotkey::snippet_add(trigger, expansion);
}

#[tauri::command]
fn remove_snippet(trigger: String) {
    hotkey::snippet_remove(&trigger);
}

// ── Scratchpad ───────────────────────────────────────────────────────────────
/// While capture is on, finished dictations are appended to the Scratchpad
/// instead of being pasted into whatever app happens to be frontmost.
#[tauri::command]
fn set_scratchpad_capture(on: bool) {
    hotkey::set_scratchpad_capture(on);
}

#[tauri::command]
fn get_scratchpad_capture() -> bool {
    hotkey::scratchpad_capture()
}

#[tauri::command]
fn get_scratchpad() -> String {
    std::fs::read_to_string(hotkey::scratchpad_path()).unwrap_or_default()
}

#[tauri::command]
fn set_scratchpad(text: String) -> Result<(), String> {
    let path = hotkey::scratchpad_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// Input device names for the microphone picker.
#[tauri::command]
fn list_microphones() -> Vec<String> {
    whimpr_audio::input_device_names()
}

/// Install or remove the login item.
///
/// Deliberately a LaunchAgent that shells out to `open -a` rather than launching
/// the executable directly: LaunchServices must be the one starting the app, or
/// macOS attributes Accessibility and Microphone to launchd instead of to
/// WhimprFlow and every permission silently reads as denied.
#[cfg(target_os = "macos")]
fn apply_launch_at_login(enabled: bool) {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = std::path::PathBuf::from(&home).join("Library/LaunchAgents");
    let plist = dir.join("com.whimpr.whimprflow.plist");

    if !enabled {
        let _ = std::fs::remove_file(&plist);
        return;
    }
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.whimpr.whimprflow</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/open</string>
        <string>-a</string>
        <string>/Applications/WhimprFlow.app</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#;
    if let Err(e) = std::fs::write(&plist, body) {
        eprintln!("[whimpr] could not write the login item: {e}");
    }
}

/// Show or hide the Dock icon. Accessory = menu-bar-only.
#[cfg(target_os = "macos")]
fn apply_dock_visibility(app: &tauri::AppHandle, show: bool) {
    let policy = if show {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    };
    if let Err(e) = app.set_activation_policy(policy) {
        eprintln!("[whimpr] could not change the activation policy: {e}");
    }
}

/// Quit and relaunch. Needed because macOS resolves microphone authorisation
/// once at process start, so a grant made while running is invisible until then.
#[tauri::command]
fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

/// Persist a pill position the user dragged to (physical pixels, window top-left).
#[tauri::command]
fn set_pill_position(x: i32, y: i32) {
    let mut settings = hotkey::current_settings();
    settings.pill_pos = Some((x, y));
    // Deliberately dragging the pill somewhere means you want it to stay there,
    // so stop following the cursor between displays — otherwise the drag would
    // appear to do nothing the next time you moved screens.
    settings.pill_follows_active_display = false;
    hotkey::update_settings(settings);
}

/// Keep the pill on whichever display the pointer is on.
///
/// `position_overlay` previously ran only at startup, on a settings change and at
/// the start of a dictation — so moving to another screen left the pill behind
/// until you next spoke, which read as "follow active display doesn't work".
fn spawn_display_watcher(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        // Name of the display the pill was last placed on.
        let mut last_target: Option<String> = None;
        loop {
            // Short enough that moving to another screen feels like the pill came
            // with you, rather than catching up a beat later.
            std::thread::sleep(std::time::Duration::from_millis(220));

            let settings = hotkey::current_settings();
            if !settings.pill_follows_active_display {
                continue;
            }
            let Some(w) = app.get_webview_window(OVERLAY_LABEL) else {
                continue;
            };
            let Some(monitor) = pill_monitor(&w, true) else { continue };

            // Move when the target display changes OR when that display's work
            // area does. Both matter and each was fixed at the cost of the other
            // in earlier attempts: the display covers "I moved screens", the work
            // area covers "the Dock just arrived here". Neither changes as the
            // cursor wanders within one screen, so the pill stays put.
            let a = monitor.work_area();
            let key = format!(
                "{}|{},{},{},{}",
                monitor.name().cloned().unwrap_or_default(),
                a.position.x,
                a.position.y,
                a.size.width,
                a.size.height
            );
            if last_target.as_deref() == Some(key.as_str()) {
                continue;
            }
            let Some(want) = desired_overlay_position(&w, &settings) else {
                continue;
            };
            last_target = Some(key.clone());
            let _ = w.set_position(want);

            // Verify rather than assume. `set_position` takes a LogicalPosition,
            // but which space tao maps that into on a mixed-DPI setup is exactly
            // what has been guessed wrong before — so read back where the window
            // actually landed, and on which display.
            std::thread::sleep(std::time::Duration::from_millis(60));
            let landed = w
                .current_monitor()
                .ok()
                .flatten()
                .and_then(|m| m.name().cloned())
                .unwrap_or_else(|| "?".into());
            let at = w
                .outer_position()
                .map(|p| format!("{},{}", p.x, p.y))
                .unwrap_or_else(|_| "?".into());
            eprintln!(
                "[whimpr] MOVE want logical({:.0},{:.0}) target[{}] -> landed on \"{}\" at physical({})",
                want.x, want.y, key, landed, at
            );
        }
    });
}

/// Forget the dragged position and go back to the computed anchor.
#[tauri::command]
fn reset_pill_position(app: tauri::AppHandle) {
    let mut settings = hotkey::current_settings();
    settings.pill_pos = None;
    // Dragging turns following off, so resetting has to turn it back on —
    // otherwise the pill goes back to bottom-centre once and then stops
    // following you between displays, which looks like the reset half-worked.
    settings.pill_follows_active_display = true;
    hotkey::update_settings(settings);
    if let Some(w) = app.get_webview_window(OVERLAY_LABEL) {
        position_overlay(&w);
    }
}

fn build_overlay(app: &tauri::App) -> tauri::Result<WebviewWindow> {
    let overlay = WebviewWindowBuilder::new(
        app,
        OVERLAY_LABEL,
        WebviewUrl::App("overlay.html".into()),
    )
    .title("WhimprBar")
    // Sized for the hover cluster: a "Dictate fn" tooltip stacked above a row of
    // action buttons. The content is bottom-aligned, so the resting nub still sits
    // at the bottom edge and everything above it is transparent until you hover.
    .inner_size(OVERLAY_IDLE_W_PT, OVERLAY_IDLE_H_PT)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .resizable(false)
    .visible(true)
    .build()?;
    // Deliberately NOT positioned here: settings (including any dragged position)
    // are only loaded once hotkey::install runs, so setup() places it afterwards.
    Ok(overlay)
}

fn build_hub(app: &tauri::App) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, HUB_LABEL, WebviewUrl::App("index.html".into()))
        .title("WhimprFlow")
        .inner_size(920.0, 640.0)
        .min_inner_size(720.0, 480.0)
        .visible(true)
        .build()
}

fn emit_bar_state(app: &tauri::AppHandle, state: &'static str) {
    let _ = app.emit_to(OVERLAY_LABEL, "whimpr://flowbar/state", BarStatePayload { state });
}

#[tauri::command]
fn get_settings() -> whimpr_core::Settings {
    hotkey::current_settings()
}

#[tauri::command]
fn set_settings(app: tauri::AppHandle, settings: whimpr_core::Settings) {
    #[cfg(target_os = "macos")]
    {
        apply_launch_at_login(settings.launch_at_login);
        apply_dock_visibility(&app, settings.show_in_dock);
    }
    hotkey::update_settings(settings);
    // Pill geometry/visibility live in the same settings blob, so re-apply both
    // immediately rather than waiting for the next dictation.
    if let Some(w) = app.get_webview_window(OVERLAY_LABEL) {
        position_overlay(&w);
    }
    sync_pill_visibility(&app, "idle");
}

/// Aggregated dictation stats for the Hub dashboard. `tz_offset_minutes` is the
/// browser's `Date.getTimezoneOffset()` so "today"/streak match the user's clock.
#[tauri::command]
fn get_stats(tz_offset_minutes: i32) -> whimpr_core::StatsSummary {
    hotkey::stats_summary(tz_offset_minutes)
}

/// Recent dictations for the Hub Home history list (newest first).
#[tauri::command]
fn get_history() -> Vec<whimpr_core::HistoryItem> {
    hotkey::history(200)
}

/// Copy arbitrary text to the system clipboard, for the Hub's history "Copy"
/// button. Note this is a plain set — unlike `paste::paste_text`, which restores
/// the previous clipboard afterwards, here the user explicitly asked for the
/// text to stay on the clipboard.
#[tauri::command]
fn copy_to_clipboard(text: String) -> Result<(), String> {
    use arboard::Clipboard;
    let mut cb = Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_text(text).map_err(|e| e.to_string())
}

/// Dictionary entries for the Hub Dictionary screen.
#[tauri::command]
fn get_dictionary() -> Vec<hotkey::DictEntryDto> {
    hotkey::dictionary_entries()
}

/// Add a manual dictionary entry (word + optional known mishears).
#[tauri::command]
fn add_dictionary_entry(correct: String, mishears: Vec<String>) {
    hotkey::dictionary_add(correct, mishears);
}

/// Remove a dictionary entry by its spelling.
#[tauri::command]
fn remove_dictionary_entry(correct: String) {
    hotkey::dictionary_remove(&correct);
}

/// Permission + capability status shown in the Hub.
#[derive(Clone, Serialize)]
struct StatusReport {
    accessibility: bool,
    microphone: bool,
    input_monitoring: bool,
    has_openai_key: bool,
    has_anthropic_key: bool,
}

#[tauri::command]
fn get_status() -> StatusReport {
    StatusReport {
        accessibility: paste::is_trusted(),
        microphone: paste::microphone_granted(),
        input_monitoring: paste::input_monitoring_granted(),
        has_openai_key: has_key("openai_api_key"),
        has_anthropic_key: has_key("anthropic_api_key"),
    }
}

/// The most recent loud diagnostic (permission/injection failure), if any —
/// lets the Hub show what went wrong even if it was opened after the fact.
/// See `diag::report`, called from the dictation pipeline whenever text
/// fails to reach the cursor.
#[tauri::command]
fn get_last_error() -> Option<diag::ErrorDto> {
    diag::last_error()
}

fn has_key(account: &str) -> bool {
    keyring::Entry::new("com.whimpr.whimprflow", account)
        .ok()
        .and_then(|e| e.get_password().ok())
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

/// Request microphone access: trigger the native prompt (bundle has a usage string)
/// by briefly opening the input device, and open the Microphone settings pane.
#[tauri::command]
fn request_microphone() {
    #[cfg(target_os = "macos")]
    {
        std::thread::spawn(|| {
            if let Ok(h) = whimpr_audio::start(|_: &[f32]| {}) {
                std::thread::sleep(std::time::Duration::from_millis(400));
                let _ = h.stop();
            }
        });
        open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone");
    }
}

/// Request Accessibility — the permission that makes the Fn key work in every app and
/// lets us type into other apps. Fire the native prompt, then open the pane.
#[tauri::command]
fn request_accessibility() {
    #[cfg(target_os = "macos")]
    {
        let _ = paste::prompt_accessibility();
        open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
    }
}

/// Request Input Monitoring (needed for the Fn key to be seen in every app, not
/// just while WhimprFlow is frontmost): register + prompt, then open the pane.
#[tauri::command]
fn request_input_monitoring() {
    #[cfg(target_os = "macos")]
    {
        let _ = paste::request_input_monitoring();
        open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent");
    }
}

/// Save (or clear, when empty) an API key in the OS keychain, then rebuild providers
/// so it takes effect immediately.
#[tauri::command]
fn set_api_key(provider: String, key: String) -> Result<(), String> {
    let account = match provider.as_str() {
        "openai" => "openai_api_key",
        "anthropic" => "anthropic_api_key",
        _ => return Err(format!("unknown provider {provider}")),
    };
    let entry =
        keyring::Entry::new("com.whimpr.whimprflow", account).map_err(|e| e.to_string())?;
    let key = key.trim();
    // Delete any existing item first so the new one is created by (and readable to)
    // this app — a key added via the `security` CLI isn't readable by the app.
    let _ = entry.delete_credential();
    if !key.is_empty() {
        entry.set_password(key).map_err(|e| e.to_string())?;
    }
    hotkey::rebuild_providers();
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            get_stats,
            get_history,
            copy_to_clipboard,
            restart_app,
            list_microphones,
            pill_cancel,
            pill_stop,
            pill_start,
            set_overlay_size,
            get_transforms,
            set_transform_enabled,
            get_snippets,
            add_snippet,
            remove_snippet,
            set_scratchpad_capture,
            get_scratchpad_capture,
            get_scratchpad,
            set_scratchpad,
            set_pill_position,
            reset_pill_position,
            get_dictionary,
            add_dictionary_entry,
            remove_dictionary_entry,
            get_status,
            get_last_error,
            request_microphone,
            request_accessibility,
            request_input_monitoring,
            set_api_key
        ])
        .setup(|app| {
            // Regular app: shows in the Dock with a normal, focusable main window.
            // (Can switch to a menu-bar-only accessory app later for the Wispr look.)
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular);

            let overlay = build_overlay(app)?;
            let hub = build_hub(app)?;
            let _ = hub.show();
            let _ = hub.set_focus();

            // Wire the Fn key to the pill via the real state machine. This also
            // loads settings.json, so the pill can be placed from here on.
            hotkey::install(app.handle().clone());

            log_monitor_table(&overlay);
            position_overlay(&overlay);
            sync_pill_visibility(app.handle(), "idle");
            spawn_display_watcher(app.handle().clone());

            // Settings exist by now, so honour the saved Dock preference and keep
            // the login item in sync with it.
            #[cfg(target_os = "macos")]
            {
                let mut s = hotkey::current_settings();
                apply_dock_visibility(app.handle(), s.show_in_dock);
                apply_launch_at_login(s.launch_at_login);

                // A saved pixel position only means anything on the display it was
                // set on, and it is ignored while following is enabled — so having
                // both set is a contradiction that leaves the pill looking stuck.
                // Drop the stale one on launch.
                if s.pill_follows_active_display && s.pill_pos.is_some() {
                    eprintln!("[whimpr] clearing a stale pinned pill position");
                    s.pill_pos = None;
                    hotkey::update_settings(s);
                }
            }

            let open = MenuItem::with_id(app, "open", "Open WhimprFlow", true, None::<&str>)?;
            let demo_rec =
                MenuItem::with_id(app, "demo_rec", "Demo: recording", true, None::<&str>)?;
            let demo_idle = MenuItem::with_id(app, "demo_idle", "Demo: idle", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit WhimprFlow", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &demo_rec, &demo_idle, &sep, &quit])?;

            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(w) = app.get_webview_window(HUB_LABEL) {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "demo_rec" => emit_bar_state(app, "recording"),
                    "demo_idle" => emit_bar_state(app, "idle"),
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon().cloned() {
                tray = tray.icon(icon);
            }
            tray.build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running WhimprFlow");
}
