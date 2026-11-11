//! publik: Sign in with publik, usage counting, and publik API as one cleanup engine.
//!
//! The first part of this file is the publik module from
//! https://publikhq.com/sdk/publik.rs.md. WhimprFlow changes four lines of it:
//! 1. `APP_TOKEN` is compiled in from `PUBLIK_APP_TOKEN` at build time (scripts/build-macos.sh,
//!    the CI secret), as every WhimprFlow build before it. There is no token file in the repo.
//! 2. `APP_VERSION` is the Cargo package version.
//! 3. `curl` runs without a console window on Windows.
//! 4. The origin is https://publikhq.com.
//!
//! The second part ("WhimprFlow") is what the app adds on top: the status card the Hub renders,
//! the 402 and revoked-key handling the dictation pipeline calls, and the link between the
//! gate's answer and Settings → Cleanup Engine. The gate window itself is publik_gate.rs.

// publik.rs — Sign in with publik, usage counting, and publik API as one provider option.
// At app start, before any window of your own: `let gate = publik::ensure_signed_in(&mut ui);`. It
//   1. loads or provisions this computer's publik key (POST /api/v1/installs);
//   2. shows Sign in with publik until GET /api/v1/wallet says "claimed" (no other way past it);
//   3. when USES_AI: asks once how the app pays for AI and stores the answer (set_choice() changes it);
//   4. starts usage counting: app_opened now, active_session every 30 minutes (POST /api/v1/telemetry).
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const APP_SLUG: &str = "whimprflow";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// false: the app calls no model. It still signs in and counts users; it skips the plan screen.
pub const USES_AI: bool = true;
/// Ask for a tier, never a vendor model name.
#[allow(dead_code)]
pub const MODELS: [&str; 3] = ["publik-fast", "publik-balanced", "publik-smart"];
// Public by design: it can only mint capped keys attributed to this app. WhimprFlow compiles it
// in from PUBLIK_APP_TOKEN at build time (scripts/build-macos.sh, the CI secret). A build without
// it cannot sign in a new computer: the gate says so. Local and your own key keep working.
const APP_TOKEN: &str = match option_env!("PUBLIK_APP_TOKEN") {
    Some(t) => t,
    None => "",
};
const OS: &str = if cfg!(target_os = "macos") { "macos" } else if cfg!(windows) { "windows" } else { "linux" };

fn origin() -> String { std::env::var("PUBLIK_ORIGIN").unwrap_or_else(|_| "https://publikhq.com".into()) } // PUBLIK_ORIGIN: tests only
pub fn default_base_url() -> String { format!("{}/api/v1", origin()) }

#[allow(dead_code)]
pub enum Provider { Own { base_url: String, api_key: String }, Publik { base_url: String, api_key: String }, NeedsOwnKey }
pub struct Screen { pub id: &'static str, pub title: &'static str, pub body: String, pub note: Option<&'static str>, pub buttons: Vec<(&'static str, &'static str)> }
/// show(): draw the screen and return the button id pressed. Once `done` is set (the gate saw the sign-in), close it and return None.
pub trait GateUi { fn show(&mut self, screen: &Screen, done: Arc<AtomicBool>) -> Option<&'static str>; fn open_url(&mut self, url: &str); }

/// ~/Library/Application Support/publik, %LOCALAPPDATA%\publik, ~/.config/publik: shared by every publik app.
fn publik_dir() -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    let base = if cfg!(target_os = "macos") { home.join("Library/Application Support") }
        else if cfg!(windows) { std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or(home.join("AppData/Local")) }
        else { std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or(home.join(".config")) };
    base.join("publik")
}
pub fn credential_path() -> PathBuf { publik_dir().join("apps").join(format!("{APP_SLUG}.json")) }

pub fn read_credential() -> Option<Value> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(credential_path()).ok()?).ok()?;
    v["key"].as_str().filter(|k| k.starts_with("pk_"))?;
    Some(v)
}
fn write_credential(v: &Value) -> Result<(), String> {
    let path = credential_path();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&path, serde_json::to_string_pretty(v).unwrap()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)); }
    Ok(())
}
/// The person's answer to the plan screen: Some("publik"), Some("own_key"), or None to ask again. Settings call it.
pub fn set_choice(choice: Option<&str>) { if let Some(mut c) = read_credential() { c["choice"] = json!(choice); let _ = write_credential(&c); } }

/// The key this app uses: the stored choice decides. PUBLIK_API_KEY / PUBLIK_API_BASE_URL override (development).
#[allow(dead_code)]
pub fn resolve_provider(user_key: Option<(String, String)>) -> Provider {
    if let Ok(key) = std::env::var("PUBLIK_API_KEY") {
        return Provider::Publik { base_url: std::env::var("PUBLIK_API_BASE_URL").unwrap_or_else(|_| default_base_url()), api_key: key };
    }
    if let Some(c) = read_credential().filter(|c| c["choice"] == "publik") {
        return Provider::Publik { base_url: c["base_url"].as_str().map(String::from).unwrap_or_else(default_base_url), api_key: c["key"].as_str().unwrap_or_default().into() };
    }
    match user_key.filter(|(_, k)| !k.is_empty()) { Some((base_url, api_key)) => Provider::Own { base_url, api_key }, None => Provider::NeedsOwnKey }
}

/// One curl call; `stdin` carries the body or a config with the key. Err: publik is not reachable.
fn curl(args: &[&str], stdin: &str) -> Result<(u16, Value), String> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-w", "\n%{http_code}"]).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); } // CREATE_NO_WINDOW: no console flashes up
    let mut child = cmd.spawn().map_err(|e| format!("curl: {e}"))?;
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).map_err(|e| e.to_string())?;
    let out = String::from_utf8_lossy(&child.wait_with_output().map_err(|e| e.to_string())?.stdout).to_string();
    let (payload, code) = out.rsplit_once('\n').unwrap_or(("", &out));
    match code.trim().parse::<u16>().unwrap_or(0) {
        0 => Err("publik is not reachable. Check your connection.".into()),
        code => Ok((code, serde_json::from_str(payload).unwrap_or(json!({})))),
    }
}
fn key_config(key: &str) -> String { format!("header = \"Authorization: Bearer {key}\"\n") }

