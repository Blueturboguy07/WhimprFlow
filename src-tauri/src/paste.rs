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
        // AXIsProcessTrusted can keep returning the value observed before the
        // reader changed the switch in System Settings. Ask TCC directly so the
        // permission watcher sees that change without requiring a relaunch.
        tcc_preflight("kTCCServiceAccessibility")
            .map(|answer| answer == 0)
            .unwrap_or_else(|| unsafe { AXIsProcessTrusted() })
    }

    /// Check Accessibility trust and, if missing, show the native prompt that offers
    /// to open System Settings → Privacy & Security → Accessibility.
    pub fn prompt_accessibility() -> bool {
        macos_accessibility_client::accessibility::application_is_trusted_with_prompt()
    }

    /// Ask TCC, right now, whether this process may use `service`.
    ///
    /// `TCCAccessPreflight` is the call underneath [`is_trusted`] and
    /// [`input_monitoring_granted`] — the two rows on the setup screen that were
    /// never wrong. It answers 0 = allowed, 1 = denied, 2 = nobody has asked yet.
    ///
    /// It is not a published symbol, so it is looked up at runtime and simply
    /// declines to answer if it ever disappears; a missing hint is survivable, a
    /// missing symbol at launch is not. `None` means "couldn't ask" — never
    /// "no".
    pub fn tcc_preflight(service: &str) -> Option<i32> {
        use objc2_foundation::NSString;

        type Preflight = unsafe extern "C" fn(*const c_void, *const c_void) -> i32;
        const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
        const TCC_FRAMEWORK: &[u8] =
            b"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC\0";
        extern "C" {
            fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
            fn dlopen(path: *const i8, mode: i32) -> *mut c_void;
        }

        unsafe {
            let name = c"TCCAccessPreflight";
            let mut sym = dlsym(RTLD_DEFAULT, name.as_ptr());
            if sym.is_null() {
                // Nothing in the process has pulled TCC.framework in yet.
                const RTLD_LAZY: i32 = 0x1;
                let handle = dlopen(TCC_FRAMEWORK.as_ptr() as *const i8, RTLD_LAZY);
                if handle.is_null() {
                    return None;
                }
                sym = dlsym(handle, name.as_ptr());
                if sym.is_null() {
                    return None;
                }
            }
            let preflight: Preflight = std::mem::transmute(sym);
            // NSString is toll-free bridged to CFStringRef.
            let service = NSString::from_str(service);
            let ptr: *const NSString = &*service;
            Some(preflight(ptr as *const c_void, std::ptr::null()))
        }
    }

    /// The raw `AVAuthorizationStatus` shape for the microphone: 3 authorized,
    /// 2 denied, 1 restricted, 0 nobody has asked yet.
    ///
    /// This used to be a straight call to
    /// `AVCaptureDevice::authorizationStatusForMediaType`, and that is the bug a
    /// tester wrote down as:
    ///
    /// > "it didn't recognize that I had given it microphone permissions,
    /// > however after taking them away and restarting the app it worked."
    ///
    /// AVFoundation answers that question **once per process and then keeps
    /// answering it forever**. Measured: two copies of this same signed binary,
    /// running in the same second against the same TCC database — the one
    /// started before the permission changed said 3 for a minute and a half
    /// after the row was gone; the one started after said 0. Neither elapsed
    /// time nor app activation ever cleared it. Restarting the app is literally
    /// the only thing that does, which is exactly what the tester discovered and
    /// exactly what the setup screen promises he will never have to do.
    ///
    /// So ask TCC directly, the way the Accessibility and Input Monitoring rows
    /// always did. AVFoundation stays as the fallback for the day the private
    /// symbol goes away, and its answer is still honoured when it says yes: a
    /// cached "yes" was true at some point in this process's life, so trusting
    /// it can only leave a row greener than before, never greyer.
    pub fn microphone_authorization() -> i64 {
        use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};

        let cached = unsafe {
            match AVMediaTypeAudio {
                Some(audio) => AVCaptureDevice::authorizationStatusForMediaType(audio).0 as i64,
                None => -1,
            }
        };
        crate::permissions::resolve_microphone(cached, tcc_preflight("kTCCServiceMicrophone"))
    }

    /// The app macOS holds responsible for what we do — `None` when that's us.
    ///
    /// TCC never asks "is this WhimprFlow?". It asks the *responsible process*,
    /// which is whoever launched us: ourselves when opened from Finder or the
    /// Dock, the terminal when started from a shell — which is how the
    /// build-from-source instructions in the README get you here on the very
    /// first run. Measured: the same signed binary, launched both ways one
    /// second apart, reported microphone "not asked yet"/Accessibility granted
    /// under a terminal and microphone granted/Accessibility not granted under
    /// itself. Every row was answering about the terminal.
    ///
    /// In that state the reader can toggle WhimprFlow on and off in the
    /// Microphone list all day and this row will never move, because the switch
    /// they are flipping is not the one being read. Better to say whose switch
    /// counts than to keep repeating "not granted" at someone doing everything
    /// right.
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

/// Windows has no TCC status to read; report "authorized" (3) — the microphone
/// row there is not gated on a system privacy grant.
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

#[cfg(all(test, target_os = "macos"))]
mod macos_permission_tests {
    use super::imp::{input_monitoring_granted, is_trusted, tcc_preflight};

    /// Accessibility and Input Monitoring were the two rows on the setup screen
    /// that were never wrong, and the reason is that both are a TCC preflight
    /// taken fresh on every call — `AXIsProcessTrusted` and
    /// `CGPreflightListenEventAccess` are preflights with nicer names.
    ///
    /// The microphone row now asks TCC the same way. This checks that the
    /// preflight really is that same mechanism: it has to reproduce both of
    /// those rows exactly, on whatever machine this runs on and whatever its
    /// grants happen to be. If it can't, the microphone answer isn't coming from
    /// where we think it is and the fix is built on sand.
    #[test]
    fn the_preflight_we_now_use_reproduces_the_two_rows_that_were_never_wrong() {
        assert_eq!(
            tcc_preflight("kTCCServiceAccessibility").map(|v| v == 0),
            Some(is_trusted()),
            "TCC preflight must agree with AXIsProcessTrusted"
        );
        assert_eq!(
            tcc_preflight("kTCCServiceListenEvent").map(|v| v == 0),
            Some(input_monitoring_granted()),
            "TCC preflight must agree with CGPreflightListenEventAccess"
        );
    }

    /// 0 allowed / 1 denied / 2 nobody has asked — anything else means the
    /// symbol we found is not the function we think it is, and we must not map
    /// its return value onto a permission row.
    #[test]
    fn the_microphone_preflight_answers_in_the_range_we_map() {
        let answer = tcc_preflight("kTCCServiceMicrophone");
        assert!(
            matches!(answer, None | Some(0) | Some(1) | Some(2)),
            "unexpected TCCAccessPreflight result: {answer:?}"
        );
    }
}
