//! WhimprFlow Tauri shell.
//!
//! Runs as a macOS accessory (menu-bar) app: a tray item, a transparent
//! always-on-top Flow Bar overlay, and a hidden Hub window. This is the M0
//! skeleton — the sidecar supervisor, real state-machine bridge, and native
//! panel promotion arrive in later milestones. The overlay already listens for
//! `whimpr://flowbar/state`, so the tray demo items prove the event pipeline.

mod appctx;
mod asr_model;
mod autolearn;
mod diag;
mod hotkey;
mod local_llm;
mod paste;
mod permissions;
mod publik;
#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "linux")]
mod linux;

use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

const OVERLAY_LABEL: &str = "whimpr_bar";
const HUB_LABEL: &str = "main";

/// The overlay's size in LOGICAL points — matches `build_overlay`'s
/// `.inner_size(300.0, 72.0)`.
///
/// Deliberately a constant rather than reading `outer_size()`: that returns
/// physical pixels for whichever display the window is on *right now*, so
/// dividing it by the scale of the display we are moving TO gave a size that
/// was wrong by the ratio between them. Moving from a 1× screen to a 2× one
/// computed a pill half its real size and parked it under the Dock. The window
/// is not resizable, so its logical size never changes.
const OVERLAY_W_PT: f64 = 300.0;
const OVERLAY_H_PT: f64 = 72.0;
/// Gap in points between the bottom of the screen's *work area* and the pill.
/// The work area already excludes the Dock and menu bar, so this is a small
/// breathing gap, not a Dock allowance.
const OVERLAY_BOTTOM_INSET_PT: f64 = 12.0;

/// Anchor the overlay window bottom-center of its monitor's **work area**
/// (i.e. above the Dock and menu bar, not just inside the monitor's full
/// bounds), correctly converted to logical points on a mixed-DPI setup.
fn position_overlay(w: &WebviewWindow) {
    // current_monitor() can be None before the window maps; fall back sensibly.
    let monitor = w
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| w.current_monitor().ok().flatten())
        .or_else(|| w.available_monitors().ok().and_then(|m| m.into_iter().next()));
    let Some(monitor) = monitor else {
        eprintln!("[whimpr] no monitor found — overlay stays at default position");
        return;
    };
    let work_area = monitor.work_area();
    let area = whimpr_core::settings::work_area_points(
        monitor.position().y,
        work_area.position.x,
        work_area.position.y,
        work_area.size.width,
        work_area.size.height,
        monitor.scale_factor(),
    );
    let (x, y) = whimpr_core::settings::pill_placement(
        area,
        OVERLAY_W_PT,
        OVERLAY_H_PT,
        OVERLAY_BOTTOM_INSET_PT,
    );
    let _ = w.set_position(tauri::LogicalPosition { x, y });
    eprintln!(
        "[whimpr] overlay placed: work area {:?} logical -> window at logical ({:.0},{:.0})",
        area, x, y
    );
}