/// POST /api/v1/installs: this computer's publik key. Err is a readable message.
pub fn provision() -> Result<Value, String> {
    let runtime = std::env::var("PUBLIK_APP_TOKEN").unwrap_or_default();
    let token = if runtime.trim().is_empty() { APP_TOKEN.trim() } else { runtime.trim() };
    if !token.starts_with("pat_") { return Err("This build has no publik app token. Ask the developer for a new build.".into()); }
    let mut prior = read_credential().unwrap_or(json!({}));
    let mut install_id = prior["install_id"].as_str().map(String::from).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    for _ in 0..2 {
        let body = json!({ "app_token": token, "app_slug": APP_SLUG, "app_version": APP_VERSION, "os": OS, "arch": std::env::consts::ARCH,
            "install_id": install_id, "disclosure_version": 1, "dialects": ["chat_completions"] });
        let url = format!("{}/api/v1/installs", origin());
        let (code, res) = curl(&["-m", "20", "-X", "POST", &url, "-H", "content-type: application/json", "--data-binary", "@-"], &body.to_string())?;
        let key = res["key"].as_str().or(if code == 200 { prior["key"].as_str() } else { None }).map(String::from);
        if let (200 | 201, Some(key)) = (code, key) {
            let c = json!({ "key": key, "base_url": res["base_url"].as_str().map(String::from).unwrap_or_else(default_base_url),
                "install_id": res["install_id"].as_str().unwrap_or(&install_id), "claim_url": res["claim_url"], "choice": prior["choice"] });
            write_credential(&c)?; // the key reaches disk before anything else sees it
            return Ok(c);
        }
        if code == 200 || res["error"]["type"] == "install_revoked" { install_id = uuid::Uuid::new_v4().to_string(); prior = json!({}); continue; }
        return Err(res["error"]["message"].as_str().map(String::from).unwrap_or_else(|| format!("publik could not set up this computer ({code}).")));
    }
    Err("publik could not set up this computer.".into())
}

/// GET /api/v1/wallet: (status, wallet or None, message). Err: publik is not reachable.
pub fn read_wallet(key: &str) -> Result<(u16, Option<Value>, Option<String>), String> {
    let url = format!("{}/api/v1/wallet", origin());
    let (code, mut b) = curl(&["-m", "15", "-K", "-", &url], &key_config(key))?;
    if code != 200 || !b["claim_state"].is_string() { return Ok((code, None, b["error"]["message"].as_str().map(String::from))); }
    if !b["plan"].is_object() { b["plan"] = json!({ "kind": "none", "label": null }); }
    Ok((code, Some(b), None))
}

/// The words of the gate. The same in every publik module.
pub fn gate_screen(id: &'static str, w: Option<&Value>, note: Option<&'static str>, message: Option<String>) -> Screen {
    if id == "unreachable" {
        return Screen { id, title: "Sign in with publik", body: message.unwrap_or_else(|| "publik is not reachable. Check your connection.".into()), note, buttons: vec![("retry", "Retry")] };
    }
    if id == "sign_in" {
        return Screen { id, title: "Sign in with publik", note, buttons: vec![("sign_in", "Sign in with publik")],
            body: "This app uses your publik account. Click Sign in with publik, then sign in in your browser. This window continues when you are signed in.".into() };
    }
    let w = w.cloned().unwrap_or(json!({ "plan": { "kind": "none" } }));
    let who = match w["account"]["handle"].as_str() { Some(h) => format!("You are signed in as @{h}."), None => "You are signed in to publik.".into() };
    if !has_plan(&w) {
        return Screen { id, title: "Choose how to pay for AI", note, body: format!("{who} This app uses AI. Get a publik plan, or use your own key."),
            buttons: vec![("get_plan", "Get a plan"), ("own_key", "Use my own key")] };
    }
    let what = if w["plan"]["kind"] == "plan" { format!("Your plan: {}.", w["plan"]["label"].as_str().unwrap_or("publik")) }
        else { format!("Your balance: ${:.2}.", w["balance_micros"].as_i64().unwrap_or(0) as f64 / 1_000_000.0) };
    Screen { id, title: "Connect your plan", note, body: format!("{who} {what} Connect it to use AI in this app, or use your own key."),
        buttons: vec![("connect", "Connect your plan"), ("own_key", "Use my own key")] }
}
fn claimed(w: &Value) -> bool { w["claim_state"] == "claimed" }
fn has_plan(w: &Value) -> bool { w["plan"]["kind"].as_str().unwrap_or("none") != "none" || w["balance_micros"].as_i64().unwrap_or(0) > 0 }

/// Show a screen; with `until`, poll GET /wallet every 3 s for at most 10 minutes. "done": the poll saw it; "timeout": it did not.
fn ask(ui: &mut dyn GateUi, screen: &Screen, key: &str, until: Option<fn(&Value) -> bool>) -> &'static str {
    let done = Arc::new(AtomicBool::new(false));
    let polled: Arc<Mutex<Option<&'static str>>> = Arc::new(Mutex::new(None));
    if let Some(until) = until {
        let (done, polled, key) = (done.clone(), polled.clone(), key.to_string());
        std::thread::spawn(move || {
            let mut result = "timeout";
            'poll: for _ in 0..200 { // 200 x 3 s = 10 minutes
                for _ in 0..30 { std::thread::sleep(Duration::from_millis(100)); if done.load(Ordering::SeqCst) { return; } }
                if let Ok((_, Some(w), _)) = read_wallet(&key) { if until(&w) { result = "done"; break 'poll; } }
            }
            *polled.lock().unwrap() = Some(result);
            done.store(true, Ordering::SeqCst);
        });
    }
    let action = ui.show(screen, done.clone());
    done.store(true, Ordering::SeqCst);
    let polled = *polled.lock().unwrap();
    action.or(polled).unwrap_or("retry")
}

/// The gate (see the top of the file). Returns (key, wallet, choice) once the person is signed in; choice is None when USES_AI is false.
pub fn ensure_signed_in(ui: &mut dyn GateUi) -> (String, Value, Option<String>) {
    if std::env::var_os("PUBLIK_SELF_TEST").is_some() { println!("{}", self_test()); std::process::exit(0); }
    let (mut sign_note, mut sign_poll, mut plan_note, mut plan_poll): (Option<&'static str>, bool, Option<&'static str>, bool) = (None, true, None, false);
    loop {
        let env_key = std::env::var("PUBLIK_API_KEY").ok().filter(|k| !k.is_empty());
        let c = match env_key.clone().map(|k| Ok(json!({ "key": k }))).or_else(|| read_credential().map(Ok)).unwrap_or_else(provision) {
            Ok(c) => c,
            Err(e) => { ask(ui, &gate_screen("unreachable", None, None, Some(e)), "", None); continue; }
        };
        let key = c["key"].as_str().unwrap_or_default().to_string();
        let w = match read_wallet(&key) {
            Err(_) => { ask(ui, &gate_screen("unreachable", None, None, None), "", None); continue; }
            Ok((401, _, _)) if env_key.is_none() => { let _ = write_credential(&json!({})); continue; } // a revoked key: provision again
            Ok((_, None, message)) => { ask(ui, &gate_screen("unreachable", None, None, message), "", None); continue; }
            Ok((_, Some(w), _)) => w,
        };
        if !claimed(&w) {
            let a = ask(ui, &gate_screen("sign_in", Some(&w), sign_note, None), &key, if sign_poll { Some(claimed) } else { None });
            if a == "sign_in" {
                ui.open_url(w["claim_url"].as_str().or(c["claim_url"].as_str()).unwrap_or(&format!("{}/wallet", origin())));
                (sign_note, sign_poll) = (Some("Waiting for you to sign in in your browser."), true);
            }
            if a == "timeout" { (sign_note, sign_poll) = (Some("No sign-in after 10 minutes. Click Sign in with publik to try again."), false); }
            continue;
        }
        let mut choice = if USES_AI { read_credential().and_then(|c| c["choice"].as_str().map(String::from)) } else { None };
        if USES_AI && choice.is_none() {
            if has_plan(&w) { (plan_note, plan_poll) = (None, false); } // the plan is there: Connect your plan
            let a = ask(ui, &gate_screen("choose", Some(&w), plan_note, None), &key, if plan_poll { Some(has_plan) } else { None });
            (plan_note, plan_poll) = (None, false);
            if a == "get_plan" {
                ui.open_url(w["top_up_url"].as_str().unwrap_or(&format!("{}/wallet", origin())));
                (plan_note, plan_poll) = (Some("Waiting for your plan. Finish in your browser."), true);
            }
            if a == "timeout" { plan_note = Some("No plan after 10 minutes. Click Get a plan to try again."); }
            if a != "connect" && a != "own_key" { continue; } // "done": the plan is there; the next screen is Connect your plan
            choice = Some(if a == "connect" { "publik" } else { "own_key" }.to_string());
            set_choice(choice.as_deref());
        }
        start_telemetry();
        return (key, w, choice);
    }
}

