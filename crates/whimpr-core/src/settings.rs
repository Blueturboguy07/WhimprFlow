//! User settings, persisted as JSON. Drives the cleanup engine (which provider,
//! how aggressive) and other behavior. Kept dependency-light so it lives in core.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cleanup::CleanupLevel;

/// Convert one display's reported geometry into a logical-point work area.
///
/// Tauri/tao report macOS displays in MIXED units, verified against a real
/// three-display setup (2× built-in at (0,0), 1× at x=1728, 1× at x=4288 — those
/// x origins are the *logical* widths stacked up, while the sizes are physical
/// pixels):
///
/// * `position`             — logical points, global
/// * `size`                 — physical pixels
/// * `work_area.position.x` — logical points, global (equals `position.x`)
/// * `work_area.position.y` — physical pixels, inset from the top of THIS display
/// * `work_area.size`       — physical pixels
///
/// Returns `(left, top, width, height)` in logical points. It lives here, away
/// from the windowing code, so it can be tested with no display attached.
pub fn work_area_points(
    monitor_y: i32,
    work_x: i32,
    work_y: i32,
    work_w: u32,
    work_h: u32,
    scale: f64,
) -> (f64, f64, f64, f64) {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    (
        work_x as f64,
        monitor_y as f64 + work_y as f64 / scale,
        work_w as f64 / scale,
        work_h as f64 / scale,
    )
}

/// Bottom-centre placement for a window of `win_w` × `win_h` logical points,
/// clamped so it can never sit outside the work area — i.e. never under the Dock.
pub fn pill_placement(
    area: (f64, f64, f64, f64),
    win_w: f64,
    win_h: f64,
    bottom_inset: f64,
) -> (f64, f64) {
    let (ax, ay, aw, ah) = area;
    let x = ax + (aw - win_w) / 2.0;
    let y = ay + ah - win_h - bottom_inset;
    // The upper bounds are floored at the work-area origin so that a window
    // LARGER than the work area pins to its top-left rather than being pushed off
    // the top of the screen — and so `clamp` is never handed min > max, which
    // would panic.
    let max_x = (ax + aw - win_w).max(ax);
    let max_y = (ay + ah - win_h).max(ay);
    (x.clamp(ax, max_x), y.clamp(ay, max_y))
}

/// Which cleanup engine processes transcripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupMode {
    /// Paste the raw transcript (no cleanup).
    Raw,
    /// Local on-device model (default — works offline, no API key).
    #[default]
    Local,
    /// publik API — the cloud option that is already set up. Priced per use in
    /// dollars at 50% of the model's published list price; the key is minted for
    /// this install and lives in the OS keychain next to the user's own keys.
    /// Never the default: WhimprFlow is local-first and only goes to the cloud
    /// when asked.
    Publik,
    /// OpenAI cloud.
    OpenAi,
    /// Anthropic cloud.
    Anthropic,
}

/// Which engine transcribes the recorded audio to text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AsrMode {
    /// Local on-device Whisper (default — works offline, no API key).
    #[default]
    Local,
    /// Cloud speech-to-text via an OpenAI-compatible `/audio/transcriptions`
    /// API (OpenAI itself, or Groq's Whisper endpoint, or any compatible
    /// host). Reuses the same stored key as the "OpenAI" cleanup mode.
    Cloud,
}

fn default_asr_model() -> String {
    "whisper-large-v3-turbo".to_string()
}