fn build_overlay(app: &tauri::App) -> tauri::Result<WebviewWindow> {
    let overlay = WebviewWindowBuilder::new(
        app,
        OVERLAY_LABEL,
        WebviewUrl::App("overlay.html".into()),
    )
    .title("WhimprBar")
    // Tight window so it only catches clicks right around the pill, not a big
    // invisible box over the app behind it.
    .inner_size(300.0, 72.0)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .resizable(false)
    // Hidden at rest: the pill only exists while WhimprFlow is actually doing
    // something (recording, cleaning up, flashing done, showing an error). The
    // tray icon is the idle presence. See `emit_flowbar_state`.
    .visible(false)
    .build()?;
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

#[derive(Clone, Serialize)]
struct BarStatePayload {
    state: &'static str,
}

/// Bar states where the pill window must exist. Idle (the rest state) hides it —
/// the overlay is invisible until a dictation actually starts.
fn bar_visible(state: &str) -> bool {
    state != "idle"
}

/// Emit a flow-bar state to the overlay AND toggle its window visibility.
///
/// The single choke point every bar-state producer goes through (the macOS
/// state machine in `hotkey.rs`, the Windows pipeline in `win.rs`, and the
/// diagnostics path in `diag.rs`), so the pill's on-screen existence can
/// never drift out of sync with the state it shows.
pub fn emit_flowbar_state(app: &tauri::AppHandle, state: &'static str) {
    let _ = app.emit_to(OVERLAY_LABEL, "whimpr://flowbar/state", BarStatePayload { state });
    if let Some(w) = app.get_webview_window(OVERLAY_LABEL) {
        if bar_visible(state) {
            // Re-anchor right before showing: the window may have never been
            // mapped, or the screen layout may have changed while hidden.
            position_overlay(&w);
            let _ = w.show();
        } else {
            let _ = w.hide();
        }
    }
}

#[tauri::command]
fn get_settings() -> whimpr_core::Settings {
    hotkey::current_settings()
}

#[tauri::command]
fn set_settings(app: tauri::AppHandle, settings: whimpr_core::Settings) {
    hotkey::update_settings(settings);
    // The hands-free hotkey may have changed — re-register it from the new
    // settings so a customized combo takes effect without a relaunch.
    apply_hands_free_shortcut(&app);
}

/// Stop and finalize the current recording — the overlay pill's red Stop button.
#[tauri::command]
fn stop_dictation() {
    hotkey::stop_dictation();
}

/// Discard the current recording — the overlay pill's ✕ button.
#[tauri::command]
fn cancel_dictation() {
    hotkey::cancel_dictation();
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
///
/// The permission half is a live read every time (see `permissions::snapshot`);
/// nothing here is remembered between calls. The Hub no longer has to ask for it
/// on a timer either — `permissions::watch` pushes the same shape at it on
/// `whimpr://permissions` the moment macOS changes its mind.
#[derive(Clone, Serialize)]
struct StatusReport {
    accessibility: bool,
    microphone: bool,
    input_monitoring: bool,
    microphone_grant: permissions::Grant,
    charged_to: Option<String>,
    microphone_hint: Option<String>,
    has_openai_key: bool,
    has_anthropic_key: bool,
    /// A publik API key is present (env, keychain, or the convention file).
    has_publik_key: bool,
    /// The on-device cleanup worker AND a model are on disk — when false,
    /// "Local" pastes the transcript as spoken and Settings says so.
    local_model_present: bool,
}

#[tauri::command]
fn get_status() -> StatusReport {
    let p = permissions::snapshot();
    StatusReport {
        accessibility: p.accessibility,
        microphone: p.microphone,
        input_monitoring: p.input_monitoring,
        microphone_grant: p.microphone_grant,
        charged_to: p.charged_to,
        microphone_hint: p.microphone_hint,
        has_openai_key: has_key("openai_api_key"),
        has_anthropic_key: has_key("anthropic_api_key"),
        has_publik_key: publik::read_key().is_some(),
        local_model_present: local_llm::worker_bin_path().is_some() && local_llm::model_path().exists(),
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

/// The keychain account for a BYO provider; anything else is refused.
fn byo_account(provider: &str) -> Result<&'static str, String> {
    match provider {
        "openai" => Ok("openai_api_key"),
        "anthropic" => Ok("anthropic_api_key"),
        _ => Err(format!("unknown provider {provider}")),
    }
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

#[cfg(target_os = "linux")]
fn open_url(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

#[cfg(target_os = "windows")]
fn open_url(url: &str) {
    let _ = std::process::Command::new("cmd").args(["/c", "start", url]).spawn();
}

/// Request microphone access: on macOS, trigger the native AVFoundation prompt so
/// the bundle registers in Privacy & Security, then open the Microphone settings
/// pane; on Linux, briefly open the input device (there is no permission prompt)
/// and open pavucontrol as the closest equivalent.
#[tauri::command]
fn request_microphone() {
    #[cfg(target_os = "macos")]
    {
        paste::request_microphone_access();
        open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone");
    }
    #[cfg(target_os = "linux")]
    {
        // On Linux, microphone access is controlled by PipeWire/PulseAudio.
        // Opening pavucontrol is the closest equivalent to macOS's permission pane.
        std::thread::spawn(|| {
            if let Ok(h) = whimpr_audio::start(|_: &[f32]| {}) {
                std::thread::sleep(std::time::Duration::from_millis(400));
                let _ = h.stop();
            }
        });
        let _ = std::process::Command::new("pavucontrol").spawn();
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
    #[cfg(target_os = "linux")]
    {
        // On Linux, keyboard input access is via the 'input' group.
        // Open the system settings or print instructions.
        eprintln!("[whimpr] Linux: ensure your user is in the 'input' group: sudo usermod -aG input $USER");
        let _ = open_url("https://wiki.archlinux.org/title/Input_device");
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
    #[cfg(target_os = "linux")]
    {
        eprintln!("[whimpr] Linux: input monitoring is handled via rdev (needs 'input' group membership)");
    }
}

/// Save (or clear, when empty) an API key in the OS keychain, then rebuild providers
/// so it takes effect immediately. Accepts exactly the two BYO providers: no UI
/// path can write a publik key here or overwrite a user key with one
/// (`publik::ACCOUNT` is managed only by `publik.rs`).
#[tauri::command]
fn set_api_key(provider: String, key: String) -> Result<(), String> {
    let account = byo_account(&provider)?;
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

/// Show and focus the Hub window, whether it's hidden (closed via the X button,
/// which we intercept below) or just needs to come to the front. Used by both
/// the tray's "Open WhimprFlow" item and a relaunch caught by the single-instance
/// guard (Windows/Linux).
fn show_hub(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window(HUB_LABEL) {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// (Re)register the customizable hands-free global hotkey from the current
/// settings — press once to start hands-free dictation, again to stop. Called at
/// startup and whenever settings change. Best-effort: an unregisterable or empty
/// accelerator just leaves the hotkey off (Fn push-to-talk and double-tap-Fn
/// hands-free still work), never a crash.
fn apply_hands_free_shortcut(app: &tauri::AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let global_shortcut = app.global_shortcut();
    let _ = global_shortcut.unregister_all();
    let accelerator = hotkey::current_settings().hands_free_hotkey;
    if accelerator.trim().is_empty() {
        return;
    }
    if let Err(e) = global_shortcut.register(accelerator.as_str()) {
        eprintln!("[whimpr] hands-free hotkey '{accelerator}' could not be registered: {e}");
    }
}

pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    // Relaunching the app (double-clicking the exe/installer shortcut again) must
    // not spawn a second process — it should just surface the running one. Without
    // this, every relaunch left a new instance in the taskbar. macOS's Dock already
    // re-activates the existing instance, so this is Windows/Linux-only.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_hub(app);
        }));
    }
    builder
        .plugin(
            // The customizable hands-free hotkey lives here — the OS registers the
            // chord, so pressing it fires our handler AND is suppressed from the
            // focused app (a listen-only CGEvent tap could not consume a printable
            // key like Space). Only the hands-free shortcut is ever registered, so
            // any Pressed event is a hands-free toggle.
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|_app, _shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        hotkey::trigger_hands_free();
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            stop_dictation,
            cancel_dictation,
            get_stats,
            get_history,
            get_dictionary,
            add_dictionary_entry,
            remove_dictionary_entry,
            get_status,
            get_last_error,
            asr_model::asr_model_status,
            asr_model::download_asr_model,
            request_microphone,
            request_accessibility,
            request_input_monitoring,
            set_api_key,
            publik::get_publik_status,
            publik::publik_refresh_wallet,
            publik::publik_accept_disclosure,
            publik::publik_forget_key,
            publik::publik_dismiss_first_run,
            publik::publik_dismiss_notice,
            publik::publik_open_link
        ])
        .setup(|app| {
            // Regular app: shows in the Dock with a normal, focusable main window.
            // (Can switch to a menu-bar-only accessory app later for the Wispr look.)
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular);

            build_overlay(app)?;
            let hub = build_hub(app)?;
            let _ = hub.show();
            let _ = hub.set_focus();
            // Closing the Hub via the X button should hide it (dictation keeps
            // running from the tray), not destroy the window — otherwise "Open
            // WhimprFlow" in the tray has nothing left to show. macOS already
            // keeps the app (and its Dock icon) alive on window close, so this
            // matters most on Windows/Linux, but is harmless everywhere.
            hub.on_window_event({
                let app_handle = app.handle().clone();
                move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Some(w) = app_handle.get_webview_window(HUB_LABEL) {
                            let _ = w.hide();
                        }
                    }
                }
            });

            // Wire the Fn key to the pill via the real state machine.
            hotkey::install(app.handle().clone());

            // Register the customizable hands-free hotkey (default Cmd+Shift+Space).
            apply_hands_free_shortcut(app.handle());

            // Keep the permission rows honest without the Hub having to be awake
            // to ask. This is what makes the setup screen's promise ("turns green
            // the moment macOS applies it — no relaunch needed") actually true:
            // the Hub's own timer stops within seconds of its window going away,
            // and the reader is granting from System Settings precisely then.
            permissions::watch(app.handle().clone());

            let open = MenuItem::with_id(app, "open", "Open WhimprFlow", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Quit WhimprFlow", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &sep, &quit])?;

            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_hub(app),
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

#[cfg(test)]
mod tests {
    use super::byo_account;

    #[test]
    fn set_api_key_still_rejects_unknown_providers() {
        // The BYO command accepts exactly openai/anthropic — "publik" is not a
        // way to write (or clobber) a key through it.
        assert_eq!(byo_account("openai"), Ok("openai_api_key"));
        assert_eq!(byo_account("anthropic"), Ok("anthropic_api_key"));
        assert!(byo_account("publik").is_err());
        assert!(byo_account("").is_err());
    }
}