/// Builder check, run by the BUILT app: PUBLIK_SELF_TEST=1 makes ensure_signed_in() print this line and exit.
pub fn self_test() -> String {
    let result = provision().and_then(|c| read_wallet(c["key"].as_str().unwrap_or_default())).and_then(|(code, w, message)| match w {
        Some(_) => Ok(()),
        None => Err(message.unwrap_or_else(|| format!("GET /wallet answered {code}"))),
    });
    match result { Ok(()) => format!("publik self-test: ok {APP_SLUG} {APP_VERSION}"), Err(e) => format!("publik self-test: failed: {e}") }
}

/// A terminal UI for a command-line app. A desktop app passes its own GateUi (see the page).
#[allow(dead_code)]
pub struct TerminalUi;
impl GateUi for TerminalUi {
    fn show(&mut self, s: &Screen, done: Arc<AtomicBool>) -> Option<&'static str> {
        println!("\n{}\n{}{}", s.title, s.body, s.note.map(|n| format!("\n{n}")).unwrap_or_default());
        for (i, (_, label)) in s.buttons.iter().enumerate() { println!("  {}. {label}", i + 1); }
        let answered = Arc::new(AtomicBool::new(false));
        let (d, a) = (done.clone(), answered.clone());
        std::thread::spawn(move || { while !d.load(Ordering::SeqCst) { std::thread::sleep(Duration::from_millis(200)); } if !a.load(Ordering::SeqCst) { println!("\nDone. Press Enter to continue."); } });
        loop {
            print!("{}", if s.buttons.len() > 1 { "Type a number and press Enter: " } else { "Press Enter: " });
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 { while !done.load(Ordering::SeqCst) { std::thread::sleep(Duration::from_millis(200)); } return None; }
            if done.load(Ordering::SeqCst) { return None; }
            let n: usize = line.trim().parse().unwrap_or(1);
            if let Some((action, _)) = s.buttons.get(n.wrapping_sub(1)) { answered.store(true, Ordering::SeqCst); return Some(action); }
        }
    }
    fn open_url(&mut self, url: &str) {
        println!("Open this link if your browser does not open: {url}");
        let _ = if cfg!(target_os = "macos") { Command::new("open").arg(url).spawn() }
            else if cfg!(windows) { Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn() }
            else { Command::new("xdg-open").arg(url).spawn() };
    }
}

// ---------- usage counting: /skills/telemetry.md sections 3-5 ----------
static TELEMETRY_STARTED: AtomicBool = AtomicBool::new(false);

