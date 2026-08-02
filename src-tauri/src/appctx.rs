//! Frontmost-app detection: which app is the paste target right now, so the
//! cleanup layer can format the output for the medium (email vs. text vs. chat).
//!
//! On macOS this reads `NSWorkspace.frontmostApplication` — the identity of the
//! focused app is public information and needs **no** extra TCC permission
//! (unlike reading the window's text, which would need Accessibility). The pill
//! overlay is non-activating, so the app the user is dictating into stays
//! frontmost; we capture its bundle id at record-start (Fn down).

/// Bundle id of the frontmost application — the paste target — e.g.
/// `com.apple.mail`. Returns `None` when it can't be determined or when
/// WhimprFlow itself is frontmost (so we don't format for our own Hub window).
#[cfg(target_os = "macos")]
#[allow(unused_unsafe)]
pub fn frontmost_bundle_id() -> Option<String> {
    use objc2_app_kit::NSWorkspace;
    // NSWorkspace reads are thread-safe; safe to call from the tap thread.
    let bid = unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let app = ws.frontmostApplication()?;
        app.bundleIdentifier()?
    };
    let bid = bid.to_string();
    if bid == "com.whimpr.whimprflow" {
        None
    } else {
        Some(bid)
    }
}

#[cfg(target_os = "linux")]
pub fn frontmost_bundle_id() -> Option<String> {
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
    None
}

#[cfg(target_os = "windows")]
pub fn frontmost_bundle_id() -> Option<String> {
    None
}
