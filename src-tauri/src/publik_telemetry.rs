//! publik_telemetry.rs — publik telemetry v1 client for Rust (Tauri, egui, CLI).
//! It sends two usage events to publik: app_opened and active_session. Nothing else.
//!
//! How to add it:
//!   1. Copy this file into `src/` and add `mod publik_telemetry;` to `main.rs`.
//!   2. Add `ureq = "2"` to Cargo.toml. If the app has reqwest, swap `http_post` below instead.
//!   3. Call `publik_telemetry::start("your-slug", env!("CARGO_PKG_VERSION"), None);` at start-up.
//!
//! Opt-outs: DO_NOT_TRACK=1, PUBLIK_TELEMETRY=0, or "usage": false in <publik dir>/consent.json.
//! Docs: https://publikhq.com/skills/telemetry.md
//! Public domain (CC0). Copy it into your app.
#![allow(dead_code)] // an app does not need to call stop()

use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ENDPOINT: &str = "https://publikhq.com/api/v1/telemetry";
const TIMEOUT: Duration = Duration::from_secs(5);

/// Returns the app's current publik key (`pk_...`), or `None` when the user is signed out.
pub type KeyProvider = Box<dyn Fn() -> Option<String> + Send + 'static>;

static STARTED: AtomicBool = AtomicBool::new(false);
static STOP: Mutex<Option<Sender<()>>> = Mutex::new(None); // stop() drops the sender; the thread then ends

// ---- HTTP: the only code that needs a crate. This copy uses the app's reqwest (feature "blocking"). ----
fn http_post(url: &str, body: &str, key: Option<&str>) -> Result<u16, String> {
    let client = reqwest::blocking::Client::builder().timeout(TIMEOUT).build().map_err(|e| e.to_string())?;
    let mut request = client.post(url).header("Content-Type", "application/json").body(body.to_string());
    if let Some(key) = key {
        request = request.header("Authorization", format!("Bearer {}", key));
    }
    request.send().map(|response| response.status().as_u16()).map_err(|e| e.to_string())
}
// ----------------------------------------------------------------------------------------------

/// Sends `app_opened` on a background thread and starts the 30-minute `active_session` timer.
/// A second call does nothing. `key_provider` runs on the background thread before each send.
pub fn start(app_slug: &str, app_version: &str, key_provider: Option<KeyProvider>) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let slug = app_slug.trim().to_lowercase();
    let version = clean_version(app_version);
    let seconds = env("PUBLIK_TELEMETRY_INTERVAL_SECONDS").parse::<f64>().ok(); // tests only
    let interval = Duration::from_secs_f64(seconds.filter(|s| (1.0..=86400.0).contains(s)).unwrap_or(1800.0));
    let (stop_sender, stop_signal) = channel::<()>();
    *STOP.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop_sender);
    let _ = std::thread::Builder::new().name("publik-telemetry".into()).spawn(move || {
        let mut install_id = None; // one install ID for the whole run
        send("app_opened", &slug, &version, key_provider.as_ref(), &mut install_id);
        while let Err(RecvTimeoutError::Timeout) = stop_signal.recv_timeout(interval) {
            send("active_session", &slug, &version, key_provider.as_ref(), &mut install_id);
        }
    });
}

/// The server takes 1 to 64 printable ASCII characters, with no "v" at the start and no space at an end.
fn clean_version(value: &str) -> String {
    let text: String = value.chars().filter(|c| (' '..='~').contains(c)).collect();
    let text: String = text.trim_start_matches(|c| matches!(c, 'v' | 'V' | ' ')).chars().take(64).collect();
    Some(text.trim_end()).filter(|t| !t.is_empty()).unwrap_or("0").to_string()
}

/// Optional. Stops the timer.
pub fn stop() {
    STOP.lock().unwrap_or_else(|e| e.into_inner()).take();
}

/// One attempt: no retry, 5 second limit, never panics.
fn send(event: &str, slug: &str, version: &str, key_provider: Option<&KeyProvider>, install: &mut Option<String>) {
    let debug = env("PUBLIK_TELEMETRY_DEBUG") == "1";
    let say = |result: String| {
        let _ = debug && writeln!(std::io::stderr(), "publik-telemetry: {} -> {}", event, result).is_ok();
    };
    let os = std::env::consts::OS;
    if !matches!(os, "macos" | "windows" | "linux") {
        return;
    }
    let arch = match std::env::consts::ARCH {
        "aarch64" => r#","arch":"arm64""#,
        "x86_64" => r#","arch":"x64""#,
        _ => "",
    };
    let dir = publik_dir();
    if opted_out(dir.as_deref()) {
        return say("off (opt-out)".into());
    }
    let id = install.get_or_insert_with(|| install_id(dir.as_deref())).clone();
    let body = format!(r#"{{"v":1,"app":"{}","install_id":"{}","event":"{}","app_version":"{}","os":"{}"{}}}"#, esc(slug), id, event, esc(version), os, arch);
    let url = Some(env("PUBLIK_TELEMETRY_URL")).filter(|u| !u.is_empty()).unwrap_or_else(|| ENDPOINT.into());
    let key = publik_key(dir.as_deref(), slug, key_provider);
    say(http_post(&url, &body, key.as_deref()).map(|status| status.to_string()).unwrap_or_else(|error| error));
}

fn env(name: &str) -> String {
    std::env::var_os(name).map(|v| v.to_string_lossy().trim().to_string()).unwrap_or_default()
}

fn esc(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).flat_map(|c| if c == '"' || c == '\\' { vec!['\\', c] } else { vec![c] }).collect()
}

