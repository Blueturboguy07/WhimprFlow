//! The Whisper speech model: where it lives, loading it, and fetching it.
//!
//! v0.2.1 shipped as a 4.9 MB dmg with no speech model inside, and the only
//! remedy it offered was a banner telling the reader to fetch a file from
//! Hugging Face by hand, put it in a hidden folder, and relaunch. For anyone
//! who installed from the dmg that is a dead end. Two fixes live here:
//!
//! 1. A release build carries `ggml-base.en.bin` inside the app
//!    (`Contents/Resources/models/`, placed there by scripts/build-macos.sh).
//!    It is the last candidate, so a bigger model the reader added still wins.
//! 2. When no usable model is found anyway (a source build, a deleted or
//!    damaged file), the Hub shows one button. It downloads the model into the
//!    models folder, checks its size and SHA-256, and loads it on the spot —
//!    no relaunch.
//!
//! Both platform layers (`hotkey.rs` on macOS, `win.rs` on Windows) read the
//! engine through [`engine`] instead of each keeping their own copy.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

/// The model a release bundles and the download button fetches: English-only,
/// 148 MB, fast to load. docs/MODELS.md lists the bigger ones.
pub const DEFAULT_FILE: &str = "ggml-base.en.bin";
const DEFAULT_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin";
/// From the Hugging Face LFS pointer for that file. scripts/fetch-speech-model.sh
/// pins the same two values for the copy a release bundles.
const DEFAULT_SHA256: &str = "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002";
const DEFAULT_BYTES: u64 = 147_964_211;

/// Models the reader may have added, most accurate first. Bigger English
/// models mis-hear names and technical terms far less.
#[cfg(target_os = "macos")]
const CANDIDATES: &[&str] = &[
    "ggml-large-v3-turbo.bin",
    "ggml-medium.en.bin",
    "ggml-small.en.bin",
    DEFAULT_FILE,
];
// large-v3-turbo is only fast enough with Metal; Windows runs whisper on the CPU.
#[cfg(not(target_os = "macos"))]
const CANDIDATES: &[&str] = &["ggml-medium.en.bin", "ggml-small.en.bin", DEFAULT_FILE];

const EVENT: &str = "whimpr://asr-model";
const PROMPT_EVENT: &str = "whimpr://asr-model/prompt";
const HUB_LABEL: &str = "main";

/// Where the speech model stands. Sent to the Hub as `whimpr://asr-model`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelState {
    /// Launch: looking for a model, or loading one (about a second).
    Loading,
    /// Loaded and transcribing. `bundled` is true for the copy inside the app.
    Ready { file: String, bundled: bool },
    /// No model file loaded. The Hub offers the download button.
    Missing,
    Downloading { received: u64, total: u64 },
    /// The download or the load after it failed. `message` says why, in words
    /// the reader can act on. The Hub offers the button again.
    Failed { message: String },
}

static ENGINE: OnceLock<Arc<whimpr_asr::WhisperEngine>> = OnceLock::new();
static STATE: Mutex<ModelState> = Mutex::new(ModelState::Loading);
static BUNDLED_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The per-user models folder. It survives app updates and reinstalls.
pub fn models_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("APPDATA").unwrap_or_default();
        PathBuf::from(base).join("WhimprFlow").join("models")
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join("Library/Application Support/WhimprFlow/models")
    }
}

/// Every model file on disk, in the order to try them: the reader's own
/// models (best first), then the copy bundled in the app. A file that fails
/// to load is skipped, so one damaged download can't block a good fallback.
fn candidates_in(user_dir: &Path, bundled_dir: Option<&Path>) -> Vec<(PathBuf, bool)> {
    let mut out: Vec<(PathBuf, bool)> = CANDIDATES
        .iter()
        .map(|name| user_dir.join(name))
        .filter(|p| p.is_file())
        .map(|p| (p, false))
        .collect();
    if let Some(dir) = bundled_dir {
        let p = dir.join(DEFAULT_FILE);
        if p.is_file() {
            out.push((p, true));
        }
    }
    out
}

fn bundled_dir() -> Option<PathBuf> {
    BUNDLED_DIR.get().cloned().flatten()
}

/// The loaded engine, or `None` while loading or when no model is available.
pub fn engine() -> Option<Arc<whimpr_asr::WhisperEngine>> {
    ENGINE.get().cloned()
}

fn set_state(app: &AppHandle, next: ModelState) {
    *STATE.lock().unwrap() = next.clone();
    let _ = app.emit(EVENT, next);
}

