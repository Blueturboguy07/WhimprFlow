//! User-facing diagnostics for injection/permission failures.
//!
//! Before this module existed, every way the dictation loop could fail to
//! deliver text — a missing macOS Accessibility grant, a stale TCC entry left
//! by an earlier build, a failed Windows keyboard hook, silence from the mic,
//! an empty transcript — only ever reached an `eprintln!`. That is invisible
//! to anyone who launched the packaged app by double-clicking it rather than
//! from a terminal, which is the default way a built app is used. Two
//! separate bug reports ("text is not writing where the cursor is") trace
//! back to exactly this: the failure was real and even correctly diagnosed in
//! the code, it just never reached the user.
//!
//! This module turns each distinct failure into a short, specific,
//! actionable [`Diagnostic`] the UI can show (see `whimpr://error` /
//! `whimpr://flowbar/state` in `src-tauri/src/diag.rs`), instead of a mystery
//! "nothing happened".

/// A distinct, identifiable reason the dictation loop failed to deliver text,
/// ordered roughly by how early in the pipeline it can occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InjectionFailure {
    /// macOS: `AXIsProcessTrusted()` is false — Accessibility was never
    /// granted (or was revoked). Windows has no equivalent gate for typing,
    /// but reuses this variant for "mic access is blocked at the OS level".
    AccessibilityNotGranted,
    /// macOS: Accessibility reads true, but the CGEventTap (or, on Windows,
    /// the `WH_KEYBOARD_LL` hook) failed to install — most commonly a stale
    /// permission entry left behind by an earlier build with a different
    /// code signature, so the hotkey silently never fires.
    HotkeyTapFailed,
    /// The OS clipboard could not be read or written.
    ClipboardUnavailable,
    /// Audio capture produced (near-)silence — the mic is muted, the wrong
    /// input device is selected, or OS mic permission is blocking the stream.
    NoAudioCaptured,
    /// Real audio came in, but the ASR engine returned an empty transcript.
    EmptyTranscript,
    /// The speech model failed to load, or no model file is present on disk.
    AsrUnavailable,
}

/// The message to show the user: a short headline (fits in the pill) and a
/// longer detail line with the exact fix (fits in a Hub banner / tooltip).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub headline: String,
    pub detail: String,
}

/// Which platform's exact wording to use. Kept as a plain enum (rather than
/// `cfg(target_os)` inside this module) specifically so every branch is
/// exercised by a unit test on any host, including CI running on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
}