/// <publik dir>: the folder that all publik apps share on this computer account.
/// A folder from the environment counts only when it is an absolute path.
fn publik_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let home = var("HOME").or_else(|| var("USERPROFILE"));
    let base = if cfg!(target_os = "macos") {
        home?.join("Library").join("Application Support")
    } else if cfg!(target_os = "windows") {
        var("LOCALAPPDATA").or_else(|| home.map(|h| h.join("AppData").join("Local")))?
    } else {
        var("XDG_CONFIG_HOME").or_else(|| home.map(|h| h.join(".config")))?
    };
    Some(base.join("publik"))
}

/// The text of a small file: UTF-8, UTF-16 or UTF-32, with or without a byte-order mark.
/// Notepad and PowerShell on Windows save UTF-16, or UTF-8 with a mark.
fn read_text(path: &Path) -> Result<String, ErrorKind> {
    let raw = std::fs::read(path).map_err(|error| error.kind())?;
    let zero = |i: usize| raw.get(i) == Some(&0);
    let full = |i: usize| matches!(raw.get(i), Some(byte) if *byte != 0);
    let (size, big) = if (raw.starts_with(&[0xff, 0xfe]) && zero(2) && zero(3)) || (full(0) && zero(1) && zero(2) && zero(3)) {
        (4, false) // UTF-32, little-endian
    } else if zero(0) && zero(1) && (raw[2..].starts_with(&[0xfe, 0xff]) || (zero(2) && full(3))) {
        (4, true)
    } else if raw.starts_with(&[0xff, 0xfe]) || (full(0) && zero(1)) {
        (2, false) // UTF-16, little-endian
    } else if raw.starts_with(&[0xfe, 0xff]) || (zero(0) && full(1)) {
        (2, true)
    } else {
        (1, false)
    };
    let unit = |bytes: &[u8]| bytes.iter().enumerate().fold(0u32, |sum, (i, byte)| sum | (*byte as u32) << (8 * if big { size - 1 - i } else { i }));
    let units = raw.chunks_exact(size).map(unit);
    let text: String = match size {
        1 => String::from_utf8_lossy(&raw).into_owned(),
        2 => char::decode_utf16(units.map(|u| u as u16)).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect(),
        _ => units.map(|u| char::from_u32(u).unwrap_or(char::REPLACEMENT_CHARACTER)).collect(),
    };
    Ok(text.trim_start_matches('\u{feff}').to_string())
}

/// Reads `"name": <string | true | false>` from the text of a small flat JSON file. Not a full JSON parser.
fn json_field(text: &str, name: &str) -> Option<String> {
    let needle = format!("\"{}\"", name);
    let mut rest = text;
    while let Some(at) = rest.find(&needle) {
        rest = &rest[at + needle.len()..];
        if let Some(value) = rest.trim_start().strip_prefix(':') {
            let value = value.trim_start();
            return match value.strip_prefix('"') {
                Some(string) => string.find('"').map(|end| string[..end].to_string()),
                None => Some(value.chars().take_while(|c| c.is_ascii_alphanumeric()).collect()),
            };
        }
    }
    None
}

/// Opt-out. Checked before each send, before telemetry.json or the network is used.
fn opted_out(dir: Option<&Path>) -> bool {
    !matches!(env("DO_NOT_TRACK").to_lowercase().as_str(), "" | "0" | "false")
        || matches!(env("PUBLIK_TELEMETRY").to_lowercase().as_str(), "0" | "false" | "off")
        || dir.and_then(|d| read_text(&d.join("consent.json")).ok()).and_then(|text| json_field(&text, "usage")).as_deref() == Some("false")
}

