//! Local speech-to-text via whisper.cpp (whisper-rs), implementing
//! [`whimpr_core::AsrEngine`]. Expects 16 kHz mono f32 samples.

use std::path::Path;
use std::sync::RwLock;

use whimpr_core::asr::{AsrCaps, AsrEngine, AsrEngineId, Transcript};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// A loaded whisper model ready to transcribe utterances.
pub struct WhisperEngine {
    ctx: WhisperContext,
    /// Whisper language code, or "auto" to let the model detect it. Held behind a
    /// lock so it can be changed from the Hub without reloading the model, which
    /// takes about a second.
    language: RwLock<String>,
    /// Comma-separated vocabulary fed to Whisper as an `initial_prompt`, which
    /// biases decoding towards these spellings. Until now the user's Dictionary
    /// only reached the *cleanup* stage, so a mis-heard name had to be repaired
    /// after the fact instead of being heard correctly in the first place.
    vocabulary: RwLock<String>,
    /// Beam search instead of greedy. Measurably better in noise, and slower.
    beam_search: RwLock<bool>,
}

impl WhisperEngine {
    /// Load a GGML/GGUF whisper model from `model_path`.
    pub fn load(model_path: &Path) -> anyhow::Result<Self> {
        let path = model_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("model path is not valid UTF-8"))?;
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| anyhow::anyhow!("failed to load whisper model: {e}"))?;
        Ok(Self {
            ctx,
            language: RwLock::new("en".to_string()),
            vocabulary: RwLock::new(String::new()),
            beam_search: RwLock::new(false),
        })
    }

    /// Bias decoding towards these spellings (names, jargon, acronyms).
    pub fn set_vocabulary(&self, terms: &[String]) {
        // Whisper's initial_prompt is plain text and is capped by the model's
        // context, so keep it short — a comma list of the words themselves is
        // what the upstream project recommends for this.
        // Collects &str borrowed from `terms`; joining produces the owned String.
        let joined = terms
            .iter()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .take(60)
            .collect::<Vec<&str>>()
            .join(", ");
        if let Ok(mut g) = self.vocabulary.write() {
            *g = joined;
        }
    }

    /// Trade latency for accuracy in noisy input.
    pub fn set_beam_search(&self, on: bool) {
        if let Ok(mut g) = self.beam_search.write() {
            *g = on;
        }
    }

    /// Set the spoken language. Only has an effect with a multilingual model —
    /// the `.en` builds ignore it and always transcribe English.
    pub fn set_language(&self, language: &str) {
        if let Ok(mut g) = self.language.write() {
            *g = language.to_string();
        }
    }
}

impl AsrEngine for WhisperEngine {
    fn id(&self) -> AsrEngineId {
        AsrEngineId::WhisperCpp
    }

    fn caps(&self) -> AsrCaps {
        AsrCaps {
            supports_streaming: false,
        }
    }

    fn transcribe(&self, pcm16k: &[f32]) -> anyhow::Result<Transcript> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow::anyhow!("whisper create_state: {e}"))?;

        // Bound before `params` so the borrow passed to set_language outlives it.
        let language = self
            .language
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "en".to_string());

        let vocabulary = self
            .vocabulary
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        let beam = self.beam_search.read().map(|g| *g).unwrap_or(false);

        // Beam search explores several hypotheses instead of committing to the
        // highest-probability token each step. In clean audio the two agree; in
        // noise, where the top token is often wrong, it recovers materially more.
        // It costs latency, hence the setting.
        let mut params = if beam {
            FullParams::new(SamplingStrategy::BeamSearch {
                beam_size: 5,
                patience: -1.0,
            })
        } else {
            FullParams::new(SamplingStrategy::Greedy { best_of: 1 })
        };
        // `None` puts whisper into auto-detect.
        if language == "auto" {
            params.set_language(None);
        } else {
            params.set_language(Some(&language));
        }
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        // Push-to-talk utterances are always one short clip, not long-form audio.
        // Without this, whisper.cpp can split it into multiple internal segments
        // that repeat the same words — which then get concatenated below,
        // producing the sentence twice. Single-segment mode avoids that.
        params.set_single_segment(true);
        params.set_no_context(true);

        // ── Noise robustness ─────────────────────────────────────────────────
        // Whisper's own fallback mechanism: decode greedily at temperature 0,
        // and if the result looks degenerate (low average logprob, or high
        // token entropy — both signatures of the model guessing at noise), retry
        // at successively higher temperatures. Without an increment there is no
        // fallback at all and a bad first pass is simply accepted.
        params.set_temperature(0.0);
        params.set_temperature_inc(0.2);
        params.set_entropy_thold(2.4);
        params.set_logprob_thold(-1.0);
        // How confident the model must be that a segment is NOT speech before it
        // is dropped. This is what suppresses background chatter and room noise
        // in the gaps rather than transcribing it as words.
        params.set_no_speech_thold(0.6);

        if !vocabulary.is_empty() {
            params.set_initial_prompt(&vocabulary);
        }

        state
            .full(params, pcm16k)
            .map_err(|e| anyhow::anyhow!("whisper full: {e}"))?;

        let n = state
            .full_n_segments()
            .map_err(|e| anyhow::anyhow!("whisper n_segments: {e}"))?;
        let mut text = String::new();
        for i in 0..n {
            if let Ok(seg) = state.full_get_segment_text(i) {
                text.push_str(&seg);
            }
        }

        Ok(Transcript {
            text: text.trim().to_string(),
            confidence: None,
        })
    }
}