fn usage_off() -> bool {
    let env = |n: &str| std::env::var(n).unwrap_or_default().trim().to_lowercase();
    if !["", "0", "false"].contains(&env("DO_NOT_TRACK").as_str()) || ["0", "false", "off"].contains(&env("PUBLIK_TELEMETRY").as_str()) { return true; }
    std::fs::read_to_string(publik_dir().join("consent.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).map(|v| v["usage"] == false).unwrap_or(false)
}
fn is_uuid_v4(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { c.is_ascii_hexdigit() })
        && b[14] == b'4' && b"89abAB".contains(&b[19])
}
fn utc_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((secs / 86_400) as i64 + 719_468, secs % 86_400); // days since 0000-03-01
    let (era, doe) = (days / 146_097, days % 146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", yoe + era * 400 + i64::from(month <= 2), month, day, rem / 3600, rem % 3600 / 60, rem % 60)
}
/// One random UUID per computer account, shared by every publik app, in <publik dir>/telemetry.json (mode 0600).
fn telemetry_install_id() -> String {
    let file = publik_dir().join("telemetry.json");
    let saved = || std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["install_id"].as_str().map(str::to_lowercase)).filter(|id| is_uuid_v4(id));
    if let Some(id) = saved() { return id; }
    let fresh = uuid::Uuid::new_v4().to_string(); // if the file cannot be saved, this ID lasts for this run only
    let _ = std::fs::create_dir_all(publik_dir());
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true); // never over another app's file
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    match options.open(&file) {
        Ok(mut f) => { let _ = writeln!(f, "{}", json!({ "install_id": fresh, "created_at": utc_now() })); fresh }
        Err(_) => saved().unwrap_or(fresh), // another publik app saved its ID first
    }
}
fn send_event(event: &str, install_id: &str) {
    let say = |result: &str| if std::env::var("PUBLIK_TELEMETRY_DEBUG").as_deref() == Ok("1") { eprintln!("publik-telemetry: {event} -> {result}"); };
    if usage_off() { return say("off (opt-out)"); }
    let key = std::env::var("PUBLIK_API_KEY").ok().or_else(|| read_credential().and_then(|c| c["key"].as_str().map(String::from))).filter(|k| k.starts_with("pk_"));
    let mut body = json!({ "v": 1, "app": APP_SLUG, "install_id": install_id, "event": event, "app_version": APP_VERSION.trim_start_matches(['v', 'V']), "os": OS });
    match std::env::consts::ARCH { "aarch64" => body["arch"] = json!("arm64"), "x86_64" => body["arch"] = json!("x64"), _ => {} }
    let url = std::env::var("PUBLIK_TELEMETRY_URL").unwrap_or_else(|_| format!("{}/api/v1/telemetry", origin()));
    let body = body.to_string();
    let args = ["-m", "5", "-K", "-", "-X", "POST", url.as_str(), "-H", "content-type: application/json", "--data-raw", body.as_str()];
    match curl(&args, &key.map(|k| key_config(&k)).unwrap_or_default()) { Ok((code, _)) => say(&code.to_string()), Err(e) => say(&e) }
}
/// Called by ensure_signed_in(). app_opened now, active_session every 30 minutes, on a background thread; never panics the app.
pub fn start_telemetry() {
    if TELEMETRY_STARTED.swap(true, Ordering::SeqCst) { return; }
    let s: u64 = std::env::var("PUBLIK_TELEMETRY_INTERVAL_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(0); // tests only
    let interval = Duration::from_secs(if (1..=86_400).contains(&s) { s } else { 1_800 });
    std::thread::spawn(move || {
        let id = telemetry_install_id();
        send_event("app_opened", &id);
        loop { std::thread::sleep(interval); send_event("active_session", &id); }
    });
}

// ---------- publik API calls ----------
/// A 402 renders its message plus exactly ONE link: this one.
#[allow(dead_code)]
pub fn top_up_url_from_402(body: &Value) -> (String, Option<String>) {
    let e = &body["error"];
    (e["message"].as_str().unwrap_or("Not enough publik API balance.").into(), e["top_up_url"].as_str().map(String::from))
}

/// `POST /search`: web results for `q` (quote a phrase to find it exactly), one published price per search; Err = (message, top_up_url) from a 402 or a readable failure.
#[allow(dead_code)]
pub fn search_results(base_url: &str, api_key: &str, q: &str, count: u8) -> Result<Vec<Value>, (String, Option<String>)> {
    let (url, body) = (format!("{base_url}/search"), json!({ "q": q, "count": count }).to_string());
    match curl(&["-m", "30", "-K", "-", "-X", "POST", &url, "-H", "content-type: application/json", "--data-raw", &body], &key_config(api_key)) {
        Ok((200, v)) => Ok(v["results"].as_array().cloned().unwrap_or_default()),
        Ok((402, v)) => Err(top_up_url_from_402(&v)),
        Ok((c, _)) => Err((format!("publik API answered {c}."), None)),
        Err(e) => Err((e, None)),
    }
}


// ============================================================================ WhimprFlow
// What WhimprFlow adds to the module: the status card the Hub renders (`PublikStatus`, mirrored
// in ui/src/hub/api.ts), the 402 and revoked-key handling the dictation pipeline calls, and the
// link between the gate's answer and Settings → Cleanup Engine. Copy rule (CONTRACT §1):
// "publik API" is the provider name, prices are dollars, never tokens.

use serde::Serialize;
use std::sync::OnceLock;
use std::time::Instant;
use tauri::{AppHandle, Emitter};
use whimpr_cleanup::publik::{self as api, fmt_usd, BalanceSnapshot, PublikError};
use whimpr_core::{CleanupMode, Settings, PUBLIK_DISCLOSURE_VERSION};

const SERVICE: &str = "com.whimpr.whimprflow";
/// The keychain account builds before 0.2.7 kept the publik key in. `adopt_keychain_key`
/// moves it to the credential file once. No UI path writes it (`set_api_key` takes the two
/// own-key accounts only).
pub const ACCOUNT: &str = "publik_api_key";
// A token with the wrong shape compiles fine and then fails every link with "Invalid or
// missing field: app_token." — that shipped in the 0.2.3 Mac build (the right token plus 12
// extra characters). Refuse to compile it instead. The message never contains the value.
const _: () = assert!(
    APP_TOKEN.is_empty() || api::is_app_token(APP_TOKEN),
    "PUBLIK_APP_TOKEN must be pat_whimprflow_ followed by exactly 32 of [a-z0-9] and nothing else (value not shown)"
);
/// `GET /wallet` at most this often from the Settings pane.
const WALLET_REFRESH_SECS: u64 = 30;

/// The one-sentence justification, shown wherever money is mentioned (CONTRACT §12).
pub const WHY_IT_COSTS: &str = "A provider charges for every request the app makes; publik pays that bill and passes it on at half the provider's list price. Nothing is charged behind your back — usage only draws from a plan or pack you choose to buy.";
/// The settings card's primary button while the account has no plan.
pub const CTA_PICK_PLAN: &str = "Pick a plan";
/// The settings card's primary button once the account is claimed.
pub const CTA_MANAGE_PLAN: &str = "Manage plan";

// ── Status the Hub sees ──────────────────────────────────────────────────────

/// Mirrored by `PublikStatus` in `ui/src/hub/api.ts`.
#[derive(Clone, Serialize, Default, Debug)]
pub struct PublikStatus {
    /// A token is compiled in, or a key is already present.
    pub available: bool,
    pub has_key: bool,
    pub balance_micros: Option<i64>,
    /// "$0.41 left", or "Your plan: Plus" when the account has a plan.
    pub balance_label: Option<String>,
    /// "last cleanup $0.0004"
    pub last_charge_label: Option<String>,
    /// "This week $1.20 of $4.62" (only with a plan budget).
    pub week_label: Option<String>,
    /// `anonymous` | `claimed`, when known.
    pub claim_state: Option<String>,
    /// Link this computer to a publik account (null once claimed).
    pub claim_url: Option<String>,
    /// The one link a 402 told us to render, remembered for the card.
    pub top_up_url: Option<String>,
    pub dashboard_url: String,
    /// The fast-tier alias in use.
    pub model: String,
    /// 402 seen and no balance since.
    pub exhausted: bool,
    /// The key was revoked or refused: the next start signs in again.
    pub disconnected: bool,
    /// Transport error or `503` on the last call. Nothing was charged.
    pub unreachable: bool,
    pub disclosure_needed: bool,
    /// Provisioning is possible in this build (token compiled in).
    pub can_provision: bool,
    /// The first-run card. Always `None` since 0.2.7: the gate's plan screen took its place.
    pub first_run: Option<FirstRunCard>,
    /// [`WHY_IT_COSTS`], so every surface reads the same sentence.
    pub justification: String,
    /// The settings card's primary button (CONTRACT §12.2).
    pub plan_cta: PlanCta,
    /// The non-blocking banner: a 402. Exactly one link.
    pub notice: Option<PublikNotice>,
    /// The plan's label when the account has one: the balance line then reads "Your plan: …".
    #[serde(skip)]
    pub plan_label: Option<String>,
}

/// The first-run card of builds before 0.2.7. Kept for the Hub's type; never built now.
#[allow(dead_code)]
#[derive(Clone, Serialize, Debug, PartialEq, Eq)]
pub struct FirstRunCard {
    pub balance_line: String,
    pub justification: String,
    pub cta_label: String,
    pub claim_url: Option<String>,
    pub later_label: String,
}

/// The settings card's primary button: label + where it goes.
#[derive(Clone, Serialize, Debug, PartialEq, Eq, Default)]
pub struct PlanCta {
    pub label: String,
    pub url: String,
    /// `true` once `claim_state` is `claimed` (the button manages, not picks).
    pub claimed: bool,
}

/// A non-blocking banner: the message (from the response where there is one)
/// and exactly one link.
#[derive(Clone, Serialize, Debug, PartialEq, Eq)]
pub struct PublikNotice {
    /// `exhausted` (402).
    pub kind: String,
    pub headline: String,
    pub message: String,
    pub link_label: String,
    pub link_url: String,
}

static STATE: OnceLock<Mutex<PublikStatus>> = OnceLock::new();
fn state() -> &'static Mutex<PublikStatus> {
    STATE.get_or_init(|| Mutex::new(PublikStatus::default()))
}
static LAST_WALLET_FETCH: Mutex<Option<Instant>> = Mutex::new(None);