fn uuid4() -> String {
    use std::hash::{BuildHasher, Hasher};
    // RandomState takes its seed from the OS random source. That is enough for an anonymous ID.
    let random = || {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        hasher.finish() as u128
    };
    let bits = ((random() << 64 | random()) & !(0xf << 76) & !(0x3 << 62)) | (0x4 << 76) | (0x2 << 62); // version 4, variant 1
    let hex = format!("{:032x}", bits);
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((secs / 86400) as i64 + 719_468, secs % 86400); // days since 0000-03-01
    let (era, doe) = (days / 146_097, days % 146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month, day, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// What telemetry.json holds. `Ok(Some(id))`: a valid ID. `Ok(None)`: the read worked and the content
/// is not valid. `Err(NotFound)`: no such file. Any other `Err`: the read failed.
fn saved_id(path: &Path) -> Result<Option<String>, ErrorKind> {
    let hyphen = |i: usize| matches!(i, 8 | 13 | 18 | 23);
    // Version 4 only: the server takes no other kind.
    let is_uuid4 = |id: &String| {
        let bytes = id.as_bytes();
        bytes.len() == 36
            && bytes.iter().enumerate().all(|(i, b)| if hyphen(i) { *b == b'-' } else { b.is_ascii_hexdigit() })
            && bytes[14] == b'4'
            && matches!(bytes[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b')
    };
    Ok(json_field(&read_text(path)?, "install_id").filter(is_uuid4).map(|id| id.to_lowercase()))
}

/// Saves a new telemetry.json in one step: the full text goes to a temp file, then a hard link gives it
/// the real name. An app that starts at the same time sees no file or the full file, never an empty file.
/// `Err(AlreadyExists)`: another app saved its file first.
fn save_new(path: &Path, text: &str) -> Result<(), ErrorKind> {
    let mut file = std::fs::OpenOptions::new();
    file.write(true).create_new(true);
    #[cfg(unix)]
    {
        std::os::unix::fs::OpenOptionsExt::mode(&mut file, 0o600);
    }
    let write = |to: &Path| file.open(to).and_then(|mut handle| handle.write_all(text.as_bytes())).map_err(|error| error.kind());
    let temp = path.with_extension(format!("json.{}.{}.tmp", std::process::id(), &uuid4()[..8]));
    let result = write(&temp).and_then(|()| match std::fs::hard_link(&temp, path).map_err(|error| error.kind()) {
        Err(ErrorKind::AlreadyExists) => Err(ErrorKind::AlreadyExists),
        Err(_) => write(path), // no hard links on this disk: plain exclusive create
        Ok(()) => Ok(()),
    });
    let _ = std::fs::remove_file(&temp);
    result
}

/// One random UUID v4 per computer account, in <publik dir>/telemetry.json (mode 0600).
fn install_id(dir: Option<&Path>) -> String {
    let fresh = uuid4(); // if the file cannot be read or saved, this ID lasts for this run only
    let Some(dir) = dir else { return fresh };
    let path = dir.join("telemetry.json");
    let text = format!("{{\"install_id\": \"{}\", \"created_at\": \"{}\"}}\n", fresh, utc_now());
    let mut folder = std::fs::DirBuilder::new();
    folder.recursive(true);
    #[cfg(unix)]
    {
        std::os::unix::fs::DirBuilderExt::mode(&mut folder, 0o700);
    }
    let mut replaced = false;
    for _round in 0..3 {
        let mut saved = saved_id(&path);
        if saved == Ok(None) && !replaced {
            std::thread::sleep(Duration::from_millis(100)); // an app with an old client can be in the middle of its save
            saved = saved_id(&path);
            if saved == Ok(None) {
                replaced = true;
                saved = match std::fs::remove_file(&path).map_err(|error| error.kind()) {
                    Ok(()) | Err(ErrorKind::NotFound) => Err(ErrorKind::NotFound), // the content is not valid: replace the file once
                    Err(_) => return fresh,
                };
            }
        }
        match saved {
            Ok(Some(found)) => return found,
            Err(ErrorKind::NotFound) => {}
            _ => return fresh, // the read failed: keep the file
        }
        if folder.create(dir).is_err() || save_new(&path, &text) != Err(ErrorKind::AlreadyExists) {
            return fresh;
        }
        // another publik app saved its ID first: the next round reads that one
    }
    fresh
}

/// Key order: the app's own publik key, then PUBLIK_API_KEY, then <publik dir>/apps/<slug>.json.
/// Only a publik key (`pk_...`) is sent. A provider key such as `sk-...` is never sent.
fn publik_key(dir: Option<&Path>, slug: &str, key_provider: Option<&KeyProvider>) -> Option<String> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let clean = |key: String| Some(key.trim().to_string()).filter(|k| k.len() > 3 && k.starts_with("pk_") && k.chars().all(word));
    let safe_slug = slug.starts_with(|c: char| c.is_ascii_alphanumeric()) && slug.chars().all(|c| word(c) || c == '.');
    let from_file = || dir.filter(|_| safe_slug).and_then(|d| read_text(&d.join("apps").join(format!("{}.json", slug))).ok()).and_then(|text| json_field(&text, "key"));
    let from_app = key_provider.and_then(|provider| provider()).and_then(clean);
    from_app.or_else(|| clean(env("PUBLIK_API_KEY"))).or_else(|| from_file().and_then(clean))
}