/// Find and load a model off the main thread. Called once from the platform
/// layer's `install`.
pub fn start(app: &AppHandle) {
    let bundled = app.path().resource_dir().ok().map(|d| d.join("models"));
    let _ = BUNDLED_DIR.set(bundled);
    let app = app.clone();
    std::thread::spawn(move || {
        let found = candidates_in(&models_dir(), bundled_dir().as_deref());
        for (path, bundled) in &found {
            if load(&app, path, *bundled) {
                return;
            }
        }
        eprintln!(
            "[whimpr] no usable speech model in {} or the app bundle",
            models_dir().display()
        );
        set_state(&app, ModelState::Missing);
    });
}

fn load(app: &AppHandle, path: &Path, bundled: bool) -> bool {
    match whimpr_asr::WhisperEngine::load(path) {
        Ok(engine) => {
            let _ = ENGINE.set(Arc::new(engine));
            let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            eprintln!("[whimpr] ASR model loaded ({}) — ready to transcribe", path.display());
            set_state(app, ModelState::Ready { file, bundled });
            crate::diag::clear_if(whimpr_core::InjectionFailure::AsrUnavailable);
            true
        }
        Err(e) => {
            eprintln!("[whimpr] ASR model at {} failed to load: {e}", path.display());
            false
        }
    }
}

