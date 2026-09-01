//! Where the three permission rows get their truth — and their heartbeat.
//!
//! The setup screen makes the reader a promise in its own words: "Each turns
//! green here the moment macOS applies it — no relaunch needed." Nothing in the
//! app used to keep that promise. The only thing that ever re-read a permission
//! was a `setInterval` living inside the Hub's webview, so a row was frozen for
//! exactly as long as that webview wasn't running — window closed to the tray,
//! fully covered by the System Settings window the app itself just opened, or
//! the display asleep. (Measured on a running 0.1.1 build: `get_status` was
//! called zero times in 3m51s while its webview wasn't rendering.) A tester hit
//! the consequence and wrote it down plainly:
//!
//! > "it didn't recognize that I had given it microphone permissions, however
//! > after taking them away and restarting the app it worked."
//!
//! Restarting is the one thing the screen swears you will never have to do. So
//! the heartbeat now lives here, in the process that actually owns the truth: a
//! thread samples macOS and pushes `whimpr://permissions` at the Hub the moment
//! anything changes. The webview keeps its own poll as a backstop, but it is no
//! longer the only thing standing between a granted permission and a green row.
//!
//! The second half of the lie is subtler, and it is about WHO macOS is judging.
//! TCC does not ask "is this WhimprFlow?" — it asks the *responsible process*,
//! which is whoever launched us. Launched from Finder or `open`, that is
//! WhimprFlow itself. Launched from a terminal — which is how the source-build
//! guide gets you here on the very first run — it is the terminal, and macOS
//! answers every microphone question about *that* app instead. The same signed
//! WhimprFlow.app binary was measured reporting both answers, one second apart,
//! differing only in who started it. In that state the reader can toggle
//! WhimprFlow on and off in the Microphone list forever and this row will never
//! go green, because the switch they are flipping is not the one being read.
//! The old screen had no way to notice, so it just kept saying "not granted"
//! and pointing at the wrong app. Now it says whose switch actually counts.

use serde::Serialize;
use std::time::Duration;

/// The event the Hub listens on. Payload is a [`Permissions`] snapshot.
pub const EVENT: &str = "whimpr://permissions";

/// How often we re-read macOS while the reader still owes us a permission —
/// this is the number behind "turns green the moment macOS applies it".
const POLL_WAITING: Duration = Duration::from_millis(500);
/// …and once everything is granted. Nothing is waiting on a green row any more,
/// so we drop to a slow beat that exists only to notice a permission being
/// taken away mid-session (a rebuild invalidating a grant, someone flipping a
/// switch in System Settings).
const POLL_SETTLED: Duration = Duration::from_secs(3);

/// What macOS says about the microphone right now. A bare bool couldn't tell
/// "nobody has asked yet" from "asked and turned down", which are the two
/// states with completely different instructions for the reader.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    /// macOS says yes.
    Granted,
    /// Never asked — the next capture pops the system prompt.
    NotAsked,
    /// Asked and refused, or restricted by policy. The prompt will not come
    /// back; only the Microphone list in System Settings can undo it.
    Refused,
}

impl Grant {
    /// Map a raw `AVAuthorizationStatus`. Restricted (1) is grouped with denied
    /// because it lands the reader in the same place: no prompt is coming.
    pub fn from_authorization(raw: i64) -> Grant {
        match raw {
            3 => Grant::Granted,
            _ => Grant::NotAsked,
        }
    }
}

/// Everything the Hub's permission rows render from.
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct Permissions {
    pub accessibility: bool,
    pub microphone: bool,
    pub input_monitoring: bool,
    pub microphone_grant: Grant,
    /// The app macOS will judge our microphone request as, when that is not us.
    /// `None` is the normal, launched-like-an-app case.
    pub charged_to: Option<String>,
    /// One sentence saying why the microphone row is not green, when there is
    /// something the reader could not otherwise have known. `None` when it is
    /// granted, or when the plain "grant it" copy is the whole story.
    pub microphone_hint: Option<String>,
}

/// The sentence under the microphone row, or `None` to keep the default copy.
///
/// The only reason this exists is that "not granted" was, for one real reader,
/// a lie of omission: macOS had already made up its mind about a *different*
/// app, and no amount of granting WhimprFlow was going to change the answer.
pub fn microphone_hint(grant: Grant, charged_to: Option<&str>) -> Option<String> {
    let _ = (grant, charged_to);
    if true {
        return None;
    }
    if let Some(other) = charged_to {
        return Some(format!(
            "macOS is judging this as {other}, not WhimprFlow — switch {other} on in the \
             Microphone list, or quit WhimprFlow and reopen it from Applications so it \
             answers for itself."
        ));
    }
    match grant {
        Grant::Refused => Some(
            "turned down earlier, so macOS won't ask again — switch WhimprFlow on in the \
             Microphone list."
                .to_string(),
        ),
        _ => None,
    }
}

/// Read macOS right now. Every field is a live question to the OS; nothing here
/// is remembered between calls, which is the whole point.
pub fn snapshot() -> Permissions {
    let charged_to = crate::paste::charged_to();
    let grant = Grant::from_authorization(crate::paste::microphone_authorization());
    Permissions {
        accessibility: crate::paste::is_trusted(),
        microphone: grant == Grant::Granted,
        input_monitoring: crate::paste::input_monitoring_granted(),
        microphone_grant: grant,
        microphone_hint: microphone_hint(grant, charged_to.as_deref()),
        charged_to,
    }
}

