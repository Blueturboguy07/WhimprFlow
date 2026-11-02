//! The push-to-talk key: its name per platform, its Windows virtual-key code,
//! and what to report when a hold ends with no audio.
//!
//! Every Hub screen used to name the macOS key ("hold Fn") on every platform.
//! Windows has no Fn virtual key — its push-to-talk key is Right Ctrl — and
//! the only places that said so were the README and a stderr line that release
//! builds hide (`windows_subsystem = "windows"`). Real Windows runs of 0.2.2
//! (GitHub Actions run 41374831510) found 0 mentions of "Right Ctrl" on Home,
//! Settings and Help, and 2 of "Fn". 11 of 13 Windows installs linked a publik
//! key and never completed a dictation. These helpers are the one place the
//! key is named and mapped, so the Hub, the hook and the log line agree.

use crate::diagnostics::InjectionFailure;

/// A hold shorter than this is an accidental tap, not a dictation attempt —
/// the same gate the macOS and Windows pipelines apply to a silent recording.
pub const MIN_REAL_ATTEMPT_MS: u64 = 600;

/// Human-readable label for a `Settings::push_to_talk_key` variant string
/// ("ControlRight" → "Right Ctrl"). Unknown strings come back unchanged.
pub fn key_label(key: &str) -> &str {
    match key {
        "ControlRight" => "Right Ctrl",
        "ControlLeft" => "Left Ctrl",
        "AltRight" => "Right Alt",
        "AltLeft" => "Left Alt",
        "ShiftRight" => "Right Shift",
        "ShiftLeft" => "Left Shift",
        "MetaRight" => "Right Super/Windows",
        "MetaLeft" => "Left Super/Windows",
        "CapsLock" => "Caps Lock",
        other => other,
    }
}

/// What the Hub tells the reader to hold on `os` (`std::env::consts::OS`):
/// "Fn" on macOS, whose hook reads a fixed key; the configured key everywhere
/// else ("Right Ctrl" by default). An empty configured key means the default.
pub fn push_to_talk_label(os: &str, configured_key: &str) -> String {
    if os == "macos" {
        return "Fn".to_string();
    }
    let key = if configured_key.trim().is_empty() { "ControlRight" } else { configured_key };
    key_label(key).to_string()
}

/// The Windows virtual-key code for a `push_to_talk_key` variant string, or
/// `None` when the string names no key the hook can watch. The caller falls
/// back to Right Ctrl (`VK_RCONTROL`, 0xA3) in that case.
pub fn windows_virtual_key(key: &str) -> Option<u16> {
    Some(match key {
        "ControlRight" => 0xA3,
        "ControlLeft" => 0xA2,
        "AltRight" => 0xA5,
        "AltLeft" => 0xA4,
        "ShiftRight" => 0xA1,
        "ShiftLeft" => 0xA0,
        "MetaRight" => 0x5C,
        "MetaLeft" => 0x5B,
        "CapsLock" => 0x14,
        "F1" => 0x70,
        "F2" => 0x71,
        "F3" => 0x72,
        "F4" => 0x73,
        "F5" => 0x74,
        "F6" => 0x75,
        "F7" => 0x76,
        "F8" => 0x77,
        "F9" => 0x78,
        "F10" => 0x79,
        "F11" => 0x7A,
        "F12" => 0x7B,
        _ => return None,
    })
}

/// Which failure describes a capture that could not start, from the error
/// text `whimpr_audio::start` produced. No input device at all is its own
/// message; anything else (the stream refused to build or play) is the OS
/// blocking the microphone.
pub fn capture_start_failure(error: &str) -> InjectionFailure {
    if error.contains("no default input device") {
        InjectionFailure::NoInputDevice
    } else {
        InjectionFailure::AccessibilityNotGranted
    }
}

