//! Hold-Fn → pill wiring for the demo shell.
//!
//! This installs an in-process CoreGraphics event tap that feeds Fn key-down /
//! key-up into the real [`whimpr_core`] dictation state machine, and turns the
//! machine's actions into `whimpr://flowbar/state` events the overlay pill
//! renders. There is no audio or ASR yet, so a finalized session is simulated as
//! completing shortly after key release — enough to see the full
//! recording → transcribing → done → idle loop driven by the actual state machine.
//!
//! In the shipping product this hook lives in a separate sidecar process (so heavy
//! inference can't stall it); running it in-process is an acceptable macOS-only
//! path for this demo and the early milestones.

/// Dictionary entry shape sent to the Hub UI (auto-learned entries flagged).
#[derive(Clone, serde::Serialize)]
pub struct DictEntryDto {
    pub correct: String,
    pub mishears: Vec<String>,
    pub auto: bool,
}

#[cfg(target_os = "macos")]
mod imp {
    use std::os::raw::c_void;
    use std::path::PathBuf;
    use super::DictEntryDto;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use serde::Serialize;
    use tauri::{AppHandle, Emitter};
    use whimpr_core::state::{Action, BarState};
    use whimpr_core::{
        AsrEngine, CleanupContext, CleanupMode, CleanupProvider, Input, PipelineEvent, StateMachine,
        TriggerToken,
    };
    use whimpr_ipc::BindingId;

    const OVERLAY_LABEL: &str = "whimpr_bar";