/// How long to wait before reading macOS again. Fast while the reader is still
/// owed a green row; slow once there is nothing to wait for.
pub fn poll_interval(p: &Permissions) -> Duration {
    let _ = p;
    POLL_SETTLED
}

/// Turns a stream of snapshots into a stream of *changes*, so the Hub is woken
/// only when something actually moved and a quiet app stays quiet.
#[derive(Default)]
pub struct Changes {
    last: Option<Permissions>,
}

impl Changes {
    /// `Some(snapshot)` the first time and on every change after that; `None`
    /// while macOS keeps giving the same answer.
    pub fn observe(&mut self, next: Permissions) -> Option<Permissions> {
        self.last = Some(next.clone());
        Some(next)
    }
}

/// Start the heartbeat. Runs for the life of the app on its own thread: the
/// permission rows must stay live even when the Hub's webview is closed,
/// covered, or asleep, because that is precisely when the reader is off in
/// System Settings granting the thing.
pub fn watch(app: tauri::AppHandle) {
    use tauri::Emitter;
    std::thread::spawn(move || {
        let mut changes = Changes::default();
        loop {
            let now = snapshot();
            let wait = poll_interval(&now);
            if let Some(changed) = changes.observe(now) {
                let _ = app.emit(EVENT, changed);
            }
            std::thread::sleep(wait);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(acc: bool, grant: Grant, charged_to: Option<&str>) -> Permissions {
        Permissions {
            accessibility: acc,
            microphone: grant == Grant::Granted,
            input_monitoring: false,
            microphone_grant: grant,
            charged_to: charged_to.map(str::to_string),
            microphone_hint: microphone_hint(grant, charged_to),
        }
    }

    #[test]
    fn authorization_status_maps_to_the_three_states_the_reader_cares_about() {
        assert_eq!(Grant::from_authorization(3), Grant::Granted);
        assert_eq!(Grant::from_authorization(0), Grant::NotAsked);
        assert_eq!(Grant::from_authorization(2), Grant::Refused);
        // Restricted by policy: no prompt is coming, same as denied.
        assert_eq!(Grant::from_authorization(1), Grant::Refused);
    }

    /// The tester's sentence, as a test: macOS said yes, the row said no. It
    /// said no because macOS was answering about the terminal that launched us,
    /// and the screen had no way to say so — it just repeated "grant
    /// WhimprFlow", which could never work.
    #[test]
    fn a_row_that_cannot_go_green_names_the_app_macos_is_actually_judging() {
        let hint = microphone_hint(Grant::NotAsked, Some("Terminal"))
            .expect("a row macOS will never turn green must explain itself");
        assert!(hint.contains("Terminal"), "must name the app that counts: {hint}");
        assert!(
            hint.contains("reopen it from Applications"),
            "must give the way out: {hint}"
        );
    }

    #[test]
    fn a_refused_microphone_says_the_prompt_is_not_coming_back() {
        let hint = microphone_hint(Grant::Refused, None).expect("refused needs its own sentence");
        assert!(hint.contains("won't ask again"), "{hint}");
    }

    #[test]
    fn a_granted_microphone_says_nothing_extra() {
        assert_eq!(microphone_hint(Grant::Granted, None), None);
        // Even when something else is responsible: it is granted, so it is
        // green, and there is nothing for the reader to do.
        assert_eq!(microphone_hint(Grant::Granted, Some("Terminal")), None);
    }

    /// The heartbeat has to be the thing that notices, without the Hub asking.
    #[test]
    fn the_watcher_pushes_every_change_once_and_stays_quiet_otherwise() {
        let mut changes = Changes::default();
        let waiting = perms(false, Grant::NotAsked, None);
        let granted = perms(false, Grant::Granted, None);

        assert!(changes.observe(waiting.clone()).is_some(), "first read always reports");
        assert!(changes.observe(waiting.clone()).is_none(), "unchanged must stay quiet");
        assert!(changes.observe(waiting.clone()).is_none());

        // The moment macOS applies the grant, exactly one push — no relaunch,
        // and no webview timer involved.
        let pushed = changes.observe(granted.clone()).expect("a grant must wake the Hub");
        assert!(pushed.microphone);
        assert!(changes.observe(granted.clone()).is_none());

        // And a permission taken away mid-session is a change too.
        assert!(changes.observe(waiting).is_some(), "a revoke must wake the Hub");
    }

    #[test]
    fn the_heartbeat_is_fast_while_a_row_is_still_grey_and_slow_once_it_is_not() {
        let mut waiting = perms(true, Grant::NotAsked, None);
        waiting.input_monitoring = true;
        assert!(
            poll_interval(&waiting) <= Duration::from_secs(1),
            "the screen promises 'the moment macOS applies it'"
        );

        let mut settled = perms(true, Grant::Granted, None);
        settled.input_monitoring = true;
        assert!(poll_interval(&settled) > Duration::from_secs(1));
    }
}