// ── This computer's key ──────────────────────────────────────────────────────

/// The publik key in force: `PUBLIK_API_KEY` (development), else the credential file the
/// gate wrote. Never provisions: the gate does that at start.
pub fn read_key() -> Option<String> {
    if let Some(k) = std::env::var("PUBLIK_API_KEY").ok().map(|k| k.trim().to_string()).filter(|k| k.starts_with("pk_")) {
        return Some(k);
    }
    read_credential().and_then(|c| c["key"].as_str().map(String::from))
}

/// The API root in force: `PUBLIK_API_BASE_URL` (development) → the credential's base URL → the default.
pub fn base_url() -> String {
    let from_credential = read_credential().and_then(|c| c["base_url"].as_str().map(String::from)).unwrap_or_default();
    api::resolve_base_url(&from_credential)
}

/// Builds before 0.2.7 kept the publik key in the keychain and the install details in
/// settings. Move them to the credential file once, so the gate finds this computer signed
/// in (or not) without a second mint. Runs before the gate, so it reads the settings file.
pub fn adopt_keychain_key() {
    if read_credential().is_some() {
        return;
    }
    let Ok(entry) = keyring::Entry::new(SERVICE, ACCOUNT) else { return };
    let Some(key) = entry.get_password().ok().map(|k| k.trim().to_string()).filter(|k| k.starts_with("pk_")) else { return };
    let settings = Settings::load(&crate::hotkey::settings_path());
    let base_url = match settings.publik_base_url.trim() {
        "" => default_base_url(),
        u => u.trim_end_matches('/').to_string(),
    };
    let install_id = match settings.publik_install_id.trim() {
        "" => uuid::Uuid::new_v4().to_string(),
        id => id.to_string(),
    };
    let claim_url = api::publik_link(Some(&settings.publik_claim_url));
    let choice = if matches!(settings.cleanup_mode, CleanupMode::Publik) { Some("publik") } else { None };
    let c = json!({ "key": key, "base_url": base_url, "install_id": install_id, "claim_url": claim_url, "choice": choice });
    match write_credential(&c) {
        Ok(()) => {
            let _ = entry.delete_credential();
            eprintln!("[whimpr] publik key moved from the keychain to the credential file");
        }
        Err(e) => eprintln!("[whimpr] publik key stays in the keychain: {e}"),
    }
}

// ── The Settings card ────────────────────────────────────────────────────────

fn set_balance_locked(s: &mut PublikStatus, micros: i64, charge: Option<i64>) {
    s.balance_micros = Some(micros);
    let label = match &s.plan_label {
        Some(plan) => format!("Your plan: {plan}"),
        None => format!("{} left", fmt_usd(micros)),
    };
    s.balance_label = Some(label);
    if let Some(c) = charge {
        s.last_charge_label = Some(format!("last cleanup {}", fmt_usd(c)));
    }
    if micros > 0 || s.plan_label.is_some() {
        s.exhausted = false;
    }
}

fn clear_exhausted_notice(s: &mut PublikStatus) {
    if !s.exhausted && s.notice.as_ref().map(|n| n.kind == "exhausted").unwrap_or(false) {
        s.notice = None;
    }
}

fn week_label(used: Option<i64>, budget: Option<i64>) -> Option<String> {
    match (used, budget) {
        (Some(u), Some(b)) if b > 0 => Some(format!("This week {} of {}", fmt_usd(u), fmt_usd(b))),
        (Some(u), _) => Some(format!("{} used this week", fmt_usd(u))),
        _ => None,
    }
}

/// The settings card's primary button (CONTRACT §12.2).
pub fn plan_cta(claim_state: Option<&str>, claim_url: Option<&str>) -> PlanCta {
    let claimed = claim_state == Some("claimed");
    if claimed {
        PlanCta { label: CTA_MANAGE_PLAN.to_string(), url: api::DASHBOARD_URL.to_string(), claimed }
    } else {
        PlanCta {
            label: CTA_PICK_PLAN.to_string(),
            url: api::publik_link(claim_url).unwrap_or_else(|| api::DASHBOARD_URL.to_string()),
            claimed,
        }
    }
}

/// The 402 banner: the gateway's message (it already says what the link does) plus
/// exactly one link, `top_up_url`, then what the app does about it.
pub fn exhausted_notice(message: &str, claim_state: Option<&str>, link: &str) -> PublikNotice {
    PublikNotice {
        kind: "exhausted".to_string(),
        headline: if claim_state == Some("claimed") { "publik API needs a plan or a pack" } else { "publik API needs a balance" }.to_string(),
        message: format!(
            "{} Dictation still works; text is pasted without cleanup — or use Local or your own key under Settings → Cleanup Engine.",
            message.trim()
        ),
        link_label: if claim_state == Some("claimed") { "Add a plan or pack" } else { "Sign in with publik" }.to_string(),
        link_url: api::publik_link(Some(link)).unwrap_or_else(|| api::DASHBOARD_URL.to_string()),
    }
}

/// After a successful cleanup: the `x-publik-*` headers move the card's
/// balance line live (`whimpr://publik`).
pub fn set_balance_from_snapshot(app: &AppHandle, snap: &BalanceSnapshot) {
    let dto = {
        let mut s = state().lock().unwrap();
        set_balance_locked(&mut s, snap.balance_micros, snap.last_charge_micros);
        s.exhausted = false; // the call went through: the plan or the balance covers it
        clear_exhausted_notice(&mut s);
        if snap.claim_state.is_some() {
            s.claim_state = snap.claim_state.clone();
            if snap.claim_state.as_deref() == Some("claimed") {
                s.claim_url = None;
            }
        }
        if let Some(w) = week_label(snap.week_used_micros, snap.week_budget_micros) {
            s.week_label = Some(w);
        }
        s.unreachable = false;
        s.disconnected = false;
        s.clone()
    };
    let _ = app.emit("whimpr://publik", dto);
}

/// A `GET /wallet` answer (the gate's at start, a refresh since) → the card.
fn apply_wallet(s: &mut PublikStatus, w: &Value) {
    s.plan_label = if w["plan"]["kind"] == "plan" { Some(w["plan"]["label"].as_str().unwrap_or("publik").to_string()) } else { None };
    set_balance_locked(s, w["balance_micros"].as_i64().unwrap_or(0), None);
    s.exhausted = !has_plan(w);
    clear_exhausted_notice(s);
    s.claim_state = w["claim_state"].as_str().map(String::from);
    s.claim_url = if claimed(w) { None } else { api::publik_link(w["claim_url"].as_str()) };
    s.top_up_url = api::publik_link(w["top_up_url"].as_str()).or_else(|| api::publik_link(w["add_credit_url"].as_str()));
    s.week_label = week_label(w["week"]["used_micros"].as_i64(), w["week"]["budget_micros"].as_i64());
    s.unreachable = false;
    s.disconnected = false;
}

