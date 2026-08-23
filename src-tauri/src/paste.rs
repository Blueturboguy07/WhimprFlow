//! Text insertion: deliver transcribed/cleaned text to the frontmost app.
//!
//! First rung of the insertion ladder — clipboard paste: save the current
//! clipboard, write our text, synthesize Cmd+V, then restore the clipboard. This
//! is the universal path that works in almost every app. (AX direct-insert and the
//! terminal/secure-input handling from the plan layer on later, in the sidecar.)
//!
//! Posting the Cmd+V keystroke requires **Accessibility** permission; [`is_trusted`]
//! reports whether it's granted so the shell can prompt.

#[cfg(target_os = "macos")]
mod imp {
    use std::os::raw::c_void;
    use std::ptr::null;
    use std::time::Duration;

    type CGEventRef = *mut c_void;
    type CGEventSourceRef = *const c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreateKeyboardEvent(
            source: CGEventSourceRef,
            keycode: u16,
            keydown: bool,
        ) -> CGEventRef;
        fn CGEventSetFlags(event: CGEventRef, flags: u64);
        fn CGEventPost(tap: u32, event: CGEventRef);
        /// Whether the app has Input Monitoring (listen-event) access — required for
        /// the Fn key tap to see keystrokes globally, not just while we're frontmost.
        fn CGPreflightListenEventAccess() -> bool;
        /// Request Input Monitoring access: registers the app in the list and prompts.
        fn CGRequestListenEventAccess() -> bool;
    }

    /// True when Input Monitoring is granted (the Fn tap works in every app).
    pub fn input_monitoring_granted() -> bool {
        unsafe { CGPreflightListenEventAccess() }
    }

    /// Prompt for Input Monitoring and register the app in the settings list.
    pub fn request_input_monitoring() -> bool {
        unsafe { CGRequestListenEventAccess() }
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        /// Re-queries TCC each call (no in-process cache) when the options dict is
        /// null.  Pass null to check without prompting; the bool return is the live
        /// grant status.
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    }

    const KCG_HID_EVENT_TAP: u32 = 0;
    const KCG_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
    const KEYCODE_V: u16 = 9;

    /// Whether the app has Accessibility permission. This one grant governs BOTH the
    /// global Fn CGEventTap (untrusted taps are silently limited to frontmost-only)
    /// and posting the Cmd+V paste into other apps.
    ///
    /// Uses `AXIsProcessTrustedWithOptions(NULL)` instead of `AXIsProcessTrusted()`
    /// because the latter caches its answer in-process and keeps returning `false`
    /// even after the user grants the permission in System Settings — until the app
    /// is relaunched.  Passing a NULL options dict skips the cache and re-queries TCC
    /// on every call, so the UI reflects the live grant without a restart.
    pub fn is_trusted() -> bool {
        unsafe { AXIsProcessTrustedWithOptions(null()) }
    }

    /// Check Accessibility trust and, if missing, show the native prompt that offers
    /// to open System Settings → Privacy & Security → Accessibility.
    pub fn prompt_accessibility() -> bool {
        macos_accessibility_client::accessibility::application_is_trusted_with_prompt()
    }

    /// Whether microphone access is authorized (so the Hub can show it accurately).
    pub fn microphone_granted() -> bool {
        use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
        unsafe {
            let Some(audio) = AVMediaTypeAudio else {
                return false;
            };
            let status = AVCaptureDevice::authorizationStatusForMediaType(audio);
            status == AVAuthorizationStatus::Authorized
        }
    }

    fn post_cmd_v() {
        unsafe {
            let down = CGEventCreateKeyboardEvent(null(), KEYCODE_V, true);
            CGEventSetFlags(down, KCG_FLAG_MASK_COMMAND);
            CGEventPost(KCG_HID_EVENT_TAP, down);
            CFRelease(down as *const c_void);

            let up = CGEventCreateKeyboardEvent(null(), KEYCODE_V, false);
            CGEventSetFlags(up, KCG_FLAG_MASK_COMMAND);
            CGEventPost(KCG_HID_EVENT_TAP, up);
            CFRelease(up as *const c_void);
        }
    }

    pub fn paste_text(text: &str) -> anyhow::Result<()> {
        use arboard::Clipboard;
        if !is_trusted() {
            return Err(anyhow::anyhow!(
                "no Accessibility permission — cannot paste (grant it in System Settings → \
                 Privacy & Security → Accessibility, then relaunch)"
            ));
        }
        let mut cb = Clipboard::new()?;
        let saved = cb.get_text().ok();
        cb.set_text(text.to_string())?;
        // Give the pasteboard a moment to settle before the paste keystroke.
        std::thread::sleep(Duration::from_millis(60));
        post_cmd_v();
        // Let the target consume the paste before we restore the old clipboard.
        std::thread::sleep(Duration::from_millis(150));
        if let Some(prev) = saved {
            let _ = cb.set_text(prev);
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub use imp::{
    input_monitoring_granted, is_trusted, microphone_granted, paste_text, prompt_accessibility,
    request_input_monitoring,
};

#[cfg(not(target_os = "macos"))]
pub fn paste_text(_text: &str) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn is_trusted() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn prompt_accessibility() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn microphone_granted() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn input_monitoring_granted() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn request_input_monitoring() -> bool {
    true
}