/// Persisted user configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub cleanup_mode: CleanupMode,
    pub cleanup_level: CleanupLevel,
    pub openai_model: String,
    /// API root for the "OpenAI" cleanup mode, e.g. `https://openrouter.ai/api/v1`
    /// to route through OpenRouter instead of OpenAI directly (same wire format).
    /// Empty string (the default) means OpenAI's own endpoint.
    #[serde(default)]
    pub openai_base_url: String,
    pub anthropic_model: String,
    /// Which engine transcribes speech to text.
    #[serde(default)]
    pub asr_mode: AsrMode,
    /// API root for `AsrMode::Cloud`, e.g. `https://api.groq.com/openai/v1`
    /// for Groq's fast Whisper endpoint. Empty string means OpenAI's own
    /// endpoint. Uses the same stored key as the "OpenAI" cleanup mode.
    #[serde(default)]
    pub asr_base_url: String,
    #[serde(default = "default_asr_model")]
    pub asr_model: String,
    /// Play the record-start ping.
    pub sound_on_start: bool,
    /// The push-to-talk key, stored as an rdev::Key variant string (e.g.
    /// "ControlRight"). Defaults to Right Ctrl. Only meaningful on platforms
    /// that read it (currently Linux's rdev-based hook); macOS/Windows use
    /// their own native key constant today.
    #[serde(default = "default_push_to_talk_key")]
    pub push_to_talk_key: String,
    /// The global hotkey that toggles HANDS-FREE (locked) dictation — press once
    /// to start talking, press again to stop, with no key held down. An
    /// accelerator string in Tauri's format (e.g. "CmdOrCtrl+Shift+Space", the
    /// default). This is the "speak without holding fn … a combination of
    /// buttons … customization in settings" ask from Publik Test 2. Holding Fn
    /// (push-to-talk) and double-tapping Fn (hands-free) still work regardless.
    /// An empty string disables the hands-free hotkey.
    #[serde(default = "default_hands_free_hotkey")]
    pub hands_free_hotkey: String,
    /// The version of the publik cost + data-path disclosure the user has
    /// accepted in this app. 0 = never accepted. Bump
    /// [`PUBLIK_DISCLOSURE_VERSION`] when the wording changes and the card
    /// shows again once.
    #[serde(default)]
    pub publik_disclosure_version: u32,
    /// Random v4 UUID minted on first provisioning (not a hardware id). It is
    /// the idempotency key of `POST /installs`, so a retried provision never
    /// mints a second key for the same install. Empty until first provision.
    #[serde(default)]
    pub publik_install_id: String,
    /// API root handed back by `POST /installs` (`base_url`). Honoured over the
    /// compiled default; empty means "use the default". Not a UI field.
    #[serde(default)]
    pub publik_base_url: String,
    /// The fast-tier alias handed back by `POST /installs` (`models.fast`).
    /// Empty means the compiled default alias. Never an upstream model slug.
    #[serde(default)]
    pub publik_model: String,
    /// Claim link for this install (`claim_url`), kept so the Settings card can
    /// offer "Link this computer" after a relaunch without a network round trip.
    /// Empty once claimed or before provisioning.
    #[serde(default)]
    pub publik_claim_url: String,
    /// Free starter usage granted by `POST /installs` (`starter_micros`), kept
    /// so the "starter running low" banner knows what 20% of it is. 0 = unknown
    /// (the gateway's documented anonymous starter is assumed).
    #[serde(default)]
    pub publik_starter_micros: i64,
    /// The first-run publik card (balance line, why it costs money, "Link this
    /// computer & pick a plan") is owed to the user: set by provisioning,
    /// cleared by "Later" or the button. Persisted so an app that quits before
    /// the card was seen shows it on the next launch (CONTRACT §12.4: never a
    /// silent starter).
    #[serde(default)]
    pub publik_cta_pending: bool,
}

/// Version of the in-app publik disclosure copy. Bumping it re-shows the card.
pub const PUBLIK_DISCLOSURE_VERSION: u32 = 1;

/// The out-of-the-box hands-free hotkey. Chosen to match what the cofounder
/// already expected to work ("command-shift space for the hands-off
/// transcribing") and to stay clear of the common macOS system shortcuts
/// (Cmd+Space is Spotlight, Ctrl+Cmd+Space is the emoji picker).
pub fn default_hands_free_hotkey() -> String {
    "CmdOrCtrl+Shift+Space".to_string()
}