/// Keep the wallet the gate read at start-up, so the card is right before the first refresh.
pub fn remember_wallet(w: &Value) {
    apply_wallet(&mut state().lock().unwrap(), w);
}

/// `GET /wallet`, at most once per `WALLET_REFRESH_SECS` unless forced.
fn refresh_wallet(force: bool) {
    let Some(key) = read_key() else { return };
    {
        let mut last = LAST_WALLET_FETCH.lock().unwrap();
        if !force {
            if let Some(t) = *last {
                if t.elapsed() < Duration::from_secs(WALLET_REFRESH_SECS) {
                    return;
                }
            }
        }
        *last = Some(Instant::now());
    }
    match read_wallet(&key) {
        Ok((_, Some(w), _)) => apply_wallet(&mut state().lock().unwrap(), &w),
        Ok((401, None, _)) => {
            eprintln!("[whimpr] publik wallet: this computer's key was not accepted");
            state().lock().unwrap().disconnected = true;
        }
        Ok((code, None, message)) => {
            eprintln!("[whimpr] publik wallet refresh failed ({code}): {}", message.unwrap_or_default());
            if code >= 500 {
                state().lock().unwrap().unreachable = true;
            }
        }
        Err(e) => {
            eprintln!("[whimpr] publik wallet refresh failed: {e}");
            state().lock().unwrap().unreachable = true;
        }
    }
}

// ── Error handling shared by the dictation pipeline ─────────────────────────

/// 402 from the gateway: remember it, and put the message plus its one link in
/// front of the user as a non-blocking banner (`whimpr://publik`). Dictation
/// carries on — raw text is pasted, and BYO / Local are untouched.
fn report_exhausted(app: &AppHandle, message: &str, available_micros: i64, top_up_url: Option<String>, claim_state: Option<String>) {
    let mut s = state().lock().unwrap();
    s.plan_label = None; // a 402 means no plan covers this request
    set_balance_locked(&mut s, available_micros, None);
    s.exhausted = true;
    if claim_state.is_some() {
        s.claim_state = claim_state;
    }
    if let Some(u) = api::publik_link(top_up_url.as_deref()) {
        s.top_up_url = Some(u);
    }
    // The one link: the gateway's top_up_url, else the claim link, else the dashboard.
    let link = s.top_up_url.clone().or_else(|| s.claim_url.clone()).unwrap_or_else(|| api::DASHBOARD_URL.to_string());
    let notice = exhausted_notice(message, s.claim_state.as_deref(), &link);
    eprintln!("[whimpr] ⚠ {}: {}", notice.headline, notice.message);
    s.notice = Some(notice);
    let _ = app.emit("whimpr://publik", s.clone());
}

/// Drop this computer's publik key. The next start shows Sign in with publik again.
pub fn forget_key() {
    let _ = std::fs::remove_file(credential_path());
    if let Ok(e) = keyring::Entry::new(SERVICE, ACCOUNT) {
        let _ = e.delete_credential();
    }
    let mut s = state().lock().unwrap();
    s.has_key = false;
    s.balance_micros = None;
    s.balance_label = None;
    s.last_charge_label = None;
    s.week_label = None;
    s.plan_label = None;
    s.exhausted = false;
    s.first_run = None;
    s.notice = None;
}

/// The key is gone or refused: drop it, say so once, loudly, and tell the person what to do.
fn disconnect(app: &AppHandle, detail: &str) {
    forget_key();
    crate::hotkey::rebuild_providers();
    {
        let mut s = state().lock().unwrap();
        s.disconnected = true;
        let _ = app.emit("whimpr://publik", s.clone());
    }
    crate::diag::report_text(app, "publik API is disconnected".to_string(), detail.to_string());
}

