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
    }

    const KCG_HID_EVENT_TAP: u32 = 0;
    const KCG_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
    const KEYCODE_V: u16 = 9;

    /// Whether the app has Accessibility permission. This one grant governs BOTH the
    /// global Fn CGEventTap (untrusted taps are silently limited to frontmost-only)
    /// and posting the Cmd+V paste into other apps.
    pub fn is_trusted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    /// Check Accessibility trust and, if missing, show the native prompt that offers
    /// to open System Settings → Privacy & Security → Accessibility.
    pub fn prompt_accessibility() -> bool {
        macos_accessibility_client::accessibility::application_is_trusted_with_prompt()
    }

    /// The raw `AVAuthorizationStatus` for the microphone (3 authorized, 2 denied,
    /// 1 restricted, 0 not yet asked; -1 if AVFoundation didn't hand us the audio
    /// media type at all).
    ///
    /// This asks macOS every single time — measured live, it tracks a TCC change
    /// inside a running process within a second, no relaunch — so anything stale
    /// in the Hub is stale on the way to the screen, not here. What it *cannot*
    /// tell you is that macOS may be answering about a different app entirely;
    /// that's what [`charged_to`] is for.
    pub fn microphone_authorization() -> i64 {
        use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};
        unsafe {
            let Some(audio) = AVMediaTypeAudio else {
                return -1;
            };
            AVCaptureDevice::authorizationStatusForMediaType(audio).0 as i64
        }
    }

    /// The app macOS holds responsible for what we do — `None` when that's us.
    ///
    /// TCC never asks "is this WhimprFlow?". It asks the *responsible process*,
    /// which is whoever launched us: ourselves when opened from Finder, the
    /// terminal when started from a shell (which is how a source build gets its
    /// first run). When it isn't us, every microphone answer macOS gives is about
    /// that other app, and the reader can flip WhimprFlow's own switch all day
    /// without moving it. Better to say whose switch counts than to keep
    /// repeating "not granted".
    ///
    /// `responsibility_get_pid_responsible_for_pid` is undocumented, so it's
    /// looked up at runtime and simply declines to answer if it ever disappears —
    /// a missing hint is survivable, a missing symbol at launch is not.
    pub fn charged_to() -> Option<String> {
        use objc2_app_kit::NSRunningApplication;

        type ResponsibleFor = unsafe extern "C" fn(i32) -> i32;
        const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
        extern "C" {
            fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
            fn getpid() -> i32;
        }

        unsafe {
            let name = c"responsibility_get_pid_responsible_for_pid";
            let sym = dlsym(RTLD_DEFAULT, name.as_ptr());
            if sym.is_null() {
                return None;
            }
            let responsible_for: ResponsibleFor = std::mem::transmute(sym);
            let me = getpid();
            let responsible = responsible_for(me);
            if responsible <= 0 || responsible == me {
                return None;
            }
            let app = NSRunningApplication::runningApplicationWithProcessIdentifier(responsible)?;
            let name = app.localizedName()?.to_string();
            (!name.is_empty()).then_some(name)
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
    charged_to, input_monitoring_granted, is_trusted, microphone_authorization, paste_text,
    prompt_accessibility, request_input_monitoring,
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

/// Windows has no TCC status to read; treat it as authorized (3) so the Hub's
/// microphone row matches [`microphone_granted`].
#[cfg(not(target_os = "macos"))]
pub fn microphone_authorization() -> i64 {
    3
}

/// Responsible-process attribution is a macOS-only idea — nothing to warn about.
#[cfg(not(target_os = "macos"))]
pub fn charged_to() -> Option<String> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn input_monitoring_granted() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn request_input_monitoring() -> bool {
    true
}
