//! The ASR seam. Per-OS backends implement [`AsrEngine`]: FluidAudio on the Apple
//! Neural Engine (macOS primary) and ONNX-Runtime Parakeet (Windows primary, macOS
//! fallback). The core only ever sees this trait; the engine choice is made behind it.

use serde::{Deserialize, Serialize};

/// Identifies which backend produced a transcript (for diagnostics + UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsrEngineId {
    FluidAudioAne,
    OnnxParakeet,
    WhisperCpp,
    /// A cloud speech-to-text API (OpenAI-compatible `/audio/transcriptions`,
    /// e.g. OpenAI itself or Groq's Whisper endpoint).
    CloudWhisper,
}

/// A finalized transcription result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    pub confidence: Option<f32>,
}

/// Static capabilities of an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsrCaps {
    pub supports_streaming: bool,
}

/// Batch, finalize-on-release speech recognition over 16 kHz mono f32 samples in
/// [-1, 1]. Push-to-talk endpoints on key release, so a batch API is sufficient;
/// streaming preview is an optional per-engine capability.
pub trait AsrEngine: Send + Sync {
    fn id(&self) -> AsrEngineId;

    fn caps(&self) -> AsrCaps {
        AsrCaps {
            supports_streaming: false,
        }
    }

    /// Load the model and run a throwaway inference so the first real call is warm.
    fn warmup(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Transcribe one complete utterance.
    fn transcribe(&self, pcm16k: &[f32]) -> anyhow::Result<Transcript>;
}