/// What the dictation pipeline calls with a `PublikError` from `cleanup`.
/// Dictation itself is never blocked — the caller has already decided to paste
/// raw; this only decides what the user is told and whether the key survives.
pub fn handle_error(app: &AppHandle, e: &PublikError) {
    match e {
        PublikError::InsufficientCredit { message, available_micros, top_up_url, claim_state, .. } => {
            report_exhausted(app, message, *available_micros, top_up_url.clone(), claim_state.clone());
        }
        PublikError::InvalidKey => {
            eprintln!("[whimpr] publik key rejected (invalid_api_key) — forgetting it");
            disconnect(app, "publik API did not accept this computer's key. Quit and open WhimprFlow again to sign in, or use Local or your own key. Dictation still works; text is pasted without cleanup.");
        }
        PublikError::KeyRevoked { reprovision: true } => {
            // The idle sweep reset this computer's key. The gate mints a new one at the next start.
            eprintln!("[whimpr] publik key reset by the idle sweep — sign in again at the next start");
            disconnect(app, "This computer's publik key was reset. Quit and open WhimprFlow again to sign in, or use Local or your own key. Dictation still works; text is pasted without cleanup.");
        }
        PublikError::KeyRevoked { reprovision: false } => {
            eprintln!("[whimpr] publik key revoked from the account — disconnected");
            disconnect(app, "This computer was removed from your publik account. Quit and open WhimprFlow again to sign in, or use Local or your own key. Dictation still works; text is pasted without cleanup.");
        }
        PublikError::RateLimited { kind, retry_after_secs } => {
            eprintln!("[whimpr] publik rate limited ({kind}, retry after {retry_after_secs:?}) — pasting raw");
        }
        PublikError::Unavailable { .. } | PublikError::Transport(_) => {
            eprintln!("[whimpr] publik unreachable ({e}) — pasting raw, nothing charged");
            let mut s = state().lock().unwrap();
            s.unreachable = true;
            let _ = app.emit("whimpr://publik", s.clone());
        }
        PublikError::Http { .. } => {
            eprintln!("[whimpr] publik error ({e}) — pasting raw");
        }
    }
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    {
        // url.dll hands the URL to the default browser without a shell; no console flashes up.
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).creation_flags(0x0800_0000).spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

fn snapshot() -> PublikStatus {
    let settings = crate::hotkey::current_settings();
    let mut s = state().lock().unwrap().clone();
    s.has_key = read_key().is_some();
    s.can_provision = !APP_TOKEN.trim().is_empty();
    s.available = s.has_key || s.can_provision;
    s.disclosure_needed = settings.publik_disclosure_version < PUBLIK_DISCLOSURE_VERSION;
    s.dashboard_url = api::DASHBOARD_URL.to_string();
    if s.model.is_empty() {
        s.model = if settings.publik_model.is_empty() { api::DEFAULT_MODEL_ALIAS.to_string() } else { settings.publik_model.clone() };
    }
    if s.claim_url.is_none() && s.claim_state.as_deref() != Some("claimed") {
        s.claim_url = read_credential().and_then(|c| api::publik_link(c["claim_url"].as_str()));
    }
    s.justification = WHY_IT_COSTS.to_string();
    s.plan_cta = plan_cta(s.claim_state.as_deref(), s.claim_url.as_deref());
    s.first_run = None;
    s
}

/// Only ever hand the system browser a publikhq.com link (CONTRACT §11.4).
fn open_publik_url(url: &str) {
    match api::publik_link(Some(url)) {
        Some(u) => open_url(&u),
        None => eprintln!("[whimpr] refused to open a link off publikhq.com"),
    }
}

// ── The gate's answer and Settings → Cleanup Engine ─────────────────────────

/// The gate's answer, applied once at start. "publik" selects publik API and stamps the
/// disclosure (the gate's plan screen said what it costs); "own_key" moves a publik selection
/// back to Local, the default, and leaves an own-key engine as it is.
pub fn apply_choice(choice: Option<&str>) {
    let mut settings = crate::hotkey::current_settings();
    let (mode, version, pending) = (settings.cleanup_mode.clone(), settings.publik_disclosure_version, settings.publik_cta_pending);
    match choice {
        Some("publik") => {
            settings.cleanup_mode = CleanupMode::Publik;
            settings.publik_disclosure_version = PUBLIK_DISCLOSURE_VERSION;
        }
        Some("own_key") => {
            if matches!(settings.cleanup_mode, CleanupMode::Publik) {
                settings.cleanup_mode = CleanupMode::Local;
            }
        }
        _ => {}
    }
    settings.publik_cta_pending = false;
    if settings.cleanup_mode != mode || settings.publik_disclosure_version != version || settings.publik_cta_pending != pending {
        crate::hotkey::update_settings(settings);
    }
}

/// Settings → Cleanup Engine changed the engine: store the same answer in the credential
/// file, so the gate does not ask again at the next start.
pub fn sync_choice(before: &Settings, after: &Settings) {
    if before.cleanup_mode != after.cleanup_mode {
        set_choice(Some(if matches!(after.cleanup_mode, CleanupMode::Publik) { "publik" } else { "own_key" }));
    }
}

// ── Tauri commands (registered in lib.rs) ──────────────────────────────────

/// Cheap and synchronous: no network. The Settings card renders from this.
#[tauri::command]
pub fn get_publik_status() -> PublikStatus {
    snapshot()
}

/// `GET /wallet` (throttled) — the Settings pane calls it on open so the
/// balance line is fresh without waiting for the next dictation.
#[tauri::command]
pub async fn publik_refresh_wallet(force: Option<bool>) -> Result<PublikStatus, String> {
    let force = force.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        refresh_wallet(force);
        snapshot()
    })
    .await
    .map_err(|e| e.to_string())
}

/// The disclosure card's "Turn on publik API": select publik API for this computer's key
/// (the gate signed it in at start), stamp the disclosure, remember the answer for the gate.
#[tauri::command]
pub async fn publik_accept_disclosure() -> Result<PublikStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if read_key().is_none() {
            return Err("This computer is not signed in to publik. Quit and open WhimprFlow again to sign in.".to_string());
        }
        let mut settings = crate::hotkey::current_settings();
        settings.cleanup_mode = CleanupMode::Publik;
        settings.publik_disclosure_version = PUBLIK_DISCLOSURE_VERSION;
        settings.publik_cta_pending = false;
        crate::hotkey::update_settings(settings);
        set_choice(Some("publik"));
        refresh_wallet(true);
        Ok(snapshot())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "Forget this key": drops this computer's publik key. The engine moves off publik API,
/// and the next start shows Sign in with publik again.
#[tauri::command]
pub fn publik_forget_key() -> PublikStatus {
    forget_key();
    let mut settings = crate::hotkey::current_settings();
    if matches!(settings.cleanup_mode, CleanupMode::Publik) {
        settings.cleanup_mode = CleanupMode::Local;
        crate::hotkey::update_settings(settings); // rebuilds the providers too
    } else {
        crate::hotkey::rebuild_providers();
    }
    snapshot()
}

/// "Later" on the first-run card of older builds: the card is no longer owed.
#[tauri::command]
pub fn publik_dismiss_first_run() -> PublikStatus {
    let mut settings = crate::hotkey::current_settings();
    if settings.publik_cta_pending {
        settings.publik_cta_pending = false;
        crate::hotkey::update_settings(settings);
    }
    state().lock().unwrap().first_run = None;
    snapshot()
}

/// Close the non-blocking banner. A later 402 re-raises it.
#[tauri::command]
pub fn publik_dismiss_notice() -> PublikStatus {
    state().lock().unwrap().notice = None;
    snapshot()
}

