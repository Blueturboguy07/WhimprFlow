//! The gate window: the publik screens (publik.rs `gate_screen`) drawn by Rust in one small
//! Tauri window, before any window of the app's own. The page is served by the custom URI scheme
//! `publikgate`, so it needs no bundled UI, no IPC and no change to the React app. A button is a
//! link to `/press?id=<button>`; the scheme handler hands the id to the thread that runs
//! `publik::ensure_signed_in`, which then asks for the next screen.
//!
//! `run()` is the one entry point: call it from `setup`, and build the app's own windows in the
//! closure it calls once the person is signed in.

use crate::publik::{self, GateUi, Screen};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime, UriSchemeContext, WebviewUrl, WebviewWindowBuilder};

/// The URI scheme that serves the gate page. Register it on the Builder with `protocol`.
pub const SCHEME: &str = "publikgate";
const LABEL: &str = "publik-gate";

/// What the gate shows right now, and where a button press goes.
struct Showing {
    html: String,
    buttons: Vec<&'static str>,
    tx: Sender<&'static str>,
}
static SHOWING: Mutex<Option<Showing>> = Mutex::new(None);

/// (key, wallet, choice) as `publik::ensure_signed_in` returns it.
pub type Gate = (String, Value, Option<String>);

/// Run the gate on its own thread. `after` runs on the main thread once the person is signed in,
/// before the gate window closes: build the app's own windows in it, so the app does not exit
/// with its last window.
pub fn run<F>(app: AppHandle, after: F)
where
    F: FnOnce(&AppHandle, Gate) + Send + 'static,
{
    let spawned = std::thread::Builder::new().name("publik-gate".into()).spawn(move || {
        let gate = publik::ensure_signed_in(&mut Window { app: app.clone() });
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            after(&handle, gate);
            if let Some(w) = handle.get_webview_window(LABEL) {
                let _ = w.close();
            }
        });
    });
    if let Err(e) = spawned {
        eprintln!("[whimpr] publik gate thread: {e}");
    }
}

/// The scheme handler: `/press?id=<button>` presses a button; every path answers with the page.
pub fn protocol<R: Runtime>(_ctx: UriSchemeContext<'_, R>, req: tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    let uri = req.uri();
    if uri.path() == "/press" {
        let id = uri.query().unwrap_or("").strip_prefix("id=").unwrap_or("");
        if let Some(s) = SHOWING.lock().unwrap().as_ref() {
            if let Some(b) = s.buttons.iter().find(|b| **b == id) {
                let _ = s.tx.send(*b);
            }
        }
    }
    let html = SHOWING.lock().unwrap().as_ref().map(|s| s.html.clone()).unwrap_or_else(waiting);
    tauri::http::Response::builder()
        .status(200)
        .header("content-type", "text/html; charset=utf-8")
        .header("cache-control", "no-store")
        .body(html.into_bytes())
        .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
}

struct Window {
    app: AppHandle,
}

impl GateUi for Window {
    fn show(&mut self, s: &Screen, done: Arc<AtomicBool>) -> Option<&'static str> {
        let (tx, rx) = mpsc::channel();
        *SHOWING.lock().unwrap() = Some(Showing { html: page(s), buttons: s.buttons.iter().map(|(id, _)| *id).collect(), tx });
        let app = self.app.clone();
        let title = s.title;
        let _ = self.app.run_on_main_thread(move || {
            if let Some(w) = app.get_webview_window(LABEL) {
                let _ = w.eval("location.replace('/')");
                let _ = w.set_focus();
                return;
            }
            // The gate is a normal window: in the Dock and in front, until the app's own windows exist.
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
            let url = if cfg!(windows) { format!("http://{SCHEME}.localhost/") } else { format!("{SCHEME}://localhost/") };
            let Ok(url) = tauri::Url::parse(&url) else { return };
            let built = WebviewWindowBuilder::new(&app, LABEL, WebviewUrl::CustomProtocol(url))
                .title(title)
                .inner_size(460.0, 320.0)
                .resizable(false)
                .maximizable(false)
                .center()
                .focused(true)
                .build();
            if let Err(e) = built {
                eprintln!("[whimpr] publik gate window: {e}");
            }
        });
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(id) => return Some(id),
                Err(RecvTimeoutError::Timeout) => {
                    if done.load(Ordering::SeqCst) {
                        return None;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    fn open_url(&mut self, url: &str) {
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(url).spawn();
        #[cfg(windows)]
        {
            // url.dll hands the URL to the default browser without a shell; no console flashes up.
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).creation_flags(0x0800_0000).spawn();
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The page for one screen: title, body, the note under it, and one link per button.
fn page(s: &Screen) -> String {
    let buttons: String = s
        .buttons
        .iter()
        .enumerate()
        .map(|(i, (id, label))| format!(r#"<a class="btn{}" href="/press?id={}">{}</a>"#, if i == 0 { " primary" } else { "" }, id, esc(label)))
        .collect();
    let note = s.note.map(|n| format!(r#"<p class="note">{}</p>"#, esc(n))).unwrap_or_default();
    shell(&esc(s.title), &format!(r#"<p class="body">{}</p>{note}<div class="row">{buttons}</div>"#, esc(&s.body)))
}

fn waiting() -> String {
    shell("Sign in with publik", r#"<p class="body">One moment.</p>"#)
}

fn shell(title: &str, main: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>{title}</title>
<style>
:root {{ color-scheme: light dark; --bg: #ffffff; --fg: #111111; --muted: #666666; --line: #d8d8d8; --accent: #0a84ff; --accent-fg: #ffffff; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg: #1e1e1e; --fg: #f2f2f2; --muted: #a0a0a0; --line: #3a3a3a; }} }}
html, body {{ margin: 0; background: var(--bg); color: var(--fg); font: 14px/1.45 -apple-system, "Segoe UI", system-ui, sans-serif; }}
main {{ padding: 28px 28px 24px; }}
h1 {{ font-size: 18px; font-weight: 600; margin: 0 0 10px; }}
.body {{ margin: 0 0 8px; }}
.note {{ margin: 0 0 8px; color: var(--muted); }}
.row {{ display: flex; gap: 10px; flex-wrap: wrap; margin-top: 18px; }}
.btn {{ display: inline-block; padding: 8px 14px; border: 1px solid var(--line); border-radius: 8px; color: var(--fg); text-decoration: none; background: transparent; }}
.btn.primary {{ background: var(--accent); border-color: var(--accent); color: var(--accent-fg); }}
.btn:focus-visible {{ outline: 2px solid var(--accent); outline-offset: 2px; }}
</style></head>
<body><main><h1>{title}</h1>{main}</main></body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_carries_the_screen_words_and_one_link_per_button() {
        let s = publik::gate_screen("sign_in", None, Some("Waiting for you to sign in in your browser."), None);
        let html = page(&s);
        assert!(html.contains("<h1>Sign in with publik</h1>"));
        assert!(html.contains("Waiting for you to sign in in your browser."));
        assert!(html.contains(r#"href="/press?id=sign_in""#));
        assert_eq!(html.matches("class=\"btn").count(), 1);
        let s = publik::gate_screen("unreachable", None, None, Some("a <b> & c".into()));
        assert!(page(&s).contains("a &lt;b&gt; &amp; c"));
    }
}