/// A dictation finished with no engine to transcribe it. Say why, and when
/// the fix is the download, bring the Hub forward with the download popup.
pub fn report_unavailable(app: &AppHandle) {
    let state = STATE.lock().unwrap().clone();
    match state {
        // Right after launch: the model is still loading. Not an error.
        ModelState::Loading | ModelState::Ready { .. } => {}
        ModelState::Downloading { received, total } => {
            crate::diag::report_text(
                app,
                "Speech model still downloading".into(),
                format!(
                    "{} of {} downloaded. Dictation starts to work when the download is done.",
                    megabytes(received),
                    megabytes(total)
                ),
            );
        }
        ModelState::Missing | ModelState::Failed { .. } => {
            crate::diag::report(app, whimpr_core::InjectionFailure::AsrUnavailable);
            if let Some(hub) = app.get_webview_window(HUB_LABEL) {
                let _ = hub.show();
                let _ = hub.unminimize();
                let _ = hub.set_focus();
            }
            let _ = app.emit(PROMPT_EVENT, ());
        }
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{} MB", (bytes + 500_000) / 1_000_000)
}

#[tauri::command]
pub fn asr_model_status() -> ModelState {
    STATE.lock().unwrap().clone()
}

/// Download the default model, verify it, and load it. Returns at once; the
/// Hub follows along on `whimpr://asr-model`. A second click while a download
/// runs (or after a model loaded) does nothing.
#[tauri::command]
pub fn download_asr_model(app: AppHandle) {
    {
        let mut state = STATE.lock().unwrap();
        if !matches!(*state, ModelState::Missing | ModelState::Failed { .. }) {
            return;
        }
        *state = ModelState::Downloading { received: 0, total: DEFAULT_BYTES };
    }
    let _ = app.emit(EVENT, ModelState::Downloading { received: 0, total: DEFAULT_BYTES });

    std::thread::spawn(move || {
        let dir = models_dir();
        let part = dir.join(format!("{DEFAULT_FILE}.part"));
        let result = fetch(&dir, &part, |received, total| {
            set_state(&app, ModelState::Downloading { received, total })
        });
        if result.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        match result {
            Ok(path) => {
                if !load(&app, &path, false) {
                    set_state(
                        &app,
                        ModelState::Failed {
                            message: "The model downloaded, but it would not load. Try again."
                                .into(),
                        },
                    );
                }
            }
            Err(message) => {
                eprintln!("[whimpr] speech model download failed: {message}");
                set_state(&app, ModelState::Failed { message });
            }
        }
    });
}

/// Download into `part`, check size and SHA-256, then move it into place as
/// `dir/ggml-base.en.bin`. `progress` gets (received, total) about 5 times a
/// second and once at the end.
fn fetch(dir: &Path, part: &Path, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;

    // The blocking client applies this timeout to each read, not to the whole
    // body, so a slow 148 MB download is fine but a stalled one ends.
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("WhimprFlow/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client
        .get(DEFAULT_URL)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| network_message(&e))?;
    let total = resp.content_length().unwrap_or(DEFAULT_BYTES);

    let mut file = std::fs::File::create(part).map_err(|e| save_message(&e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut received: u64 = 0;
    let mut last_emit = Instant::now();
    loop {
        let n = resp.read(&mut buf).map_err(|_| {
            "The download stopped part way. Check your internet connection, then try again."
                .to_string()
        })?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| save_message(&e))?;
        hasher.update(&buf[..n]);
        received += n as u64;
        if last_emit.elapsed() >= Duration::from_millis(200) {
            progress(received, total);
            last_emit = Instant::now();
        }
    }
    file.sync_all().map_err(|e| save_message(&e))?;
    drop(file);
    progress(received, total);

    let digest = to_hex(&hasher.finalize());
    if received != DEFAULT_BYTES || digest != DEFAULT_SHA256 {
        return Err("The downloaded file was damaged (checksum mismatch). Try again.".into());
    }
    let dest = dir.join(DEFAULT_FILE);
    std::fs::rename(part, &dest).map_err(|e| save_message(&e))?;
    Ok(dest)
}

fn network_message(e: &reqwest::Error) -> String {
    match e.status() {
        Some(status) => format!("The download server answered {status}. Try again later."),
        None => "WhimprFlow could not reach huggingface.co. Check your internet connection, \
                 then try again."
            .into(),
    }
}

fn save_message(e: &std::io::Error) -> String {
    format!("Could not save the model to {}: {e}", models_dir().display())
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("whimpr-asr-model-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn no_model_anywhere_means_no_candidates() {
        let user = scratch("none-user");
        let bundled = scratch("none-bundled");
        assert!(candidates_in(&user, Some(&bundled)).is_empty());
        assert!(candidates_in(&user, None).is_empty());
    }

    #[test]
    fn the_bundled_copy_is_used_when_the_user_has_none() {
        let user = scratch("bundled-user");
        let bundled = scratch("bundled-app");
        std::fs::write(bundled.join(DEFAULT_FILE), b"x").unwrap();
        let found = candidates_in(&user, Some(&bundled));
        assert_eq!(found, vec![(bundled.join(DEFAULT_FILE), true)]);
    }

    #[test]
    fn a_model_the_user_added_wins_over_the_bundled_copy() {
        let user = scratch("pref-user");
        let bundled = scratch("pref-app");
        std::fs::write(bundled.join(DEFAULT_FILE), b"x").unwrap();
        std::fs::write(user.join("ggml-small.en.bin"), b"x").unwrap();
        std::fs::write(user.join(DEFAULT_FILE), b"x").unwrap();
        let found = candidates_in(&user, Some(&bundled));
        assert_eq!(
            found,
            vec![
                (user.join("ggml-small.en.bin"), false),
                (user.join(DEFAULT_FILE), false),
                (bundled.join(DEFAULT_FILE), true),
            ]
        );
    }

    #[test]
    fn a_leftover_partial_download_is_not_a_candidate() {
        let user = scratch("part-user");
        std::fs::write(user.join(format!("{DEFAULT_FILE}.part")), b"x").unwrap();
        assert!(candidates_in(&user, None).is_empty());
    }

    #[test]
    fn state_serializes_as_a_tagged_union_for_the_hub() {
        let v = serde_json::to_value(ModelState::Downloading { received: 5, total: 10 }).unwrap();
        assert_eq!(v, serde_json::json!({"state": "downloading", "received": 5, "total": 10}));
        let v = serde_json::to_value(ModelState::Missing).unwrap();
        assert_eq!(v, serde_json::json!({"state": "missing"}));
        let v = serde_json::to_value(ModelState::Ready { file: DEFAULT_FILE.into(), bundled: true })
            .unwrap();
        assert_eq!(v, serde_json::json!({"state": "ready", "file": DEFAULT_FILE, "bundled": true}));
    }

    #[test]
    fn hex_matches_the_pinned_checksum_format() {
        assert_eq!(to_hex(&[0x00, 0xab, 0x0f]), "00ab0f");
        assert_eq!(DEFAULT_SHA256.len(), 64);
        assert_eq!(to_hex(&Sha256::digest(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    /// The real download, end to end: 148 MB from huggingface.co, checksum,
    /// rename, and a Whisper load of the result. Network, so opt-in:
    /// `cargo test --release -p whimpr-tauri real_download -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_download_verifies_and_loads() {
        let dir = scratch("real-download");
        let part = dir.join(format!("{DEFAULT_FILE}.part"));
        let mut calls = 0u32;
        let mut last = (0u64, 0u64);
        let path = fetch(&dir, &part, |r, t| {
            calls += 1;
            last = (r, t);
        })
        .expect("download");
        assert_eq!(path, dir.join(DEFAULT_FILE));
        assert!(!part.exists(), "the .part file is renamed away");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), DEFAULT_BYTES);
        assert_eq!(last, (DEFAULT_BYTES, DEFAULT_BYTES));
        assert!(calls > 2, "progress was reported during the download ({calls} calls)");
        whimpr_asr::WhisperEngine::load(&path).expect("the downloaded model loads");
        eprintln!("downloaded, verified, loaded; {calls} progress calls");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn megabytes_rounds_to_the_nearest_whole_mb() {
        assert_eq!(megabytes(DEFAULT_BYTES), "148 MB");
        assert_eq!(megabytes(0), "0 MB");
    }
}