fn default_push_to_talk_key() -> String {
    "ControlRight".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cleanup_mode: CleanupMode::default(),
            cleanup_level: CleanupLevel::Light,
            openai_model: "gpt-4o-mini".to_string(),
            openai_base_url: String::new(),
            anthropic_model: "claude-haiku-4-5".to_string(),
            asr_mode: AsrMode::default(),
            asr_base_url: String::new(),
            asr_model: default_asr_model(),
            sound_on_start: true,
            push_to_talk_key: default_push_to_talk_key(),
            hands_free_hotkey: default_hands_free_hotkey(),
            publik_disclosure_version: 0,
            publik_install_id: String::new(),
            publik_base_url: String::new(),
            publik_model: String::new(),
            publik_claim_url: String::new(),
            publik_starter_micros: 0,
            publik_cta_pending: false,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real values captured from a 2× built-in plus two 1× externals. If the pill
    // ever drifts under the Dock again, these pin down why.
    const BUILTIN: (i32, i32, i32, u32, u32, f64) = (0, 0, 66, 3456, 1982, 2.0);
    const DELL: (i32, i32, i32, u32, u32, f64) = (0, 1728, 30, 2560, 1410, 1.0);
    const HP: (i32, i32, i32, u32, u32, f64) = (0, 4288, 30, 1920, 1050, 1.0);

    fn area(m: (i32, i32, i32, u32, u32, f64)) -> (f64, f64, f64, f64) {
        work_area_points(m.0, m.1, m.2, m.3, m.4, m.5)
    }

    #[test]
    fn retina_work_area_is_converted_to_points() {
        // 3456 physical / 2 = 1728 points wide; the 66px menu bar is 33 points.
        assert_eq!(area(BUILTIN), (0.0, 33.0, 1728.0, 991.0));
    }

    #[test]
    fn non_retina_displays_keep_their_global_offset() {
        assert_eq!(area(DELL), (1728.0, 30.0, 2560.0, 1410.0));
        assert_eq!(area(HP), (4288.0, 30.0, 1920.0, 1050.0));
    }

    #[test]
    fn displays_do_not_overlap_in_point_space() {
        // Each display must start where the previous one ends, or the cursor
        // hit-test lands on the wrong screen — which is exactly what happened.
        let (bx, _, bw, _) = area(BUILTIN);
        let (dx, _, dw, _) = area(DELL);
        let (hx, ..) = area(HP);
        assert_eq!(bx + bw, dx);
        assert_eq!(dx + dw, hx);
    }

    #[test]
    fn pill_sits_above_the_dock_on_the_retina_screen() {
        let a = area(BUILTIN);
        let (x, y) = pill_placement(a, 320.0, 132.0, 12.0);
        assert_eq!(x, 704.0); // centred: (1728 - 320) / 2
        assert_eq!(y, 880.0); // 33 + 991 - 132 - 12
        // The Dock starts at 1024 points; the pill must end before it.
        assert!(y + 132.0 <= 1024.0);
    }

    /// The bug that survived several rounds: the window's size was derived from
    /// physical pixels on the display it was leaving, divided by the scale of the
    /// display it was arriving at. Across a 1× → 2× boundary that halved it, and
    /// the pill ended up below the work area — under the Dock.
    #[test]
    fn a_halved_window_size_would_put_the_pill_under_the_dock() {
        let a = area(BUILTIN); // work area ends at 33 + 991 = 1024
        let (_, correct) = pill_placement(a, 320.0, 132.0, 12.0);
        assert!(correct + 132.0 <= 1024.0);

        // Same call with the size halved, as the old code computed it.
        let (_, halved) = pill_placement(a, 160.0, 70.0, 12.0);
        assert_eq!(halved, 942.0, "reproduces the value seen in the logs");
        assert!(
            halved + 132.0 > 1024.0,
            "the real 132pt-tall pill overhangs the work area by 50pt"
        );
    }

    #[test]
    fn pill_lands_on_the_right_external_screen() {
        let (x, y) = pill_placement(area(HP), 320.0, 132.0, 12.0);
        assert_eq!(x, 5088.0); // 4288 + (1920 - 320) / 2
        assert_eq!(y, 936.0);
    }

    #[test]
    fn placement_is_clamped_inside_the_work_area() {
        // A window taller than the work area must pin to the top of it, not be
        // pushed off the top of the screen and under the menu bar.
        let a = area(HP);
        let (x, y) = pill_placement(a, 320.0, 5000.0, 12.0);
        assert_eq!(y, a.1);
        assert!(x >= a.0);
    }

    #[test]
    fn placement_survives_a_window_wider_than_the_screen() {
        let a = area(BUILTIN);
        let (x, y) = pill_placement(a, 9999.0, 132.0, 12.0);
        assert_eq!(x, a.0);
        assert!(y >= a.1);
    }

    #[test]
    fn a_dock_appearing_moves_the_pill_up() {
        let without = pill_placement(area(HP), 320.0, 132.0, 12.0);
        // Same display, work area shortened by a 70pt Dock.
        let with_dock =
            pill_placement(work_area_points(0, 4288, 30, 1920, 980, 1.0), 320.0, 132.0, 12.0);
        assert!(with_dock.1 < without.1);
    }

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert_eq!(s.cleanup_mode, CleanupMode::Local);
        assert_eq!(s.cleanup_level, CleanupLevel::Light);
    }

    #[test]
    fn round_trips_json() {
        let s = Settings {
            cleanup_mode: CleanupMode::Local,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.cleanup_mode, CleanupMode::Local);
    }

    #[test]
    fn hands_free_hotkey_defaults_and_survives_old_settings() {
        // A fresh install gets the Cmd+Shift+Space default.
        assert_eq!(Settings::default().hands_free_hotkey, "CmdOrCtrl+Shift+Space");

        // Settings written by a build BEFORE this field existed must still load —
        // the field is `#[serde(default)]`, so it fills in rather than failing.
        let old_json = r#"{
            "cleanup_mode": "local",
            "cleanup_level": "light",
            "openai_model": "gpt-4o-mini",
            "anthropic_model": "claude-haiku-4-5",
            "sound_on_start": true
        }"#;
        let loaded: Settings = serde_json::from_str(old_json).unwrap();
        assert_eq!(loaded.hands_free_hotkey, "CmdOrCtrl+Shift+Space");

        // A user who set their own combo keeps it.
        let mut custom = Settings::default();
        custom.hands_free_hotkey = "Alt+Space".to_string();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&custom).unwrap()).unwrap();
        assert_eq!(back.hands_free_hotkey, "Alt+Space");
    }

    #[test]
    fn publik_fields_survive_old_settings() {
        // A settings.json written before publik existed must still load, with
        // Local still selected and nothing provisioned.
        let old_json = r#"{
            "cleanup_mode": "local",
            "cleanup_level": "light",
            "openai_model": "gpt-4o-mini",
            "anthropic_model": "claude-haiku-4-5",
            "sound_on_start": true
        }"#;
        let loaded: Settings = serde_json::from_str(old_json).unwrap();
        assert_eq!(loaded.cleanup_mode, CleanupMode::Local);
        assert_eq!(loaded.publik_disclosure_version, 0);
        assert_eq!(loaded.publik_install_id, "");
        assert_eq!(loaded.publik_base_url, "");
        assert_eq!(loaded.publik_model, "");
        assert_eq!(loaded.publik_claim_url, "");
        assert_eq!(loaded.publik_starter_micros, 0);
        assert!(!loaded.publik_cta_pending, "no card is owed before anything was provisioned");
    }

    #[test]
    fn publik_mode_round_trips() {
        let s = Settings {
            cleanup_mode: CleanupMode::Publik,
            publik_disclosure_version: PUBLIK_DISCLOSURE_VERSION,
            publik_install_id: "3f1c9b5e-7a2d-4c8e-9f0b-1d2e3f4a5b6c".to_string(),
            publik_base_url: "https://publikhq.com/api/v1".to_string(),
            publik_starter_micros: 250_000,
            publik_cta_pending: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"cleanup_mode\":\"publik\""), "{json}");
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.cleanup_mode, CleanupMode::Publik);
        assert_eq!(back.publik_install_id, s.publik_install_id);
        assert_eq!(back.publik_base_url, s.publik_base_url);
        assert_eq!(back.publik_starter_micros, 250_000);
        assert!(back.publik_cta_pending, "the owed first-run card survives a relaunch");
    }

    #[test]
    fn local_stays_the_default_even_with_publik_available() {
        // Tier 2: publik is offered, never preselected. Nothing in this crate
        // may ever make Publik the default.
        assert_eq!(CleanupMode::default(), CleanupMode::Local);
        assert_eq!(Settings::default().cleanup_mode, CleanupMode::Local);
    }
}
