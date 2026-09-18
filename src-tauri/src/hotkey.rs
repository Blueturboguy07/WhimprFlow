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
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicPtr, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
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
    /// The push-to-talk binding, held in atomics because the tap callback runs on
    /// every flags-changed event and must not take a lock to read it.
    static PTT_KEYCODE: AtomicI64 = AtomicI64::new(KEYCODE_FN);
    static PTT_MASK: AtomicU64 = AtomicU64::new(FLAG_SECONDARY_FN);
    const K_CG_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
    const K_CG_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

    static APP: OnceLock<AppHandle> = OnceLock::new();
    static MACHINE: OnceLock<Mutex<StateMachine>> = OnceLock::new();
    static CLOCK: OnceLock<Instant> = OnceLock::new();
    static FN_IS_DOWN: AtomicBool = AtomicBool::new(false);
    static TAP_PORT: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
    /// Set once at startup if no Whisper model file exists on disk at all —
    /// distinct from "still loading", so the finalize path only shows the
    /// user a loud "no speech model" error for the real case, not a race
    /// against the ~1s background load right after launch.
    static ASR_MODEL_MISSING: AtomicBool = AtomicBool::new(false);
    /// Bundle id of the app that was frontmost at record-start = the paste target.
    /// Cleanup uses it to format for the medium (email vs. text vs. chat).
    static TARGET_APP: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    static CAPTURE: OnceLock<Mutex<Option<whimpr_audio::CaptureHandle>>> = OnceLock::new();
    static ASR: OnceLock<Arc<whimpr_asr::WhisperEngine>> = OnceLock::new();
    static OPENAI: OnceLock<Mutex<Option<whimpr_cleanup::OpenAiProvider>>> = OnceLock::new();
    static ANTHROPIC: OnceLock<Mutex<Option<whimpr_cleanup::AnthropicProvider>>> = OnceLock::new();
    static LOCAL: OnceLock<Mutex<Option<crate::local_llm::LocalWorker>>> = OnceLock::new();
    static SETTINGS: OnceLock<Mutex<whimpr_core::Settings>> = OnceLock::new();
    static SNIPPETS: OnceLock<Mutex<whimpr_core::SnippetStore>> = OnceLock::new();
    static TRANSFORMS: OnceLock<Mutex<whimpr_core::TransformStore>> = OnceLock::new();
    /// Last state pushed to the pill, so UI-driven controls can act correctly.
    static LAST_BAR: OnceLock<Mutex<&'static str>> = OnceLock::new();
    /// When set, finished dictations are sent to the Hub's Scratchpad instead of
    /// being pasted into the frontmost app.
    static SCRATCHPAD_CAPTURE: AtomicBool = AtomicBool::new(false);
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

    /// The whisper ASR model to load: prefer the most accurate one present, in
    /// descending quality order, falling back to the small base model. Bigger
    /// English models mis-hear names/technical terms far less (and better ASR means
    /// less for cleanup and the dictionary to fix downstream).
    fn model_path() -> PathBuf {
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
    fn snippets_path() -> PathBuf {
        support_dir().join("snippets.json")
    }
    fn transforms_path() -> PathBuf {
        support_dir().join("transforms.json")
    }
    pub fn scratchpad_path() -> PathBuf {
        support_dir().join("scratchpad.txt")
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
        refresh_asr_vocabulary();
    }

    /// Remove a dictionary entry by spelling and persist.
    pub fn dictionary_remove(correct: &str) {
        if let Some(m) = DICTIONARY.get() {
            let mut store = m.lock().unwrap();
            if store.remove(correct) {
                let _ = store.save(&dict_path());
            }
        }
        refresh_asr_vocabulary();
    }

    /// Add an AUTO-learned entry (from the post-paste correction observer) and persist.
    /// Marked ✨ auto in the UI. No-op if it would duplicate an existing entry's data.
    pub fn dictionary_learn(correct: String, mishears: Vec<String>) {
        if let Some(m) = DICTIONARY.get() {
            let mut store = m.lock().unwrap();
            store.add(correct, mishears, whimpr_core::DictSource::Auto);
            let _ = store.save(&dict_path());
        }
        refresh_asr_vocabulary();
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
        apply_live_settings(&new);
        rebuild_providers();
    }

    // ── Pill controls ────────────────────────────────────────────────────────
    // The pill's Stop and Cancel buttons, and click-to-start, all feed the SAME
    // state machine the Fn key does — rather than a parallel code path that could
    // drift out of sync with it.
    fn last_bar() -> &'static str {
        LAST_BAR
            .get()
            .map(|m| *m.lock().unwrap())
            .unwrap_or("idle")
    }

    /// Discard the in-flight dictation. Same input Esc produces.
    pub fn ui_cancel() {
        handle_input(Input::Trigger(TriggerToken::Cancel { at_ms: now_ms() }));
    }

    /// Finish now and insert what has been said so far.
    pub fn ui_stop() {
        let t = now_ms();
        if last_bar() == "locked" {
            // Hands-free: a tap of the key is what ends it.
            handle_input(Input::Trigger(TriggerToken::Down {
                binding: BindingId::PushToTalk,
                at_ms: t,
            }));
            handle_input(Input::Trigger(TriggerToken::Up {
                binding: BindingId::PushToTalk,
                at_ms: t + 1,
            }));
        } else {
            // Held: releasing finalises it.
            handle_input(Input::Trigger(TriggerToken::Up {
                binding: BindingId::PushToTalk,
                at_ms: t,
            }));
        }
    }

    /// Start a hands-free dictation from a click on the pill.
    ///
    /// The machine only enters hands-free via a double-tap, so this synthesises
    /// one: a short tap (under HOLD_MIN_MS so it is read as a tap, not a hold),
    /// then a second press inside the DOUBLE_TAP_MS window, which locks it.
    pub fn ui_start() {
        if last_bar() != "idle" {
            return;
        }
        let t = now_ms();
        let ptt = BindingId::PushToTalk;
        handle_input(Input::Trigger(TriggerToken::Down { binding: ptt, at_ms: t }));
        handle_input(Input::Trigger(TriggerToken::Up { binding: ptt, at_ms: t + 10 }));
        handle_input(Input::Trigger(TriggerToken::Down { binding: ptt, at_ms: t + 20 }));
        handle_input(Input::Trigger(TriggerToken::Up { binding: ptt, at_ms: t + 30 }));
    }

    // ── Audio cues ───────────────────────────────────────────────────────────
    #[derive(Clone, Copy)]
    pub enum Cue {
        /// Recording has begun — the "I'm listening" confirmation.
        Start,
        /// Text has been inserted.
        Done,
        /// Cancelled, or nothing usable was heard.
        Cancel,
    }

    /// Play a short macOS system sound.
    ///
    /// `afplay` rather than NSSound: no extra dependency, no main-thread
    /// requirement, and it can be fired from the event-tap thread without any
    /// risk of blocking key handling. The child is reaped on its own thread so
    /// repeated dictations don't leave zombie processes behind.
    fn play_cue(cue: Cue) {
        if !current_settings().sound_on_start {
            return;
        }
        // Chosen to be short and unobtrusive: a soft click to start, a lighter
        // one to confirm, a duller one for a discard.
        let sound = match cue {
            Cue::Start => "Pop",
            Cue::Done => "Tink",
            Cue::Cancel => "Bottle",
        };
        let path = format!("/System/Library/Sounds/{sound}.aiff");
        std::thread::spawn(move || {
            let _ = std::process::Command::new("/usr/bin/afplay")
                .arg(&path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }

    // ── Transforms ───────────────────────────────────────────────────────────
    /// If the utterance opens with a transform trigger, run the transform and
    /// return its output.
    ///
    /// The deterministic gates are deliberately NOT applied here. They exist to
    /// catch a cleanup model rewriting when it was told only to tidy — but a
    /// transform is *asked* to rewrite, so every gate (over-deletion, novelty,
    /// hallucination) would reject a perfectly good result.
    fn try_transform(raw: &str, settings: &whimpr_core::Settings) -> Option<String> {
        let (name, prompt, body) = {
            let store = TRANSFORMS.get()?.lock().unwrap();
            let (t, body) = store.detect(raw)?;
            (t.name.clone(), t.prompt.clone(), body)
        };
        eprintln!("[whimpr] transform: {name}");

        let style = {
            let s = settings.style_instructions.trim();
            if s.is_empty() { None } else { Some(s.to_string()) }
        };
        let ctx = CleanupContext {
            level: settings.cleanup_level,
            transform_prompt: Some(prompt),
            style,
            ..Default::default()
        };

        let run_local = || -> Option<anyhow::Result<String>> {
            LOCAL.get().and_then(|m| {
                m.lock().unwrap().as_mut().map(|w| {
                    let messages = whimpr_core::cleanup::build_messages(&body, &ctx);
                    w.cleanup(&messages)
                })
            })
        };
        let result = match settings.cleanup_mode {
            CleanupMode::OpenAi => OPENAI
                .get()
                .and_then(|m| m.lock().unwrap().as_ref().map(|p| p.cleanup(&body, &ctx)))
                .or_else(run_local),
            CleanupMode::Anthropic => ANTHROPIC
                .get()
                .and_then(|m| m.lock().unwrap().as_ref().map(|p| p.cleanup(&body, &ctx)))
                .or_else(run_local),
            CleanupMode::Local => run_local(),
            CleanupMode::Raw => None,
        };

        match result {
            Some(Ok(out)) if !out.trim().is_empty() => {
                Some(whimpr_core::cleanup::post_process(&out))
            }
            Some(Err(e)) => {
                eprintln!("[whimpr] transform failed, using the words as spoken: {e}");
                // Better to paste what they said than to swallow the dictation.
                Some(body)
            }
            _ => Some(body),
        }
    }

    pub fn transforms() -> Vec<whimpr_core::Transform> {
        TRANSFORMS
            .get()
            .map(|m| m.lock().unwrap().items.clone())
            .unwrap_or_default()
    }

    pub fn transform_set_enabled(id: &str, enabled: bool) {
        if let Some(m) = TRANSFORMS.get() {
            let mut g = m.lock().unwrap();
            g.set_enabled(id, enabled);
            let _ = g.save(&transforms_path());
        }
    }

    // ── Snippets ─────────────────────────────────────────────────────────────
    fn expand_snippets(text: &str) -> String {
        SNIPPETS
            .get()
            .map(|m| m.lock().unwrap().expand(text))
            .unwrap_or_else(|| text.to_string())
    }

    pub fn snippets() -> Vec<whimpr_core::Snippet> {
        SNIPPETS
            .get()
            .map(|m| m.lock().unwrap().items.clone())
            .unwrap_or_default()
    }

    pub fn snippet_add(trigger: String, expansion: String) {
        if let Some(m) = SNIPPETS.get() {
            let mut g = m.lock().unwrap();
            g.add(&trigger, &expansion);
            let _ = g.save(&snippets_path());
        }
    }

    pub fn snippet_remove(trigger: &str) {
        if let Some(m) = SNIPPETS.get() {
            let mut g = m.lock().unwrap();
            g.remove(trigger);
            let _ = g.save(&snippets_path());
        }
    }

    // ── Scratchpad ───────────────────────────────────────────────────────────
    pub fn set_scratchpad_capture(on: bool) {
        SCRATCHPAD_CAPTURE.store(on, Ordering::Relaxed);
        eprintln!("[whimpr] scratchpad capture: {on}");
    }

    pub fn scratchpad_capture() -> bool {
        SCRATCHPAD_CAPTURE.load(Ordering::Relaxed)
    }

    /// Push the settings that other subsystems cache into place. Everything here
    /// takes effect immediately — no relaunch, no model reload.
    pub fn apply_live_settings(s: &whimpr_core::Settings) {
        let (keycode, mask) = s.push_to_talk_key.keycode_and_mask();
        PTT_KEYCODE.store(keycode, Ordering::Relaxed);
        PTT_MASK.store(mask, Ordering::Relaxed);

        if let Some(asr) = ASR.get() {
            asr.set_language(&s.language);
            asr.set_beam_search(s.noise_robust_asr);
            // Bias the speech model towards the user's own vocabulary. Until now
            // the Dictionary only reached cleanup, which meant a mis-heard name
            // had to be repaired afterwards rather than heard correctly.
            asr.set_vocabulary(&dictionary_terms());
        }
    }

    /// The spellings from the Dictionary, for Whisper's initial prompt.
    fn dictionary_terms() -> Vec<String> {
        DICTIONARY
            .get()
            .map(|d| {
                d.lock()
                    .unwrap()
                    .entries
                    .iter()
                    .map(|e| e.correct.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Re-send the vocabulary to the speech model after the Dictionary changes,
    /// so a word added now biases the very next dictation.
    pub fn refresh_asr_vocabulary() {
        if let Some(asr) = ASR.get() {
            asr.set_vocabulary(&dictionary_terms());
        }
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
        eprintln!(
            "[whimpr] cleanup providers: openai={}, anthropic={}",
            openai.is_some(),
            anthropic.is_some()
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
    }

    /// Clean a raw transcript per the current settings (mode + level), feeding in the
    /// dictionary vocabulary relevant to this utterance. Falls back to raw whenever
    /// cleanup is off, the provider is unavailable, it errors, or the gates reject it.
    fn clean_transcript(raw: &str) -> String {
        let settings = current_settings();
        let level = settings.cleanup_level;
        if matches!(settings.cleanup_mode, CleanupMode::Raw) || level.bypasses_llm() {
            return raw.to_string();
        }
        // A spoken transform command takes over the whole utterance.
        if let Some(out) = try_transform(raw, &settings) {
            return out;
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
        let style = {
            let s = settings.style_instructions.trim();
            if s.is_empty() { None } else { Some(s.to_string()) }
        };
        let ctx = CleanupContext {
            level,
            vocab,
            app_bundle_id,
            style,
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
            CleanupMode::Local => run_local(),
            CleanupMode::Raw => None,
        };
        match result {
            Some(Ok(cleaned)) => {
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
        // Remembered so the pill's Stop button knows whether it is ending a held
        // dictation (release) or a hands-free one (which needs a tap).
        *LAST_BAR.get_or_init(|| Mutex::new("idle")).lock().unwrap() = state;
        let _ = app.emit_to(OVERLAY_LABEL, "whimpr://flowbar/state", BarPayload { state });
        // Show/hide for "only while dictating" mode, and re-anchor onto the
        // display the user is actually on when a session starts.
        crate::sync_pill_visibility(app, state);
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
                    let device = current_settings().microphone;
                    match whimpr_audio::start_on_device(Some(device), move |bars| {
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
                    let Some(asr) = ASR.get().cloned() else {
                        eprintln!("[whimpr] ASR not ready (model still loading or missing)");
                        if was_real_attempt && ASR_MODEL_MISSING.load(Ordering::SeqCst) {
                            crate::diag::report(&app2, whimpr_core::InjectionFailure::AsrUnavailable);
                        }
                        finish();
                        return;
                    };
                    let pcm = whimpr_audio::resample_to_16k(&res.samples, res.sample_rate);
                    match asr.transcribe(&pcm) {
                        Ok(t) => {
                            let raw = t.text;
                            eprintln!("[whimpr] TRANSCRIPT: \"{}\"", raw);
                            // Clean the transcript (cloud LLM if configured), then paste.
                            let text = clean_transcript(&raw);
                            if text != raw {
                                eprintln!("[whimpr] CLEANED:   \"{}\"", text);
                            }
                            // Snippet expansion happens AFTER the cleanup gates:
                            // an expansion legitimately multiplies the text, which
                            // the over-deletion / novelty gates would otherwise
                            // read as the model going rogue.
                            let text = expand_snippets(&text);

                            if !text.is_empty() {
                                if SCRATCHPAD_CAPTURE.load(Ordering::Relaxed) {
                                    // Routed to the Hub instead of the frontmost app.
                                    let _ = app2.emit("whimpr://scratchpad/append", &text);
                                    eprintln!("[whimpr] routed to scratchpad");
                                } else if let Err(e) = crate::paste::paste_text(&text) {
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
                                play_cue(Cue::Done);
                                // Log words + speaking time for the Hub stats (WPM, streak…).
                                record_dictation(&text, res.duration_secs());
                                // Watch the field for a post-paste correction to learn (✨).
                                // Skipped while capturing to the Scratchpad: there is
                                // no target field to watch for a correction.
                                if !SCRATCHPAD_CAPTURE.load(Ordering::Relaxed) {
                                    crate::autolearn::watch_correction(&text);
                                }
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
                                crate::diag::report(&app2, whimpr_core::InjectionFailure::AsrUnavailable);
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
                play_cue(Cue::Cancel);
            }
            // The ASR path (StopCaptureAndFinalize) now drives pipeline completion.
            Action::RunPipeline { .. } => {}
            // The state machine emits this the moment recording starts. It was a
            // no-op, which made the "Play a sound when recording starts" toggle
            // purely decorative.
            Action::PlayPing => play_cue(Cue::Start),
            // WarnSessionCap and anything new: still no-ops.
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
            if keycode == PTT_KEYCODE.load(Ordering::Relaxed) {
                let flags = unsafe { CGEventGetFlags(event) };
                let down = (flags & PTT_MASK.load(Ordering::Relaxed)) != 0;
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

    pub fn install(app: AppHandle) {
        let _ = APP.set(app);
        let _ = MACHINE.set(Mutex::new(StateMachine::new()));
        let _ = CLOCK.set(Instant::now());

        // Load the speech-to-text model off the main thread (it takes ~1s).
        std::thread::spawn(|| {
            let path = model_path();
            if !path.exists() {
                eprintln!("[whimpr] ASR model not found at {}", path.display());
                ASR_MODEL_MISSING.store(true, Ordering::SeqCst);
                return;
            }
            match whimpr_asr::WhisperEngine::load(&path) {
                Ok(engine) => {
                    let _ = ASR.set(Arc::new(engine));
                    // Settings may already be loaded by now; if not, update_settings
                    // will push the language through later.
                    if let Some(asr) = ASR.get() {
                        asr.set_language(&current_settings().language);
                    }
                    eprintln!("[whimpr] ASR model loaded — ready to transcribe");
                }
                Err(e) => {
                    eprintln!("[whimpr] ASR model load failed: {e}");
                    ASR_MODEL_MISSING.store(true, Ordering::SeqCst);
                }
            }
        });

        // Load settings + dictionary, and build cloud providers from stored keys.
        let settings = whimpr_core::Settings::load(&settings_path());
        let dict = whimpr_core::DictionaryStore::load(&dict_path());
        eprintln!(
            "[whimpr] cleanup mode: {:?}, level: {:?}",
            settings.cleanup_mode, settings.cleanup_level
        );
        let _ = SETTINGS.set(Mutex::new(settings));
        let _ = DICTIONARY.set(Mutex::new(dict));
        let _ = SNIPPETS.set(Mutex::new(whimpr_core::SnippetStore::load(&snippets_path())));
        let _ = TRANSFORMS.set(Mutex::new(whimpr_core::TransformStore::load(&transforms_path())));
        let _ = STATS.set(Mutex::new(whimpr_core::StatsStore::load(&stats_path())));
        // Bind the push-to-talk key before the tap is created.
        apply_live_settings(&current_settings());
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
                "[whimpr] ⚠ Accessibility NOT granted — Fn only works while WhimprFlow is \
                 frontmost and paste is disabled. Prompting; grant WhimprFlow under System \
                 Settings → Privacy & Security → Accessibility (no relaunch needed)."
            );
            crate::paste::prompt_accessibility();
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
    current_settings, dictionary_add, dictionary_entries, dictionary_learn, dictionary_remove,
    history, install, rebuild_providers, scratchpad_capture, scratchpad_path, set_scratchpad_capture,
    snippet_add, snippet_remove, snippets, stats_summary, transform_set_enabled, transforms,
    ui_cancel, ui_start, ui_stop, update_settings,
};

// Windows uses the real (but unverified) platform layer in `crate::win`.
#[cfg(target_os = "windows")]
pub use crate::win::{
    current_settings, dictionary_add, dictionary_entries, dictionary_learn, dictionary_remove,
    history, install, rebuild_providers, stats_summary, update_settings,
};

// Other platforms (Linux, etc.): inert stubs so the crate still builds.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
    pub fn snippets() -> Vec<whimpr_core::Snippet> {
        Vec::new()
    }
    pub fn snippet_add(_trigger: String, _expansion: String) {}
    pub fn snippet_remove(_trigger: &str) {}
    pub fn set_scratchpad_capture(_on: bool) {}
    pub fn scratchpad_capture() -> bool {
        false
    }
    pub fn scratchpad_path() -> std::path::PathBuf {
        std::path::PathBuf::from("scratchpad.txt")
    }
    pub fn transforms() -> Vec<whimpr_core::Transform> {
        Vec::new()
    }
    pub fn transform_set_enabled(_id: &str, _enabled: bool) {}
    pub fn ui_cancel() {}
    pub fn ui_stop() {}
    pub fn ui_start() {}
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use other::{
    current_settings, dictionary_add, dictionary_entries, dictionary_learn, dictionary_remove,
    history, install, rebuild_providers, scratchpad_capture, scratchpad_path, set_scratchpad_capture,
    snippet_add, snippet_remove, snippets, stats_summary, transform_set_enabled, transforms,
    ui_cancel, ui_start, ui_stop, update_settings,
};