/// What to report when the push-to-talk key was released and there is no
/// audio to transcribe. `start_error` is the text the capture failed to start
/// with, if it failed; `held_ms` is how long the key was down.
///
/// `None` means stay quiet: the hold was too short to be a real attempt. Bug
/// fix: on Windows every one of these paths used to end in an `eprintln!`
/// that release builds hide, so a blocked microphone looked exactly like a
/// successful dictation that typed nothing.
pub fn missing_capture_failure(start_error: Option<&str>, held_ms: u64) -> Option<InjectionFailure> {
    if held_ms < MIN_REAL_ATTEMPT_MS {
        return None;
    }
    Some(match start_error {
        Some(error) => capture_start_failure(error),
        None => InjectionFailure::NoAudioCaptured,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Platform;

    /// Every value the Hub's "Push-to-Talk Key" card offers (SettingsPane.tsx
    /// PTT_KEYS) and the Linux parser accepts (linux.rs parse_ptt_key).
    const PICKER_KEYS: [&str; 21] = [
        "ControlRight", "ControlLeft", "AltRight", "AltLeft", "ShiftRight", "ShiftLeft",
        "MetaRight", "MetaLeft", "CapsLock", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8",
        "F9", "F10", "F11", "F12",
    ];

    #[test]
    fn windows_names_right_ctrl_by_default_and_never_fn() {
        assert_eq!(push_to_talk_label("windows", "ControlRight"), "Right Ctrl");
        assert_eq!(push_to_talk_label("windows", ""), "Right Ctrl");
        for key in PICKER_KEYS {
            assert_ne!(push_to_talk_label("windows", key), "Fn", "{key}");
            assert_ne!(push_to_talk_label("linux", key), "Fn", "{key}");
        }
    }

    #[test]
    fn macos_always_names_fn() {
        for key in PICKER_KEYS {
            assert_eq!(push_to_talk_label("macos", key), "Fn", "{key}");
        }
        assert_eq!(push_to_talk_label("macos", ""), "Fn");
    }

    #[test]
    fn linux_names_the_configured_key() {
        assert_eq!(push_to_talk_label("linux", "AltLeft"), "Left Alt");
        assert_eq!(push_to_talk_label("linux", "F9"), "F9");
    }

    #[test]
    fn every_picker_key_has_a_windows_virtual_key() {
        for key in PICKER_KEYS {
            assert!(windows_virtual_key(key).is_some(), "{key} has no VK code");
        }
        assert_eq!(windows_virtual_key("ControlRight"), Some(0xA3)); // VK_RCONTROL
        assert_eq!(windows_virtual_key("ControlLeft"), Some(0xA2)); // VK_LCONTROL
        assert_eq!(windows_virtual_key("CapsLock"), Some(0x14)); // VK_CAPITAL
        assert_eq!(windows_virtual_key("F12"), Some(0x7B)); // VK_F12
        assert_eq!(windows_virtual_key("Fn"), None);
        assert_eq!(windows_virtual_key(""), None);
    }

    #[test]
    fn windows_virtual_keys_are_distinct() {
        let codes: Vec<u16> = PICKER_KEYS.iter().filter_map(|k| windows_virtual_key(k)).collect();
        let mut dedup = codes.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(codes.len(), dedup.len());
    }

    #[test]
    fn a_short_tap_with_no_audio_stays_quiet() {
        assert_eq!(missing_capture_failure(None, 0), None);
        assert_eq!(missing_capture_failure(None, MIN_REAL_ATTEMPT_MS - 1), None);
        assert_eq!(missing_capture_failure(Some("no default input device"), 200), None);
    }

    #[test]
    fn a_real_hold_with_no_microphone_says_no_microphone_found() {
        // The GitHub Actions runner case (run 41374831510): cpal finds no
        // input device, so the capture never starts.
        let failure = missing_capture_failure(Some("no default input device"), 1500).unwrap();
        assert_eq!(failure, InjectionFailure::NoInputDevice);
        assert_eq!(failure.diagnose(Platform::Windows).headline, "No microphone found");
    }

    #[test]
    fn a_real_hold_with_a_blocked_microphone_says_access_blocked() {
        // Windows' "Let desktop apps access your microphone" off: the device
        // exists but the stream refuses to build or play.
        let failure =
            missing_capture_failure(Some("failed to build input stream: access denied"), 1500)
                .unwrap();
        assert_eq!(failure, InjectionFailure::AccessibilityNotGranted);
        assert_eq!(failure.diagnose(Platform::Windows).headline, "Microphone access blocked");
    }

    #[test]
    fn a_real_hold_whose_capture_never_landed_says_no_audio() {
        assert_eq!(missing_capture_failure(None, 1500), Some(InjectionFailure::NoAudioCaptured));
    }
}
