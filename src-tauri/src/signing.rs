//! Whether this build's code signature is stable enough for macOS to keep a
//! permission grant attached to it.
//!
//! When the user grants Accessibility, macOS does not remember "WhimprFlow" —
//! it records the *designated requirement* of the process that asked. A
//! Developer ID signature produces a requirement keyed to the team identifier:
//!
//! ```text
//! identifier "com.whimpr.whimprflow" and anchor apple generic and ... leaf[subject.OU] = R5R3ZS54LV
//! ```
//!
//! which is byte-identical across every rebuild, so the grant survives one. An
//! ad-hoc signature — what `cargo build` leaves behind when no identity is
//! configured — produces a bare hash of the binary instead:
//!
//! ```text
//! cdhash H"53b870a9dcfc5a05b5a7e48367bca975aee26130"
//! ```
//!
//! Every rebuild changes one byte and therefore changes that requirement, so
//! the newly built app is a *different client* to TCC. The failure this causes
//! is silent and extremely confusing: System Settings still lists WhimprFlow
//! with its switch on (that list is drawn from the bundle path and name), while
//! the running app's own check keeps returning false, because the requirement
//! recorded next to that switch no longer matches the process asking.
//!
//! No amount of re-toggling fixes it, and it looks for all the world like the
//! app is ignoring the permission. So rather than tell the user they have not
//! granted something they plainly have, the Hub asks [`stable_identity`] and
//! explains the real situation.

#[cfg(target_os = "macos")]
mod imp {
    use std::os::raw::c_void;
    use std::ptr::null;

    type CFTypeRef = *const c_void;

    // Include the signing information (team identifier, certificates) in the
    // dictionary returned by SecCodeCopySigningInformation. Apple's
    // kSecCSSigningInformation.
    const K_SEC_CS_SIGNING_INFORMATION: u32 = 1 << 1;
    const ERR_SEC_SUCCESS: i32 = 0;

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        /// A code object for the running process.
        fn SecCodeCopySelf(flags: u32, code: *mut CFTypeRef) -> i32;
        fn SecCodeCopySigningInformation(
            code: CFTypeRef,
            flags: u32,
            information: *mut CFTypeRef,
        ) -> i32;
        /// CFStringRef key: the Team ID from the signing certificate. Absent
        /// for ad-hoc and unsigned code, present for anything signed with a
        /// real Apple-issued identity.
        static kSecCodeInfoTeamIdentifier: CFTypeRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
    }

    /// True when this build is signed with an identity that gives it the same
    /// designated requirement after a rebuild — i.e. when a permission the user
    /// grants will still be attached to the app tomorrow.
    ///
    /// Any Apple-issued identity qualifies; only ad-hoc and unsigned builds do
    /// not. Errors are reported as "stable" so a failure to introspect never
    /// puts a scary banner in front of a user whose install is fine.
    pub fn stable_identity() -> bool {
        unsafe {
            let mut code: CFTypeRef = null();
            if SecCodeCopySelf(0, &mut code) != ERR_SEC_SUCCESS || code.is_null() {
                return true;
            }
            let mut info: CFTypeRef = null();
            let status = SecCodeCopySigningInformation(code, K_SEC_CS_SIGNING_INFORMATION, &mut info);
            CFRelease(code);
            if status != ERR_SEC_SUCCESS || info.is_null() {
                return true;
            }
            let team = CFDictionaryGetValue(info, kSecCodeInfoTeamIdentifier);
            CFRelease(info);
            !team.is_null()
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    /// Only macOS ties permissions to a code signature this way.
    pub fn stable_identity() -> bool {
        true
    }
}

pub use imp::stable_identity;
