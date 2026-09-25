//! Linux platform layer for WhimprFlow: a global keyboard listener for
//! push-to-talk, clipboard+key-simulation text injection, and the same dictation
//! pipeline (audio → Whisper ASR → cleanup LLM → paste) that macOS/Windows use,
//! plus Hub-facing settings/stats/dictionary functions.
//!
//! Default push-to-talk key: Right Ctrl (same as Windows).
//! Keyboard listening uses `rdev` (X11 + Wayland). Paste uses `arboard` + `enigo`.

#![cfg(target_os = "linux")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};
use whimpr_core::{AsrEngine, CleanupContext, CleanupMode, CleanupProvider, StatsSummary};

const OVERLAY_LABEL: &str = "whimpr_bar";

static APP: OnceLock<AppHandle> = OnceLock::new();
static CLOCK: OnceLock<Instant> = OnceLock::new();
static RECORDING: AtomicBool = AtomicBool::new(false);
static CAPTURE: OnceLock<Mutex<Option<whimpr_audio::CaptureHandle>>> = OnceLock::new();
static ASR: OnceLock<Arc<whimpr_asr::WhisperEngine>> = OnceLock::new();
static LOCAL: OnceLock<Mutex<Option<crate::local_llm::LocalWorker>>> = OnceLock::new();
static OPENAI: OnceLock<Mutex<Option<whimpr_cleanup::OpenAiProvider>>> = OnceLock::new();
static ANTHROPIC: OnceLock<Mutex<Option<whimpr_cleanup::AnthropicProvider>>> = OnceLock::new();
static SETTINGS: OnceLock<Mutex<whimpr_core::Settings>> = OnceLock::new();
static DICTIONARY: OnceLock<Mutex<whimpr_core::DictionaryStore>> = OnceLock::new();
static STATS: OnceLock<Mutex<whimpr_core::StatsStore>> = OnceLock::new();

// ── Support paths (XDG-compliant) ──────────────────────────────────────────────