impl InjectionFailure {
    /// Build the concrete, platform-worded message for this failure.
    pub fn diagnose(self, platform: Platform) -> Diagnostic {
        use InjectionFailure::*;
        use Platform::*;
        match (self, platform) {
            (AccessibilityNotGranted, MacOs) => Diagnostic {
                headline: "Accessibility permission needed".into(),
                detail: "WhimprFlow can't type into other apps yet. Open System Settings → \
                    Privacy & Security → Accessibility, turn on WhimprFlow, then try again — \
                    no relaunch needed."
                    .into(),
            },
            (AccessibilityNotGranted, Windows) => Diagnostic {
                headline: "Microphone access blocked".into(),
                detail: "Windows is blocking microphone access. Open Settings → Privacy & \
                    security → Microphone and turn on \"Let desktop apps access your \
                    microphone\", then try again."
                    .into(),
            },
            (HotkeyTapFailed, MacOs) => Diagnostic {
                headline: "Fn key isn't wired up".into(),
                detail: "Accessibility shows granted, but the key listener failed to start — \
                    usually a stale permission entry left by an earlier build. Open System \
                    Settings → Privacy & Security → Accessibility, turn WhimprFlow off and back \
                    on (or remove it with \"−\" and re-add it), then relaunch WhimprFlow."
                    .into(),
            },
            (HotkeyTapFailed, Windows) => Diagnostic {
                headline: "Right Ctrl isn't wired up".into(),
                detail: "The keyboard hook failed to install. Relaunch WhimprFlow; if it keeps \
                    happening, another app may be holding a global keyboard hook (some \
                    anti-cheat or security tools do) — close it and relaunch."
                    .into(),
            },
            (ClipboardUnavailable, _) => Diagnostic {
                headline: "Couldn't paste".into(),
                detail: "The system clipboard couldn't be read or written, so the transcript \
                    wasn't inserted. Try again — if it persists, another app may be holding the \
                    clipboard."
                    .into(),
            },
            (NoAudioCaptured, MacOs) => Diagnostic {
                headline: "No audio came in".into(),
                detail: "The microphone appears silent. Check System Settings → Privacy & \
                    Security → Microphone is on for WhimprFlow, and that the right input device \
                    is selected."
                    .into(),
            },
            (NoAudioCaptured, Windows) => Diagnostic {
                headline: "No audio came in".into(),
                detail: "The microphone appears silent. Check Settings → Privacy & security → \
                    Microphone allows desktop apps, and that the right input device is selected."
                    .into(),
            },
            (EmptyTranscript, _) => Diagnostic {
                headline: "Didn't catch that".into(),
                detail: "WhimprFlow heard audio but transcribed no words — try speaking a bit \
                    louder or closer to the mic."
                    .into(),
            },
            (AsrUnavailable, _) => Diagnostic {
                headline: "Speech model not found".into(),
                detail: "No Whisper model is installed. Download a ggml-base.en.bin model (see \
                    MODELS.md in the repo, or huggingface.co/ggerganov/whisper.cpp) into the \
                    models folder, then relaunch."
                    .into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_FAILURES: [InjectionFailure; 6] = [
        InjectionFailure::AccessibilityNotGranted,
        InjectionFailure::HotkeyTapFailed,
        InjectionFailure::ClipboardUnavailable,
        InjectionFailure::NoAudioCaptured,
        InjectionFailure::EmptyTranscript,
        InjectionFailure::AsrUnavailable,
    ];
    const ALL_PLATFORMS: [Platform; 2] = [Platform::MacOs, Platform::Windows];
    /// A pill headline should stay short enough not to force the flow bar
    /// wider than its normal error-state width.
    const MAX_HEADLINE_LEN: usize = 40;

    #[test]
    fn every_failure_has_nonempty_copy_on_every_platform() {
        for &failure in &ALL_FAILURES {
            for &platform in &ALL_PLATFORMS {
                let d = failure.diagnose(platform);
                assert!(!d.headline.trim().is_empty(), "{failure:?}/{platform:?} headline empty");
                assert!(!d.detail.trim().is_empty(), "{failure:?}/{platform:?} detail empty");
            }
        }
    }

    #[test]
    fn headlines_fit_the_pill() {
        for &failure in &ALL_FAILURES {
            for &platform in &ALL_PLATFORMS {
                let d = failure.diagnose(platform);
                assert!(
                    d.headline.chars().count() <= MAX_HEADLINE_LEN,
                    "{failure:?}/{platform:?} headline too long for the pill: {:?} ({} chars)",
                    d.headline,
                    d.headline.chars().count()
                );
            }
        }
    }

    #[test]
    fn accessibility_diagnostic_names_the_exact_macos_setting() {
        let d = InjectionFailure::AccessibilityNotGranted.diagnose(Platform::MacOs);
        assert!(d.detail.contains("System Settings"));
        assert!(d.detail.contains("Privacy & Security"));
        assert!(d.detail.contains("Accessibility"));
    }

    #[test]
    fn hotkey_tap_failed_explains_the_stale_tcc_gotcha_on_macos() {
        let d = InjectionFailure::HotkeyTapFailed.diagnose(Platform::MacOs);
        // This is the exact real-world failure mode: AXIsProcessTrusted() is
        // true but the tap still won't create because of a stale grant from
        // an earlier build's code signature. The fix must say what to do
        // about it, not just repeat "grant Accessibility" (which is already
        // granted in this case).
        assert!(d.detail.contains("stale"));
        assert!(d.detail.contains("off and back on") || d.detail.contains("remove"));
    }

    #[test]
    fn asr_unavailable_points_at_models_md() {
        let d = InjectionFailure::AsrUnavailable.diagnose(Platform::MacOs);
        assert!(d.detail.contains("MODELS.md"));
    }

    #[test]
    fn windows_wording_never_mentions_macos_settings() {
        for &failure in &ALL_FAILURES {
            let d = failure.diagnose(Platform::Windows);
            assert!(!d.detail.contains("System Settings"));
            assert!(!d.headline.contains("System Settings"));
        }
    }

    #[test]
    fn macos_wording_never_mentions_windows_settings_app() {
        for &failure in &ALL_FAILURES {
            let d = failure.diagnose(Platform::MacOs);
            assert!(!d.detail.contains("Settings → Privacy & security"));
        }
    }

    #[test]
    fn diagnose_is_deterministic() {
        // Same input always produces the same message — callers (and tests)
        // can rely on it rather than re-deriving copy from state each time.
        for &failure in &ALL_FAILURES {
            for &platform in &ALL_PLATFORMS {
                assert_eq!(failure.diagnose(platform), failure.diagnose(platform));
            }
        }
    }
}