/// Open the claim page, the plan button's target, the banner's one link, the
/// usage dashboard, the terms, or the 402's link — publikhq.com only.
#[tauri::command]
pub fn publik_open_link(kind: String) {
    let s = snapshot();
    let url = match kind.as_str() {
        "claim" => s.claim_url.clone().unwrap_or_else(|| s.dashboard_url.clone()),
        "first_run" => {
            let _ = publik_dismiss_first_run();
            s.claim_url.clone().unwrap_or_else(|| s.dashboard_url.clone())
        }
        "plan" => s.plan_cta.url.clone(),
        "notice" => s.notice.as_ref().map(|n| n.link_url.clone()).unwrap_or_else(|| s.dashboard_url.clone()),
        "top_up" => s.top_up_url.clone().or_else(|| s.claim_url.clone()).unwrap_or_else(|| s.dashboard_url.clone()),
        "terms" => api::TERMS_URL.to_string(),
        _ => s.dashboard_url.clone(),
    };
    open_publik_url(&url);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_gate_screens_carry_the_contract_words() {
        let anonymous = json!({ "claim_state": "anonymous", "plan": { "kind": "none" }, "balance_micros": 0 });
        let s = gate_screen("sign_in", Some(&anonymous), None, None);
        assert_eq!(s.title, "Sign in with publik");
        assert_eq!(s.buttons, vec![("sign_in", "Sign in with publik")]);
        let s = gate_screen("unreachable", None, None, None);
        assert_eq!(s.body, "publik is not reachable. Check your connection.");
        assert_eq!(s.buttons, vec![("retry", "Retry")]);
        let empty = json!({ "claim_state": "claimed", "account": { "handle": "mann" }, "plan": { "kind": "none" }, "balance_micros": 0 });
        let s = gate_screen("choose", Some(&empty), None, None);
        assert_eq!(s.title, "Choose how to pay for AI");
        assert_eq!(s.body, "You are signed in as @mann. This app uses AI. Get a publik plan, or use your own key.");
        assert_eq!(s.buttons, vec![("get_plan", "Get a plan"), ("own_key", "Use my own key")]);
        let plan = json!({ "claim_state": "claimed", "account": { "handle": "mann" }, "plan": { "kind": "plan", "label": "Plus" }, "balance_micros": 0 });
        let s = gate_screen("choose", Some(&plan), None, None);
        assert_eq!(s.title, "Connect your plan");
        assert_eq!(s.body, "You are signed in as @mann. Your plan: Plus. Connect it to use AI in this app, or use your own key.");
        assert_eq!(s.buttons, vec![("connect", "Connect your plan"), ("own_key", "Use my own key")]);
    }

    #[test]
    fn the_402_notice_renders_the_message_and_exactly_one_link() {
        let n = exhausted_notice("Not enough publik API balance.", Some("claimed"), "https://publikhq.com/dashboard/api/add");
        assert_eq!(n.kind, "exhausted");
        assert!(n.message.starts_with("Not enough publik API balance."));
        assert_eq!(n.link_label, "Add a plan or pack");
        assert_eq!(n.link_url, "https://publikhq.com/dashboard/api/add");
        // A link off publikhq.com is dropped; the dashboard stands in.
        let off = exhausted_notice("m", Some("claimed"), "https://phish.example/x");
        assert_eq!(off.link_url, api::DASHBOARD_URL);
    }

    #[test]
    fn links_off_publikhq_com_are_dropped_everywhere() {
        assert_eq!(plan_cta(Some("anonymous"), Some("https://evil.example/claim")).url, api::DASHBOARD_URL);
        assert_eq!(plan_cta(Some("anonymous"), Some("https://publikhq.com/claim/AB12")).url, "https://publikhq.com/claim/AB12");
        assert!(plan_cta(Some("claimed"), None).claimed);
        assert_eq!(plan_cta(Some("claimed"), None).label, CTA_MANAGE_PLAN);
        let mut s = PublikStatus::default();
        apply_wallet(&mut s, &json!({ "claim_state": "claimed", "balance_micros": 410_000, "plan": { "kind": "none" }, "top_up_url": "https://phish.example/x", "week": { "used_micros": 1_200_000, "budget_micros": 4_620_000 } }));
        assert_eq!(s.top_up_url, None);
        assert_eq!(s.balance_label.as_deref(), Some("$0.41 left"));
        assert!(!s.exhausted);
        assert_eq!(s.week_label.as_deref(), Some("This week $1.20 of $4.62"));
        apply_wallet(&mut s, &json!({ "claim_state": "claimed", "balance_micros": 0, "plan": { "kind": "plan", "label": "Plus" } }));
        assert_eq!(s.balance_label.as_deref(), Some("Your plan: Plus"));
        assert!(!s.exhausted);
        apply_wallet(&mut s, &json!({ "claim_state": "claimed", "balance_micros": 0, "plan": { "kind": "none" } }));
        assert!(s.exhausted);
    }

    #[test]
    fn cta_copy_says_publik_api_in_dollars_and_never_credits() {
        let n = exhausted_notice("Not enough publik API balance.", Some("claimed"), "https://publikhq.com/wallet");
        let texts = [n.headline.as_str(), n.message.as_str(), n.link_label.as_str(), CTA_PICK_PLAN, CTA_MANAGE_PLAN, WHY_IT_COSTS];
        for t in texts {
            let lower = t.to_ascii_lowercase();
            assert!(!t.contains("OpenAI API access"), "{t}");
            assert!(!t.contains("ChatGPT"), "{t}");
            assert!(!lower.contains("credits"), "'credits' as a unit in: {t}");
            assert!(!lower.contains("token"), "tokens, not dollars, in: {t}");
            assert!(!lower.contains("openai") && !lower.contains("anthropic"), "provider named in: {t}");
            assert!(!lower.contains("free starter"), "pre-0059 starter promise in: {t}");
        }
        assert!(WHY_IT_COSTS.contains("half the provider's list price"));
        assert!(WHY_IT_COSTS.contains("Nothing is charged behind your back"));
    }

    #[test]
    fn week_label_reads_like_the_settings_copy() {
        assert_eq!(week_label(Some(1_200_000), Some(4_620_000)).as_deref(), Some("This week $1.20 of $4.62"));
        assert_eq!(week_label(Some(680_000), None).as_deref(), Some("$0.68 used this week"));
        assert_eq!(week_label(None, None), None);
    }

    /// The copy rule (CONTRACT §1): "publik API" is the provider name; the
    /// two forbidden phrases below, "credits" as a unit and any per-token
    /// dollar figure never appear in the publik surfaces. Scans the non-test
    /// half of the Rust sources, every Hub UI source, and the README.
    #[test]
    fn copy_rule_holds_in_every_publik_surface() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = vec![
            root.join("src-tauri/src/publik.rs"),
            root.join("crates/whimpr-cleanup/src/publik.rs"),
            root.join("README.md"),
        ];
        for entry in std::fs::read_dir(root.join("ui/src/hub")).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().map(|e| e == "tsx" || e == "ts").unwrap_or(false) {
                files.push(p);
            }
        }
        assert!(files.iter().any(|f| f.ends_with("CloudDisclosure.tsx")), "the disclosure card must exist");
        let forbidden = ["OpenAI API access", "ChatGPT credits", "ChatGPT credit"];
        // The promises publik migration 0059 (founder, 2026-09-28) retired: a new
        // computer starts at $0.00; the one free thing is $0.05 for linking it, once.
        let old_policy = ["free starter", "first $0.25", "$0.25 is free", "starts with free", "small free balance"];
        let per_token = ["per token", "/token", "per 1M tokens", "per million tokens", "/1M tokens"];
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            // Rust files: only the shipped half — the test module is where the
            // forbidden strings are spelled out.
            let text = match text.split_once("#[cfg(test)]") {
                Some((shipped, _)) if f.extension().map(|e| e == "rs").unwrap_or(false) => shipped.to_string(),
                _ => text,
            };
            for line in text.lines() {
                for bad in forbidden {
                    assert!(!line.contains(bad), "{}: forbidden string {bad:?} in: {line}", f.display());
                }
                // "credits" as a unit ("500 credits", "buy credits"); "credit" singular is the contract's own word.
                let lower = line.to_ascii_lowercase();
                for bad in old_policy {
                    assert!(!lower.contains(bad), "{}: pre-0059 starter promise {bad:?} in: {line}", f.display());
                }
                assert!(!lower.contains("credits"), "{}: 'credits' as a unit in: {line}", f.display());
                if line.contains('$') {
                    for bad in per_token {
                        assert!(!lower.contains(bad), "{}: per-token dollar figure in: {line}", f.display());
                    }
                }
                // No hourly-cost figure (R25 S17): the rate + "under $2 a month" only.
                assert!(!(line.contains('$') && (lower.contains("/hour") || lower.contains("per hour") || lower.contains("an hour"))), "{}: hourly cost figure in: {line}", f.display());
            }
        }
    }
}