fn support_dir() -> std::path::PathBuf {
    // XDG_DATA_HOME or ~/.local/share
    if let Ok(d) = std::env::var("XDG_DATA_HOME") {
        if !d.is_empty() {
            return std::path::PathBuf::from(d).join("WhimprFlow");
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home).join(".local/share/WhimprFlow")
}

fn settings_path() -> std::path::PathBuf {
    support_dir().join("settings.json")
}
fn dict_path() -> std::path::PathBuf {
    support_dir().join("dictionary.json")
}
fn stats_path() -> std::path::PathBuf {
    support_dir().join("stats.json")
}
fn whisper_model_path() -> std::path::PathBuf {
    let dir = support_dir().join("models");
    for name in [
        "ggml-large-v3-turbo.bin",
        "ggml-medium.en.bin",
        "ggml-small.en.bin",
        "ggml-base.en.bin",
    ] {
        let p = dir.join(name);
        if p.exists() {
            return p;
        }
    }
    dir.join("ggml-base.en.bin")
}

// ── Time helpers ────────────────────────────────────────────────────────────────

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_ms() -> u64 {
    CLOCK.get().map(|c| c.elapsed().as_millis() as u64).unwrap_or(0)
}

// ── Pill state emission ─────────────────────────────────────────────────────────

fn emit_bar(state: &'static str) {
    if let Some(app) = APP.get() {
        #[derive(Clone, serde::Serialize)]
        struct P {
            state: &'static str,
        }
        let _ = app.emit_to(OVERLAY_LABEL, "whimpr://flowbar/state", P { state });
    }
}

// ── Frontmost app detection (Linux) ─────────────────────────────────────────────

/// Try to detect the frontmost window's class/name. Returns None on failure
/// or when running under an unsupported display server.
fn foreground_app() -> Option<String> {
    // Try xdotool first (most reliable on X11)
    if let Ok(out) = std::process::Command::new("xdotool")
        .args(["getactivewindow", "getwindowclassname"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    // Fallback: try to get the window title via xprop
    if let Ok(out) = std::process::Command::new("sh")
        .arg("-c")
        .arg("xprop -id $(xprop -root 32x '\t$0' _NET_ACTIVE_WINDOW | cut -f2) WM_CLASS 2>/dev/null | cut -d'\"' -f4")
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

// ── Text injection: clipboard + Ctrl+V via enigo ────────────────────────────────

pub fn paste_text(text: &str) -> anyhow::Result<()> {
    use arboard::Clipboard;
    let mut cb = Clipboard::new()?;
    let saved = cb.get_text().ok();
    cb.set_text(text.to_string())?;
    std::thread::sleep(Duration::from_millis(60));

    // Simulate Ctrl+V. Try ydotool first (Wayland), then xdotool (X11).
    let ydotool = std::process::Command::new("ydotool")
        .args(["key", "29:1", "47:1", "47:0", "29:0"]) // Ctrl down, V down, V up, Ctrl up
        .output();
    if ydotool.is_err() || !ydotool.as_ref().map(|o| o.status.success()).unwrap_or(false) {
        // Fallback: xdotool for X11
        let _ = std::process::Command::new("xdotool")
            .args(["key", "--clearmodifiers", "ctrl+v"])
            .output();
    }

    std::thread::sleep(Duration::from_millis(150));
    if let Some(prev) = saved {
        let _ = cb.set_text(prev);
    }
    Ok(())
}

// ── Cleanup (shared, cross-platform building blocks) ────────────────────────────

fn current_settings_inner() -> whimpr_core::Settings {
    SETTINGS
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default()
}

fn read_key(account: &str, env_var: &str) -> Option<String> {
    if let Ok(k) = std::env::var(env_var) {
        let k = k.trim().to_string();
        if !k.is_empty() {
            return Some(k);
        }
    }
    keyring::Entry::new("com.whimpr.whimprflow", account)
        .ok()
        .and_then(|e| e.get_password().ok())
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

fn clean_transcript(raw: &str) -> String {
    let settings = current_settings_inner();
    let level = settings.cleanup_level;
    if matches!(settings.cleanup_mode, CleanupMode::Raw) || level.bypasses_llm() {
        return raw.to_string();
    }
    let raw_norm = whimpr_core::cleanup::pre_normalize_layout(raw);
    let raw_out = whimpr_core::cleanup::post_process(&raw_norm);
    let vocab = DICTIONARY
        .get()
        .map(|d| d.lock().unwrap().prefilter(&raw_norm, 15))
        .unwrap_or_default();
    let ctx = CleanupContext {
        level,
        vocab,
        app_bundle_id: foreground_app(),
        ..Default::default()
    };
    let run_local = || -> Option<anyhow::Result<String>> {
        LOCAL.get().and_then(|m| {
            m.lock().unwrap().as_mut().map(|w| {
                let messages = whimpr_core::cleanup::build_messages(&raw_norm, &ctx);
                w.cleanup(&messages)
            })
        })
    };
    let result = match settings.cleanup_mode {
        CleanupMode::OpenAi => OPENAI
            .get()
            .and_then(|m| m.lock().unwrap().as_ref().map(|p| p.cleanup(&raw_norm, &ctx)))
            .or_else(run_local),
        CleanupMode::Anthropic => ANTHROPIC
            .get()
            .and_then(|m| m.lock().unwrap().as_ref().map(|p| p.cleanup(&raw_norm, &ctx)))
            .or_else(run_local),
        CleanupMode::Local => run_local(),
        CleanupMode::Raw => None,
    };
    match result {
        Some(Ok(cleaned)) => {
            let cleaned = whimpr_core::cleanup::post_process(&cleaned);
            if whimpr_core::cleanup::evaluate_gates(&raw_out, &cleaned, level).passed() {
                cleaned
            } else {
                eprintln!("[whimpr:linux] cleanup gate rejected — pasting raw");
                raw_out
            }
        }
        Some(Err(e)) => {
            eprintln!("[whimpr:linux] cleanup failed ({e}) — pasting raw");
            raw_out
        }
        None => raw_out,
    }
}

fn record_dictation(text: &str, duration_secs: f32, app: Option<String>) {
    let words = whimpr_core::stats::count_words(text);
    if words == 0 {
        return;
    }
    if let Some(m) = STATS.get() {
        let mut store = m.lock().unwrap();
        let duration_ms = (duration_secs.max(0.0) * 1000.0) as u32;
        let chars = text.chars().count() as u32;
        store.record(words, duration_ms, chars, unix_now(), text.to_string(), app);
        let _ = store.save(&stats_path());
    }
}

// ── The push-to-talk pipeline ───────────────────────────────────────────────────

fn on_ptt_down() {
    if RECORDING.swap(true, Ordering::SeqCst) {
        return; // already recording
    }
    let _ = now_ms();
    eprintln!("[whimpr:linux] PTT DOWN — starting capture");
    emit_bar("recording");
    std::thread::spawn(|| match whimpr_audio::start(|_: &[f32]| {}) {
        Ok(handle) => {
            *CAPTURE.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(handle);
        }
        Err(e) => eprintln!("[whimpr:linux] mic capture failed: {e}"),
    });
}

fn on_ptt_up() {
    if !RECORDING.swap(false, Ordering::SeqCst) {
        return; // wasn't recording
    }
    eprintln!("[whimpr:linux] PTT UP — finalizing");
    emit_bar("transcribing");
    let app = foreground_app();
    let handle = CAPTURE.get().and_then(|slot| slot.lock().unwrap().take());
    std::thread::spawn(move || {
        let Some(res) = handle.and_then(|h| h.stop()) else {
            eprintln!("[whimpr:linux] no audio captured");
            emit_bar("idle");
            return;
        };
        let Some(asr) = ASR.get().cloned() else {
            eprintln!("[whimpr:linux] ASR not ready");
            emit_bar("idle");
            return;
        };
        let pcm = whimpr_audio::resample_to_16k(&res.samples, res.sample_rate);
        match asr.transcribe(&pcm) {
            Ok(t) => {
                let raw = &t.text;
                eprintln!("[whimpr:linux] TRANSCRIPT: \"{}\"", raw);
                emit_bar("done");
                let text = clean_transcript(raw);
                if text != *raw {
                    eprintln!("[whimpr:linux] CLEANED:   \"{}\"", text);
                }
                if !text.is_empty() {
                    if let Err(e) = paste_text(&text) {
                        eprintln!("[whimpr:linux] paste failed: {e}");
                    }
                    record_dictation(&text, res.duration_secs(), app);
                }
            }
            Err(e) => eprintln!("[whimpr:linux] ASR error: {e}"),
        }
        // Return pill to idle after a short "done" display
        std::thread::sleep(Duration::from_millis(500));
        emit_bar("idle");
    });
}

// ── Global keyboard listener via rdev ────────────────────────────────────────────

/// Parse a string like "ControlRight" into the corresponding rdev::Key.
/// Returns None for unrecognized keys.
fn parse_ptt_key(s: &str) -> Option<rdev::Key> {
    use rdev::Key::*;
    match s {
        "ControlRight" => Some(ControlRight),
        "ControlLeft" => Some(ControlLeft),
        "AltRight" => Some(AltGr),
        "AltLeft" => Some(Alt),
        "ShiftRight" => Some(ShiftRight),
        "ShiftLeft" => Some(ShiftLeft),
        "MetaRight" => Some(MetaRight),
        "MetaLeft" => Some(MetaLeft),
        "CapsLock" => Some(CapsLock),
        "F1" => Some(F1),
        "F2" => Some(F2),
        "F3" => Some(F3),
        "F4" => Some(F4),
        "F5" => Some(F5),
        "F6" => Some(F6),
        "F7" => Some(F7),
        "F8" => Some(F8),
        "F9" => Some(F9),
        "F10" => Some(F10),
        "F11" => Some(F11),
        "F12" => Some(F12),
        _ => {
            eprintln!("[whimpr:linux] unrecognized push-to-talk key '{s}', falling back to ControlRight");
            Some(ControlRight)
        }
    }
}

/// Human-readable label for a key variant string (shown in the startup message).
fn ptt_key_label(s: &str) -> &str {
    match s {
        "ControlRight" => "Right Ctrl",
        "ControlLeft" => "Left Ctrl",
        "AltRight" => "Right Alt",
        "AltLeft" => "Left Alt",
        "ShiftRight" => "Right Shift",
        "ShiftLeft" => "Left Shift",
        "MetaRight" => "Right Super/Meta",
        "MetaLeft" => "Left Super/Meta",
        "CapsLock" => "Caps Lock",
        s if s.starts_with('F') => s, // F1-F12
        _ => s,
    }
}

fn spawn_keyboard_listener() {
    // Capture the configured key *now* (before the thread starts) so the
    // listener always sees the key that was active at install time.
    let ptt_key_str = current_settings_inner().push_to_talk_key;
    let ptt_key = parse_ptt_key(&ptt_key_str);
    let label = ptt_key_label(&ptt_key_str).to_string();

    std::thread::spawn(move || {
        let Some(ptt_key) = ptt_key else {
            eprintln!("[whimpr:linux] invalid push-to-talk key '{ptt_key_str}' — listener not started");
            return;
        };
        // Give the app a moment to fully initialize before hooking keys
        std::thread::sleep(Duration::from_millis(500));
        eprintln!("[whimpr:linux] starting global keyboard listener (push-to-talk: {label})");

        if let Err(e) = rdev::listen(move |event| {
            use rdev::EventType::{KeyPress, KeyRelease};
            match event.event_type {
                KeyPress(k) if k == ptt_key => on_ptt_down(),
                KeyRelease(k) if k == ptt_key => on_ptt_up(),
                _ => {}
            }
        }) {
            eprintln!(
                "[whimpr:linux] rdev keyboard listener failed: {e:?}. \
                 You may need to add yourself to the 'input' group: \
                 sudo usermod -aG input $USER && reboot"
            );
        }
    });
}

// ── Public surface (mirrors the macOS `hotkey::` and Windows `win::` functions) ──

pub fn install(app: AppHandle) {
    let _ = APP.set(app.clone());
    let _ = CLOCK.set(Instant::now());
    let _ = SETTINGS.set(Mutex::new(whimpr_core::Settings::load(&settings_path())));
    let _ = DICTIONARY.set(Mutex::new(whimpr_core::DictionaryStore::load(&dict_path())));
    let _ = STATS.set(Mutex::new(whimpr_core::StatsStore::load(&stats_path())));
    let _ = OPENAI.set(Mutex::new(None));
    let _ = ANTHROPIC.set(Mutex::new(None));
    let _ = LOCAL.set(Mutex::new(None));

    // Ensure support dirs exist
    let _ = std::fs::create_dir_all(&support_dir());
    let _ = std::fs::create_dir_all(&support_dir().join("models"));

    rebuild_providers();
    eprintln!(
        "[whimpr:linux] data dir: {}",
        support_dir().display()
    );

    // Load Whisper ASR model
    std::thread::spawn(|| {
        let path = whisper_model_path();
        if !path.exists() {
            eprintln!(
                "[whimpr:linux] ⚠ ASR model not found at {}. \
                 Download a whisper.cpp model (e.g., ggml-base.en.bin) and place it there.",
                path.display()
            );
            return;
        }
        match whimpr_asr::WhisperEngine::load(&path) {
            Ok(engine) => {
                let _ = ASR.set(Arc::new(engine));
                eprintln!("[whimpr:linux] ASR model loaded — ready to transcribe");
            }
            Err(e) => eprintln!("[whimpr:linux] ASR model load failed: {e}"),
        }
    });

    // Start the local cleanup worker
    std::thread::spawn(|| {
        if let Some(w) = crate::local_llm::spawn_default() {
            if let Some(slot) = LOCAL.get() {
                *slot.lock().unwrap() = Some(w);
            }
        }
    });

    // Start the global keyboard listener for push-to-talk
    let settings = current_settings_inner();
    let ptt_label = ptt_key_label(&settings.push_to_talk_key);
    spawn_keyboard_listener();

    eprintln!("[whimpr:linux] installed — hold {ptt_label} to dictate");
}

/// Stop / cancel the overlay pill's current recording, and toggle hands-free
/// dictation. The Linux driver does not route through the shared
/// `handle_input` state machine the macOS layer uses (it toggles `RECORDING`
/// directly from the rdev key listener), so these are inert here for now —
/// mirrors `win.rs`'s stub until a Linux equivalent of the pill's Stop/✕/
/// hands-free buttons is wired up.
pub fn stop_dictation() {}
pub fn cancel_dictation() {}
pub fn trigger_hands_free() {}

pub fn current_settings() -> whimpr_core::Settings {
    current_settings_inner()
}

pub fn update_settings(new: whimpr_core::Settings) {
    if let Some(m) = SETTINGS.get() {
        *m.lock().unwrap() = new.clone();
    }
    let _ = new.save(&settings_path());
    rebuild_providers();
}

pub fn rebuild_providers() {
    let settings = current_settings_inner();

    let openai = read_key("openai_api_key", "OPENAI_API_KEY").map(|k| {
        whimpr_cleanup::OpenAiProvider::with_base_url(
            k,
            settings.openai_model.clone(),
            Some(settings.openai_base_url.clone()),
        )
    });
    let anthropic = read_key("anthropic_api_key", "ANTHROPIC_API_KEY")
        .map(|k| whimpr_cleanup::AnthropicProvider::new(k, settings.anthropic_model.clone()));

    match OPENAI.get() {
        Some(m) => *m.lock().unwrap() = openai,
        None => {
            let _ = OPENAI.set(Mutex::new(openai));
        }
    }
    match ANTHROPIC.get() {
        Some(m) => *m.lock().unwrap() = anthropic,
        None => {
            let _ = ANTHROPIC.set(Mutex::new(anthropic));
        }
    }
}

pub fn stats_summary(tz_offset_minutes: i32) -> StatsSummary {
    STATS
        .get()
        .map(|m| m.lock().unwrap().summary(tz_offset_minutes, unix_now()))
        .unwrap_or_else(|| whimpr_core::StatsStore::default().summary(tz_offset_minutes, unix_now()))
}

pub fn history(limit: usize) -> Vec<whimpr_core::HistoryItem> {
    STATS
        .get()
        .map(|m| m.lock().unwrap().history(limit))
        .unwrap_or_default()
}

pub fn dictionary_entries() -> Vec<crate::hotkey::DictEntryDto> {
    DICTIONARY
        .get()
        .map(|m| {
            m.lock()
                .unwrap()
                .entries
                .iter()
                .map(|e| crate::hotkey::DictEntryDto {
                    correct: e.correct.clone(),
                    mishears: e.mishears.clone(),
                    auto: matches!(e.source, whimpr_core::DictSource::Auto),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn dictionary_add(correct: String, mishears: Vec<String>) {
    if let Some(m) = DICTIONARY.get() {
        let mut store = m.lock().unwrap();
        store.add(correct, mishears, whimpr_core::DictSource::Manual);
        let _ = store.save(&dict_path());
    }
}

pub fn dictionary_remove(correct: &str) {
    if let Some(m) = DICTIONARY.get() {
        let mut store = m.lock().unwrap();
        if store.remove(correct) {
            let _ = store.save(&dict_path());
        }
    }
}

pub fn dictionary_learn(correct: String, mishears: Vec<String>) {
    if let Some(m) = DICTIONARY.get() {
        let mut store = m.lock().unwrap();
        store.add(correct, mishears, whimpr_core::DictSource::Auto);
        let _ = store.save(&dict_path());
    }
}