    // --- CoreGraphics / CoreFoundation FFI (listen-only Fn tap) -----------
    type CFMachPortRef = *mut c_void;
    type CFRunLoopSourceRef = *mut c_void;
    type CFRunLoopRef = *mut c_void;
    type CFStringRef = *const c_void;
    type CFAllocatorRef = *const c_void;
    type CGEventRef = *mut c_void;
    type CGEventTapProxy = *mut c_void;
    type CGEventTapCallBack =
        extern "C" fn(CGEventTapProxy, u32, CGEventRef, *mut c_void) -> CGEventRef;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: u64,
            callback: CGEventTapCallBack,
            user_info: *mut c_void,
        ) -> CFMachPortRef;
        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
        fn CGEventGetFlags(event: CGEventRef) -> u64;
        fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFMachPortCreateRunLoopSource(
            allocator: CFAllocatorRef,
            port: CFMachPortRef,
            order: isize,
        ) -> CFRunLoopSourceRef;
        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
        fn CFRunLoopRun();
        static kCFRunLoopDefaultMode: CFStringRef;
    }

    const K_CG_SESSION_EVENT_TAP: u32 = 1;
    const K_CG_HEAD_INSERT: u32 = 0;
    const K_CG_TAP_OPTION_LISTEN_ONLY: u32 = 1;
    const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;
    const EVENTS_OF_INTEREST: u64 = 1 << K_CG_EVENT_FLAGS_CHANGED;
    const FLAG_SECONDARY_FN: u64 = 0x0080_0000;
    const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
    const KEYCODE_FN: i64 = 63;
    const K_CG_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
    const K_CG_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

    static APP: OnceLock<AppHandle> = OnceLock::new();
    static MACHINE: OnceLock<Mutex<StateMachine>> = OnceLock::new();
    static CLOCK: OnceLock<Instant> = OnceLock::new();
    static FN_IS_DOWN: AtomicBool = AtomicBool::new(false);
    static TAP_PORT: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    /// Bundle id of the app that was frontmost at record-start = the paste target.
    /// Cleanup uses it to format for the medium (email vs. text vs. chat).
    static TARGET_APP: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    static CAPTURE: OnceLock<Mutex<Option<whimpr_audio::CaptureHandle>>> = OnceLock::new();
    static OPENAI: OnceLock<Mutex<Option<whimpr_cleanup::OpenAiProvider>>> = OnceLock::new();
    static ANTHROPIC: OnceLock<Mutex<Option<whimpr_cleanup::AnthropicProvider>>> = OnceLock::new();
    static PUBLIK: OnceLock<Mutex<Option<whimpr_cleanup::PublikProvider>>> = OnceLock::new();
    static LOCAL: OnceLock<Mutex<Option<crate::local_llm::LocalWorker>>> = OnceLock::new();
    /// The cloud speech-to-text engine (`Settings::asr_mode == Cloud`), rebuilt
    /// whenever the OpenAI key or the ASR base URL / model changes. `None` when
    /// Cloud mode isn't selected or no OpenAI-slot key is saved — the finalize
    /// path falls back to `asr_model::engine()` (Local) in that case.
    static ASR_CLOUD: OnceLock<Mutex<Option<std::sync::Arc<dyn AsrEngine>>>> = OnceLock::new();
    static SETTINGS: OnceLock<Mutex<whimpr_core::Settings>> = OnceLock::new();
    static DICTIONARY: OnceLock<Mutex<whimpr_core::DictionaryStore>> = OnceLock::new();
    static STATS: OnceLock<Mutex<whimpr_core::StatsStore>> = OnceLock::new();

    #[derive(Clone, Serialize)]
    struct BarPayload {
        state: &'static str,
    }

    #[derive(Clone, Serialize)]
    struct WavePayload {
        bars: Vec<f32>,
    }

    #[derive(Clone, Serialize)]
    struct TranscriptPayload {
        text: String,
    }

    fn support_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join("Library/Application Support/WhimprFlow")
    }
    fn settings_path() -> PathBuf {
        support_dir().join("settings.json")
    }
    fn dict_path() -> PathBuf {
        support_dir().join("dictionary.json")
    }
    fn stats_path() -> PathBuf {
        support_dir().join("stats.json")
    }

    /// Seconds since the Unix epoch (UTC), or 0 if the clock is before the epoch.
    fn unix_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Play the record-start ping, if `Settings::sound_on_start` is on.
    ///
    /// `Action::PlayPing` used to fall into the state machine's `_ => {}`
    /// catch-all, so the "play a sound when recording starts" setting did
    /// nothing. `afplay` rather than `NSSound`: no extra dependency, no
    /// main-thread requirement, and it can be fired from the event-tap thread
    /// without any risk of blocking key handling. Spawned so a slow process
    /// launch never delays the rest of the tap callback.
    fn play_start_cue() {
        if !current_settings().sound_on_start {
            return;
        }
        std::thread::spawn(|| {
            let _ = std::process::Command::new("/usr/bin/afplay")
                .arg("/System/Library/Sounds/Pop.aiff")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }

    /// Log one completed dictation to the stats store (words, speaking time, text,
    /// target app) and persist it. Powers both the Hub stats and the history list.
    pub fn record_dictation(text: &str, duration_secs: f32) {
        let words = whimpr_core::stats::count_words(text);
        if words == 0 {
            return;
        }
        let app = TARGET_APP.get().and_then(|m| m.lock().unwrap().clone());
        if let Some(m) = STATS.get() {
            let mut store = m.lock().unwrap();
            let duration_ms = (duration_secs.max(0.0) * 1000.0) as u32;
            let chars = text.chars().count() as u32;
            store.record(words, duration_ms, chars, unix_now(), text.to_string(), app);
            let _ = store.save(&stats_path());
        }
    }

    /// The most recent dictations for the Hub Home history list.
    pub fn history(limit: usize) -> Vec<whimpr_core::HistoryItem> {
        STATS
            .get()
            .map(|m| m.lock().unwrap().history(limit))
            .unwrap_or_default()
    }

    /// The dictionary entries for the Hub Dictionary screen (auto-learned flagged).
    pub fn dictionary_entries() -> Vec<DictEntryDto> {
        DICTIONARY
            .get()
            .map(|m| {
                m.lock()
                    .unwrap()
                    .entries
                    .iter()
                    .map(|e| DictEntryDto {
                        correct: e.correct.clone(),
                        mishears: e.mishears.clone(),
                        auto: matches!(e.source, whimpr_core::DictSource::Auto),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Add a manual dictionary entry and persist.
    pub fn dictionary_add(correct: String, mishears: Vec<String>) {
        if let Some(m) = DICTIONARY.get() {
            let mut store = m.lock().unwrap();
            store.add(correct, mishears, whimpr_core::DictSource::Manual);
            let _ = store.save(&dict_path());
        }
    }

    /// Remove a dictionary entry by spelling and persist.
    pub fn dictionary_remove(correct: &str) {
        if let Some(m) = DICTIONARY.get() {
            let mut store = m.lock().unwrap();
            if store.remove(correct) {
                let _ = store.save(&dict_path());
            }
        }
    }

    /// Add an AUTO-learned entry (from the post-paste correction observer) and persist.
    /// Marked ✨ auto in the UI. No-op if it would duplicate an existing entry's data.
    pub fn dictionary_learn(correct: String, mishears: Vec<String>) {
        if let Some(m) = DICTIONARY.get() {
            let mut store = m.lock().unwrap();
            store.add(correct, mishears, whimpr_core::DictSource::Auto);
            let _ = store.save(&dict_path());
        }
    }

    /// Aggregated stats for the Hub. `tz_offset_minutes` is the UI's
    /// `Date.getTimezoneOffset()` so day math matches the user's local clock.
    pub fn stats_summary(tz_offset_minutes: i32) -> whimpr_core::StatsSummary {
        STATS
            .get()
            .map(|m| m.lock().unwrap().summary(tz_offset_minutes, unix_now()))
            .unwrap_or_else(|| {
                whimpr_core::StatsStore::default().summary(tz_offset_minutes, unix_now())
            })
    }

    /// Read an API key from an env var or the OS keychain (never a plaintext file).
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
    fn read_openai_key() -> Option<String> {
        read_key("openai_api_key", "OPENAI_API_KEY")
    }
    fn read_anthropic_key() -> Option<String> {
        read_key("anthropic_api_key", "ANTHROPIC_API_KEY")
    }

    /// A snapshot of the current settings.
    pub fn current_settings() -> whimpr_core::Settings {
        SETTINGS
            .get()
            .map(|m| m.lock().unwrap().clone())
            .unwrap_or_default()
    }
    /// Apply new settings and rebuild the cloud providers (picks up model changes).
    pub fn update_settings(new: whimpr_core::Settings) {
        if let Some(m) = SETTINGS.get() {
            *m.lock().unwrap() = new.clone();
        }
        let _ = new.save(&settings_path());
        rebuild_providers();
    }

    /// (Re)build the cloud cleanup providers from the current keys + settings. Called
    /// at startup and whenever a key or model changes, so edits take effect live.
    pub fn rebuild_providers() {
        let settings = current_settings();
        let openai = read_openai_key().map(|k| {
            whimpr_cleanup::OpenAiProvider::with_base_url(
                k,
                settings.openai_model.clone(),
                Some(settings.openai_base_url.clone()),
            )
        });
        let anthropic = read_anthropic_key()
            .map(|k| whimpr_cleanup::AnthropicProvider::new(k, settings.anthropic_model.clone()));
        // publik: its own keychain account + the base URL / alias the
        // provisioning response handed back (never the user's OpenAI slot).
        let publik = crate::publik::read_key().map(|k| {
            whimpr_cleanup::PublikProvider::new(
                k,
                &whimpr_cleanup::publik::resolve_base_url(&settings.publik_base_url),
                &settings.publik_model,
            )
        });
        eprintln!(
            "[whimpr] cleanup providers: openai={}, anthropic={}, publik={}",
            openai.is_some(),
            anthropic.is_some(),
            publik.is_some()
        );
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
        match PUBLIK.get() {
            Some(m) => *m.lock().unwrap() = publik,
            None => {
                let _ = PUBLIK.set(Mutex::new(publik));
            }
        }
        rebuild_cloud_asr(&settings);
    }

    /// (Re)build the cloud speech-to-text engine to match `Settings::asr_mode`.
    /// Cloud ASR reuses the OpenAI-slot key; `None` (Local, or Cloud with no key
    /// saved) means the finalize path falls back to `asr_model::engine()`.
    fn rebuild_cloud_asr(settings: &whimpr_core::Settings) {
        let engine: Option<std::sync::Arc<dyn AsrEngine>> = if matches!(
            settings.asr_mode,
            whimpr_core::AsrMode::Cloud
        ) {
            read_openai_key().map(|key| {
                let engine: std::sync::Arc<dyn AsrEngine> =
                    std::sync::Arc::new(whimpr_cleanup::CloudAsr::with_base_url(
                        key,
                        settings.asr_model.clone(),
                        Some(settings.asr_base_url.clone()),
                    ));
                engine
            })
        } else {
            None
        };
        match ASR_CLOUD.get() {
            Some(m) => *m.lock().unwrap() = engine,
            None => {
                let _ = ASR_CLOUD.set(Mutex::new(engine));
            }
        }
    }

    /// Clean a raw transcript per the current settings (mode + level), feeding in the
    /// dictionary vocabulary relevant to this utterance. Falls back to raw whenever
    /// cleanup is off, the provider is unavailable, it errors, or the gates reject it.
    fn clean_transcript(app: &AppHandle, raw: &str) -> String {
        let settings = current_settings();
        let level = settings.cleanup_level;
        if matches!(settings.cleanup_mode, CleanupMode::Raw) || level.bypasses_llm() {
            return raw.to_string();
        }
        // Turn explicit spoken layout cues ("new line", "new paragraph") into break
        // markers up front — the model passes an opaque marker through reliably but
        // mangles the literal cue words. The model sees `raw` (with markers); the gate
        // and any raw fallback use `raw_out` (markers restored to real breaks) so we
        // never paste a "[[NL]]" token or lose an explicit break.
        let raw_norm = whimpr_core::cleanup::pre_normalize_layout(raw);
        let raw = raw_norm.as_str();
        let raw_out = whimpr_core::cleanup::post_process(&raw_norm);
        let vocab = DICTIONARY
            .get()
            .map(|d| d.lock().unwrap().prefilter(raw, 15))
            .unwrap_or_default();
        let app_bundle_id = TARGET_APP.get().and_then(|m| m.lock().unwrap().clone());
        if let Some(app) = app_bundle_id.as_deref() {
            eprintln!("[whimpr] cleanup target app: {app}");
        }
        let ctx = CleanupContext {
            level,
            vocab,
            app_bundle_id,
            ..Default::default()
        };
        // Run the on-device model with the same prompt + per-app formatting.
        let run_local = || -> Option<anyhow::Result<String>> {
            LOCAL.get().and_then(|m| {
                m.lock().unwrap().as_mut().map(|w| {
                    // System prompt + few-shot demonstration turns + the transcript,
                    // so the on-device model actually produces newlines/lists and
                    // resolves self-corrections instead of just being told to.
                    let messages = whimpr_core::cleanup::build_messages(raw, &ctx);
                    w.cleanup(&messages)
                })
            })
        };
        // Selected provider, falling back to local when a cloud key can't be read
        // (so cleanup still runs) — and Local mode uses the worker directly.
        let result: Option<anyhow::Result<String>> = match settings.cleanup_mode {
            CleanupMode::OpenAi => OPENAI
                .get()
                .and_then(|m| m.lock().unwrap().as_ref().map(|p| p.cleanup(raw, &ctx)))
                .or_else(run_local),
            CleanupMode::Anthropic => ANTHROPIC
                .get()
                .and_then(|m| m.lock().unwrap().as_ref().map(|p| p.cleanup(raw, &ctx)))
                .or_else(run_local),
            CleanupMode::Publik => PUBLIK
                .get()
                .and_then(|m| m.lock().unwrap().as_ref().map(|p| p.cleanup(raw, &ctx)))
                .or_else(run_local),
            CleanupMode::Local => run_local(),
            CleanupMode::Raw => None,
        };
        match result {
            Some(Ok(cleaned)) => {
                // The gateway stamps the charge + remaining balance on every
                // answer; move the Settings card's balance line without a
                // second request.
                if matches!(settings.cleanup_mode, CleanupMode::Publik) {
                    let snap = PUBLIK
                        .get()
                        .and_then(|m| m.lock().unwrap().as_ref().and_then(|p| p.last_balance()));
                    if let Some(snap) = snap {
                        crate::publik::set_balance_from_snapshot(app, &snap);
                    }
                }
                // Deterministic safety net: convert any leftover spoken layout cue the
                // model missed into real line breaks, strip stray code fences, cap blank
                // lines. Guarantees no "new line"/"new paragraph" word reaches the cursor.
                let cleaned = whimpr_core::cleanup::post_process(&cleaned);
                if whimpr_core::cleanup::evaluate_gates(&raw_out, &cleaned, level).passed() {
                    cleaned
                } else {
                    eprintln!("[whimpr] cleanup gate rejected the edit — pasting raw");
                    raw_out
                }
            }
            Some(Err(e)) => {
                // A typed publik outcome (402 / revoked key / unreachable)
                // decides what the user is told and whether the key survives;
                // the dictation itself is never blocked — raw is pasted below.
                if let Some(pe) = e.downcast_ref::<whimpr_cleanup::PublikError>() {
                    crate::publik::handle_error(app, pe);
                }
                eprintln!("[whimpr] cleanup failed ({e}) — pasting raw");
                raw_out
            }
            None => {
                if matches!(settings.cleanup_mode, CleanupMode::Local) {
                    eprintln!("[whimpr] local cleanup model not wired yet — pasting raw");
                } else {
                    eprintln!("[whimpr] cleanup provider has no API key — pasting raw");
                }
                raw_out
            }
        }
    }

    fn now_ms() -> u64 {
        CLOCK.get().map(|c| c.elapsed().as_millis() as u64).unwrap_or(0)
    }

    fn bar_name(b: BarState) -> &'static str {
        match b {
            BarState::Idle => "idle",
            BarState::Recording => "recording",
            BarState::Locked => "locked",
            BarState::Transcribing => "transcribing",
            BarState::Done => "done",
            BarState::Cancelled => "cancelled",
            BarState::Error => "error",
        }
    }

    fn emit_bar(app: &AppHandle, state: &'static str) {
        eprintln!("[whimpr] pill -> {state}");
        let _ = app.emit_to(OVERLAY_LABEL, "whimpr://flowbar/state", BarPayload { state });
    }

    /// Feed one input into the shared state machine and enact its actions.
    fn handle_input(input: Input) {
        let (Some(app), Some(machine)) = (APP.get(), MACHINE.get()) else {
            return;
        };
        let actions = {
            let mut m = machine.lock().unwrap();
            m.step(input)
        };
        for action in actions {
            apply_action(app, action);
        }
    }

    fn apply_action(app: &AppHandle, action: Action) {
        match action {
            Action::ShowBar(bar) => {
                emit_bar(app, bar_name(bar));
                // Let the "done" tick linger briefly before returning to idle.
                if bar == BarState::Done {
                    let app2 = app.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(500));
                        emit_bar(&app2, "idle");
                    });
                }
            }
            // Start the microphone; stream real RMS bars to the pill waveform.
            // Runs off the tap thread so the mic-permission prompt can't stall keys.
            Action::StartCapture { .. } => {
                let app_thread = app.clone();
                std::thread::spawn(move || {
                    let app_cb = app_thread.clone();
                    match whimpr_audio::start(move |bars| {
                        let _ = app_cb.emit_to(
                            OVERLAY_LABEL,
                            "whimpr://audio/waveform",
                            WavePayload { bars: bars.to_vec() },
                        );
                    }) {
                        Ok(handle) => {
                            *CAPTURE.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(handle);
                        }
                        Err(e) => eprintln!("[whimpr] mic capture failed to start: {e}"),
                    }
                });
            }
            // Stop the mic, transcribe the buffered audio, and advance the machine.
            Action::StopCaptureAndFinalize { session } => {
                let app2 = app.clone();
                let handle = CAPTURE.get().and_then(|slot| slot.lock().unwrap().take());
                std::thread::spawn(move || {
                    // Whatever happens, return the pill to idle (done -> idle).
                    let finish =
                        || handle_input(Input::Pipeline(PipelineEvent::Committed { session }));
                    let Some(res) = handle.and_then(|h| h.stop()) else {
                        eprintln!("[whimpr] no audio captured");
                        finish();
                        return;
                    };
                    let peak = res.samples.iter().fold(0f32, |m, &s| m.max(s.abs()));
                    eprintln!(
                        "[whimpr] captured {} samples @ {} Hz (~{:.2}s), peak {:.4}",
                        res.samples.len(),
                        res.sample_rate,
                        res.duration_secs(),
                        peak
                    );
                    // Below ~600ms is very likely an accidental brief tap (the SPEC's
                    // own single-tap-no-op gate), so don't alarm the user over it —
                    // only surface a loud diagnostic for holds long enough to be a
                    // real, intentional dictation attempt.
                    let was_real_attempt = res.duration_secs() >= 0.6;
                    if peak < 0.005 {
                        eprintln!(
                            "[whimpr] ⚠ audio is silent — the mic isn't being captured. Grant \
                             Microphone access to your terminal (System Settings → Privacy & \
                             Security → Microphone), then fully quit + reopen it and rerun."
                        );
                        if was_real_attempt {
                            crate::diag::report(
                                &app2,
                                whimpr_core::InjectionFailure::NoAudioCaptured,
                            );
                        }
                        finish();
                        return;
                    }
                    // Cloud ASR (Settings::asr_mode == Cloud, with an OpenAI-slot key
                    // saved) takes priority when configured; otherwise fall back to
                    // the shared local Whisper engine (asr_model.rs).
                    let cloud = ASR_CLOUD.get().and_then(|m| m.lock().unwrap().clone());
                    let asr: std::sync::Arc<dyn AsrEngine> = match cloud {
                        Some(engine) => engine,
                        None => match crate::asr_model::engine() {
                            Some(engine) => engine,
                            None => {
                                eprintln!(
                                    "[whimpr] ASR not ready (model still loading or missing)"
                                );
                                if was_real_attempt {
                                    crate::asr_model::report_unavailable(&app2);
                                }
                                finish();
                                return;
                            }
                        },
                    };
                    let pcm = whimpr_audio::resample_to_16k(&res.samples, res.sample_rate);
                    match asr.transcribe(&pcm) {
                        Ok(t) => {
                            let raw = t.text;
                            eprintln!("[whimpr] TRANSCRIPT: \"{}\"", raw);
                            // Clean the transcript (cloud LLM if configured), then paste.
                            let text = clean_transcript(&app2, &raw);
                            if text != raw {
                                eprintln!("[whimpr] CLEANED:   \"{}\"", text);
                            }
                            if !text.is_empty() {
                                if let Err(e) = crate::paste::paste_text(&text) {
                                    eprintln!("[whimpr] paste failed: {e}");
                                    // Distinguish the two real causes: Accessibility was
                                    // never (or no longer) granted, vs. everything else
                                    // (clipboard contention, etc.) — `is_trusted()` is the
                                    // authoritative check, cheaper and more precise than
                                    // matching on the error string.
                                    let failure = if !crate::paste::is_trusted() {
                                        whimpr_core::InjectionFailure::AccessibilityNotGranted
                                    } else {
                                        whimpr_core::InjectionFailure::ClipboardUnavailable
                                    };
                                    crate::diag::report(&app2, failure);
                                } else {
                                    crate::diag::clear_last_error();
                                }
                                // Log words + speaking time for the Hub stats (WPM, streak…).
                                record_dictation(&text, res.duration_secs());
                                // Watch the field for a post-paste correction to learn (✨).
                                crate::autolearn::watch_correction(&text);
                            } else if was_real_attempt {
                                crate::diag::report(&app2, whimpr_core::InjectionFailure::EmptyTranscript);
                            }
                            let _ = app2.emit_to(
                                OVERLAY_LABEL,
                                "whimpr://transcript",
                                TranscriptPayload { text },
                            );
                        }
                        Err(e) => {
                            eprintln!("[whimpr] ASR error: {e}");
                            if was_real_attempt {
                                crate::diag::report(&app2, whimpr_core::InjectionFailure::TranscriptionFailed);
                            }
                        }
                    }
                    finish();
                });
            }
            Action::DiscardCapture { .. } => {
                if let Some(slot) = CAPTURE.get() {
                    if let Some(handle) = slot.lock().unwrap().take() {
                        let _ = handle.stop();
                    }
                }
            }
            // The ASR path (StopCaptureAndFinalize) now drives pipeline completion.
            Action::RunPipeline { .. } => {}
            Action::PlayPing => play_start_cue(),
            // WarnSessionCap: no-op for now.
            _ => {}
        }
    }

    extern "C" fn tap_callback(
        _proxy: CGEventTapProxy,
        etype: u32,
        event: CGEventRef,
        _info: *mut c_void,
    ) -> CGEventRef {
        if etype == K_CG_TAP_DISABLED_BY_TIMEOUT || etype == K_CG_TAP_DISABLED_BY_USER_INPUT {
            let port = TAP_PORT.load(Ordering::SeqCst);
            if !port.is_null() {
                unsafe { CGEventTapEnable(port, true) };
            }
            return event;
        }
        if etype == K_CG_EVENT_FLAGS_CHANGED {
            let keycode =
                unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };
            if keycode == KEYCODE_FN {
                let flags = unsafe { CGEventGetFlags(event) };
                let down = (flags & FLAG_SECONDARY_FN) != 0;
                let was_down = FN_IS_DOWN.swap(down, Ordering::SeqCst);
                let at_ms = now_ms();
                if down && !was_down {
                    eprintln!("[whimpr] Fn DOWN");
                    // Snapshot the paste target now, while the user's app is focused.
                    let target = crate::appctx::frontmost_bundle_id();
                    *TARGET_APP.get_or_init(|| Mutex::new(None)).lock().unwrap() = target;
                    handle_input(Input::Trigger(TriggerToken::Down {
                        binding: BindingId::PushToTalk,
                        at_ms,
                    }));
                } else if !down && was_down {
                    eprintln!("[whimpr] Fn UP");
                    handle_input(Input::Trigger(TriggerToken::Up {
                        binding: BindingId::PushToTalk,
                        at_ms,
                    }));
                }
            }
        }
        event
    }

    /// Stop and finalize the current recording — the pill's red Stop button.
    /// Drives the same state machine the Fn key does, so it works in either
    /// mode. Reported dead in Publik Test 2 ("the red with the square in it").
    pub fn stop_dictation() {
        handle_input(Input::Trigger(TriggerToken::Stop { at_ms: now_ms() }));
    }

    /// Discard the current recording — the pill's ✕ button. Same path Esc would
    /// take, if Esc were wired. Reported dead in Publik Test 2 ("the X button
    /// doesn't work").
    pub fn cancel_dictation() {
        handle_input(Input::Trigger(TriggerToken::Cancel { at_ms: now_ms() }));
    }

    /// Toggle HANDS-FREE (locked) dictation — the customizable global hotkey
    /// (default Cmd+Shift+Space) fires this. The state machine treats a
    /// `HandsFree` press as a toggle: from idle it starts a locked session that
    /// keeps recording with no key held, and while locked it finalizes. This is
    /// the "speak without having to hold down fn" ask from Publik Test 2.
    pub fn trigger_hands_free() {
        handle_input(Input::Trigger(TriggerToken::Down {
            binding: BindingId::HandsFree,
            at_ms: now_ms(),
        }));
    }

    pub fn install(app: AppHandle) {
        let _ = APP.set(app);
        let _ = MACHINE.set(Mutex::new(StateMachine::new()));
        let _ = CLOCK.set(Instant::now());

        // Find and load the speech-to-text model off the main thread (~1s).
        crate::asr_model::start(APP.get().expect("APP set above"));

        // Load settings + dictionary, and build cloud providers from stored keys.
        let settings = whimpr_core::Settings::load(&settings_path());
        let dict = whimpr_core::DictionaryStore::load(&dict_path());
        eprintln!(
            "[whimpr] cleanup mode: {:?}, level: {:?}",
            settings.cleanup_mode, settings.cleanup_level
        );
        let _ = SETTINGS.set(Mutex::new(settings));
        let _ = DICTIONARY.set(Mutex::new(dict));
        let _ = STATS.set(Mutex::new(whimpr_core::StatsStore::load(&stats_path())));
        rebuild_providers();

        // Start the local cleanup worker in the background (model load takes a few
        // seconds; the first local cleanup waits for it, subsequent ones are fast).
        std::thread::spawn(|| {
            let worker = crate::local_llm::spawn_default();
            let _ = LOCAL.set(Mutex::new(worker));
        });

        // Accessibility is the ONE permission that makes the Fn CGEventTap global AND
        // lets us post the Cmd+V paste into other apps. Without it, a keyboard tap is
        // silently limited to frontmost-only — the exact bug. Prompt for it up front.
        if crate::paste::is_trusted() {
            eprintln!("[whimpr] Accessibility granted — Fn works in every app, paste enabled");
        } else {
            eprintln!(
                "[whimpr] ⚠ Accessibility NOT granted — clearing any stale TCC entry, \
                 re-prompting, and opening System Settings → Privacy & Security → \
                 Accessibility (no relaunch needed)."
            );
            // Self-heal: `is_trusted()` can stay permanently false even after the
            // user grants (and re-grants) Accessibility in System Settings, because
            // `tauri build` ad-hoc-signs every build with a fresh code signature and
            // a TCC row tied to an OLDER signature never transfers to a new one —
            // toggling the checkbox then just re-toggles that STALE row and nothing
            // downstream of `is_trusted()` ever notices, matching the reports
            // ("reinstalling/re-toggling doesn't fix it"). Clear the stale row for
            // THIS build's own identifier (read from its own config, not assumed,
            // so this is correct under whatever bundle id this binary actually
            // ships under) so the grant the user makes next lands against this
            // build's real signature, then re-fire the native prompt. Spawned so a
            // slow tccutil call never delays the rest of startup below.
            let bundle_id = APP
                .get()
                .map(|a| a.config().identifier.clone())
                .unwrap_or_else(|| "com.whimpr.whimprflow".to_string());
            std::thread::spawn(move || {
                match std::process::Command::new("/usr/bin/tccutil")
                    .args(["reset", "Accessibility", &bundle_id])
                    .output()
                {
                    Ok(out) if out.status.success() => eprintln!(
                        "[whimpr] tccutil reset done: {}",
                        String::from_utf8_lossy(&out.stdout).trim()
                    ),
                    Ok(out) => eprintln!(
                        "[whimpr] accessibility self-heal: tccutil exited with {}: {}",
                        out.status,
                        String::from_utf8_lossy(&out.stderr).trim()
                    ),
                    Err(e) => eprintln!(
                        "[whimpr] accessibility self-heal: failed to run tccutil: {e}"
                    ),
                }
                crate::paste::prompt_accessibility();
                crate::open_url(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                );
            });
        }
        // Input Monitoring is NOT the gate for a CGEventTap — kept only as diagnostics.
        eprintln!(
            "[whimpr] (info) Input Monitoring: {}",
            crate::paste::input_monitoring_granted()
        );

        // Periodic tick drives the double-tap timeout / session cap.
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_millis(100));
            handle_input(Input::Tick { now_ms: now_ms() });
        });

        // The event tap runs on a thread with its own CFRunLoop. CRITICAL: create it
        // ONLY after the process is trusted for Accessibility. macOS fixes a keyboard
        // tap's privilege at CGEventTapCreate time — a tap born untrusted is
        // permanently frontmost-only and is NOT upgraded when the grant later arrives.
        // Polling here also means the Fn key starts working the moment the user grants
        // Accessibility, without a relaunch.
        std::thread::spawn(|| {
            while !crate::paste::is_trusted() {
                std::thread::sleep(Duration::from_millis(500));
            }
            eprintln!("[whimpr] Accessibility present — creating global Fn tap");
            // Bug fix: this used to try CGEventTapCreate exactly once and, if it
            // came back null despite Accessibility being granted (the real-world
            // stale-TCC-entry case this app's own comments already knew about),
            // give up FOREVER with only an eprintln — the Fn key would then do
            // nothing for the rest of the run, indistinguishable from "text isn't
            // typed" to the user, with zero visible explanation. Now: report it
            // loudly once, and keep retrying — toggling the Accessibility entry
            // off/on in System Settings, or removing and re-adding it, fixes the
            // stale grant without requiring a relaunch, and this way that fix is
            // picked up automatically.
            let mut reported = false;
            let port = loop {
                let port = unsafe {
                    CGEventTapCreate(
                        K_CG_SESSION_EVENT_TAP,
                        K_CG_HEAD_INSERT,
                        K_CG_TAP_OPTION_LISTEN_ONLY,
                        EVENTS_OF_INTEREST,
                        tap_callback,
                        null_mut(),
                    )
                };
                if !port.is_null() {
                    break port;
                }
                eprintln!(
                    "[whimpr] Fn tap null despite Accessibility — likely a stale TCC entry from \
                     an earlier build. Run: tccutil reset Accessibility com.whimpr.whimprflow, \
                     or toggle WhimprFlow off/on under System Settings → Privacy & Security → \
                     Accessibility. Retrying…"
                );
                if !reported {
                    if let Some(app) = APP.get() {
                        crate::diag::report(app, whimpr_core::InjectionFailure::HotkeyTapFailed);
                    }
                    reported = true;
                }
                std::thread::sleep(Duration::from_secs(5));
            };
            if reported {
                eprintln!("[whimpr] Fn tap recovered — the key is live now.");
                crate::diag::clear_last_error();
            }
            TAP_PORT.store(port, Ordering::SeqCst);
            unsafe {
                let source = CFMachPortCreateRunLoopSource(null(), port, 0);
                CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopDefaultMode);
                CGEventTapEnable(port, true);
                CFRunLoopRun();
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub use imp::{
    cancel_dictation, current_settings, dictionary_add, dictionary_entries, dictionary_learn,
    dictionary_remove, history, install, rebuild_providers, stats_summary, stop_dictation,
    trigger_hands_free, update_settings,
};

// Windows uses the real (but unverified) platform layer in `crate::win`.
#[cfg(target_os = "windows")]
pub use crate::win::{
    cancel_dictation, current_settings, dictionary_add, dictionary_entries, dictionary_learn,
    dictionary_remove, history, install, rebuild_providers, stats_summary, stop_dictation,
    trigger_hands_free, update_settings,
};

// Linux: real platform layer (rdev hotkey + xdotool/ydotool paste + full pipeline).
#[cfg(target_os = "linux")]
pub use crate::linux::{
    cancel_dictation, current_settings, dictionary_add, dictionary_entries, dictionary_learn,
    dictionary_remove, history, install, rebuild_providers, stats_summary, stop_dictation,
    trigger_hands_free, update_settings,
};

// Other platforms (none currently shipped): inert stubs so the crate still builds.
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod other {
    pub fn install(_app: tauri::AppHandle) {}
    pub fn current_settings() -> whimpr_core::Settings {
        whimpr_core::Settings::default()
    }
    pub fn update_settings(_new: whimpr_core::Settings) {}
    pub fn rebuild_providers() {}
    pub fn stats_summary(tz_offset_minutes: i32) -> whimpr_core::StatsSummary {
        whimpr_core::StatsStore::default().summary(tz_offset_minutes, 0)
    }
    pub fn history(_limit: usize) -> Vec<whimpr_core::HistoryItem> {
        Vec::new()
    }
    pub fn dictionary_entries() -> Vec<super::DictEntryDto> {
        Vec::new()
    }
    pub fn dictionary_add(_correct: String, _mishears: Vec<String>) {}
    pub fn dictionary_remove(_correct: &str) {}
    pub fn dictionary_learn(_correct: String, _mishears: Vec<String>) {}
    pub fn stop_dictation() {}
    pub fn cancel_dictation() {}
    pub fn trigger_hands_free() {}
}
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub use other::{
    cancel_dictation, current_settings, dictionary_add, dictionary_entries, dictionary_learn,
    dictionary_remove, history, install, rebuild_providers, stats_summary, stop_dictation,
    trigger_hands_free, update_settings,
};
