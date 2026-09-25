//! Spawns and talks to the local-LLM cleanup worker (a separate process, so
//! llama.cpp and whisper.cpp never link into the same binary). One JSON request
//! per line over stdio: `{system,user}` -> `{text}`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub struct LocalWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl LocalWorker {
    pub fn spawn(worker_bin: &Path, model: &Path) -> anyhow::Result<Self> {
        let mut child = Command::new(worker_bin)
            .arg(model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("no stdin"))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow::anyhow!("no stdout"))?);
        Ok(Self { child, stdin, stdout })
    }

    /// Send one cleanup request (system prompt + few-shot turns + transcript) and
    /// read the response (blocks until the line comes).
    pub fn cleanup(
        &mut self,
        messages: &[whimpr_core::cleanup::CleanupMsg],
    ) -> anyhow::Result<String> {
        // Size the output budget to the transcript, so a long dictation is not
        // truncated mid-sentence with its last words dropped (Publik Test 2:
        // "sometimes the last few words I say are cut off … because of the
        // cleanup"). The cleaned text is about as long as what was said, and the
        // real transcript is the LAST message (the few-shot turns come before
        // it); ~4 chars/token, doubled for reformatting headroom, floored at 400
        // so a short dictation is unchanged and fast.
        let transcript_chars = messages.last().map(|m| m.content.chars().count()).unwrap_or(0);
        let max_tokens = (transcript_chars / 2).max(400);
        let req = serde_json::json!({ "messages": messages, "max_tokens": max_tokens });
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()?;

        let mut resp = String::new();
        if self.stdout.read_line(&mut resp)? == 0 {
            anyhow::bail!("local worker closed");
        }
        let v: serde_json::Value = serde_json::from_str(&resp)?;
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            anyhow::bail!("local llm: {err}");
        }
        Ok(v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string())
    }
}

impl Drop for LocalWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Platform application-support dir:
/// - Linux: `$XDG_DATA_HOME/WhimprFlow` or `~/.local/share/WhimprFlow`
/// - macOS: `~/Library/Application Support/WhimprFlow`
/// - Windows: `%APPDATA%\WhimprFlow`
fn app_support_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        if let Ok(d) = std::env::var("XDG_DATA_HOME") {
            if !d.is_empty() {
                return PathBuf::from(d).join("WhimprFlow");
            }
        }
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(".local/share/WhimprFlow")
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("APPDATA").unwrap_or_default();
        PathBuf::from(base).join("WhimprFlow")
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join("Library/Application Support/WhimprFlow")
    }
}

/// Find the worker binary: next to the app executable (bundled), else the dev build dir.
pub fn worker_bin_path() -> Option<PathBuf> {
    let exe_name = if cfg!(target_os = "windows") {
        "whimpr-llm-worker.exe"
    } else {
        "whimpr-llm-worker"
    };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(exe_name);
            if cand.exists() {
                return Some(cand);
            }
        }
    }
    // Dev fallback.
    #[cfg(target_os = "windows")]
    {
        let dev = std::env::current_dir()
            .unwrap_or_default()
            .join("target/release")
            .join(exe_name);
        return dev.exists().then_some(dev);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let dev = PathBuf::from(home).join("WhimprFlow/target/release/whimpr-llm-worker");
        dev.exists().then_some(dev)
    }
}

/// The local cleanup model path (same models dir as whisper/ASR). Prefer the
/// larger, much more capable Qwen3-4B if present (far better at
/// self-corrections and structure than the 1.5B); fall back to the 1.5B otherwise.
pub fn model_path() -> PathBuf {
    let dir = app_support_dir().join("models");
    for name in [
        "qwen3-4b-instruct-2507-q4_k_m.gguf",
        "qwen2.5-1.5b-instruct-q4_k_m.gguf",
    ] {
        let p = dir.join(name);
        if p.exists() {
            return p;
        }
    }
    dir.join("qwen2.5-1.5b-instruct-q4_k_m.gguf")
}

/// Spawn the worker if both the binary and the model are present.
pub fn spawn_default() -> Option<LocalWorker> {
    let bin = worker_bin_path()?;
    let model = model_path();
    if !model.exists() {
        eprintln!("[whimpr] local model not found at {}", model.display());
        return None;
    }
    match LocalWorker::spawn(&bin, &model) {
        Ok(w) => {
            eprintln!("[whimpr] local LLM worker started ({})", bin.display());
            Some(w)
        }
        Err(e) => {
            eprintln!("[whimpr] local LLM worker failed to start: {e}");
            None
        }
    }
}
