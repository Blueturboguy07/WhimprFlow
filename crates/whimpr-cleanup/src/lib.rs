//! Cloud cleanup providers. The OpenAI provider (default cloud) sends the shared
//! WhimprFlow system prompt plus the assembled context and returns cleaned text.
//! On any failure the caller falls back to the raw transcript — cleanup is an
//! enhancement, never a gate.

use std::time::Duration;

use whimpr_core::cleanup::{build_messages, CleanupContext, CleanupProvider, ProviderId};

pub mod publik;
pub use publik::{BalanceSnapshot, Provisioned, PublikError, PublikProvider, Wallet};

/// Default OpenAI Chat Completions endpoint.
const OPENAI_DEFAULT_URL: &str = "https://api.openai.com/v1/chat/completions";

/// Output token budget for a cleanup, scaled to the transcript so a long
/// dictation's cleaned text is not truncated with its last words dropped
/// (Publik Test 2: "sometimes the last few words I say are cut off … because of
/// the cleanup"). The cleaned text is about as long as what was said; ~4
/// chars/token, doubled for reformatting headroom, and floored so a short
/// dictation keeps the generous fixed cap it always had.
pub(crate) fn cleanup_max_tokens(raw: &str, floor: usize) -> usize {
    (raw.chars().count() / 2).max(floor)
}

/// Cleanup via the OpenAI Chat Completions API — or any OpenAI-compatible
/// endpoint (OpenRouter, a local server, etc.) when `base_url` is set.
/// OpenRouter in particular speaks this exact wire format at
/// `https://openrouter.ai/api/v1/chat/completions`.
pub struct OpenAiProvider {
    client: reqwest::blocking::Client,
    api_key: String,
    model: String,
    /// Full chat-completions URL. Defaults to OpenAI's when empty.
    url: String,
}

impl OpenAiProvider {
    pub fn new(api_key: String, model: impl Into<String>) -> Self {
        Self::with_base_url(api_key, model, None)
    }

    /// `base_url` is the API root (e.g. `https://openrouter.ai/api/v1`), without
    /// the `/chat/completions` suffix. `None` or empty uses OpenAI directly.
    pub fn with_base_url(
        api_key: String,
        model: impl Into<String>,
        base_url: Option<String>,
    ) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client");
        let url = match base_url.map(|s| s.trim().trim_end_matches('/').to_string()) {
            Some(base) if !base.is_empty() => format!("{base}/chat/completions"),
            _ => OPENAI_DEFAULT_URL.to_string(),
        };
        Self {
            client,
            api_key,
            model: model.into(),
            url,
        }
    }
}

impl CleanupProvider for OpenAiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenAi
    }

    fn cleanup(&self, raw: &str, ctx: &CleanupContext) -> anyhow::Result<String> {
        // System prompt + few-shot demonstration turns + the real transcript.
        let messages: Vec<serde_json::Value> = build_messages(raw, ctx)
            .into_iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect();
        let body = serde_json::json!({
            "model": self.model,
            "temperature": 0.2,
            "max_tokens": cleanup_max_tokens(raw, 512),
            "messages": messages,
        });

        let resp = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()?;

        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().unwrap_or_default();
            anyhow::bail!("OpenAI HTTP {status}: {detail}");
        }

        let v: serde_json::Value = resp.json()?;
        let text = v["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            anyhow::bail!("OpenAI returned empty content");
        }
        Ok(text)
    }
}

/// Cleanup via the Anthropic Messages API. Same shared system prompt; the only
/// difference from OpenAI is the wire envelope (top-level `system`, `x-api-key`).
pub struct AnthropicProvider {
    client: reqwest::blocking::Client,
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: impl Into<String>) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("failed to build HTTP client");
        Self {
            client,
            api_key,
            model: model.into(),
        }
    }
}

impl CleanupProvider for AnthropicProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Anthropic
    }

    fn cleanup(&self, raw: &str, ctx: &CleanupContext) -> anyhow::Result<String> {
        // Anthropic takes the system prompt top-level; the few-shot turns and the
        // real transcript go in `messages` as user/assistant turns.
        let mut system = String::new();
        let mut messages: Vec<serde_json::Value> = Vec::new();
        for m in build_messages(raw, ctx) {
            if m.role == "system" {
                system = m.content;
            } else {
                messages.push(serde_json::json!({ "role": m.role, "content": m.content }));
            }
        }
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": cleanup_max_tokens(raw, 512),
            "temperature": 0.2,
            "system": system,
            "messages": messages,
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()?;

        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().unwrap_or_default();
            anyhow::bail!("Anthropic HTTP {status}: {detail}");
        }

        let v: serde_json::Value = resp.json()?;
        let text = v["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            anyhow::bail!("Anthropic returned empty content");
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::cleanup_max_tokens;

    #[test]
    fn short_dictation_keeps_the_floor() {
        // A short utterance stays at the generous fixed cap — behaviour unchanged.
        assert_eq!(cleanup_max_tokens("hey, quick note", 512), 512);
        assert_eq!(cleanup_max_tokens("", 512), 512);
    }

    #[test]
    fn long_dictation_scales_above_the_floor_so_the_tail_is_not_cut() {
        // ~2400 chars ≈ 600 tokens of speech; the old fixed 512 cap would truncate
        // the cleaned text and drop the last words. The budget now scales with it.
        let long = "word ".repeat(480); // 2400 chars
        let budget = cleanup_max_tokens(&long, 512);
        assert!(budget > 512, "a long dictation must get more than the floor, got {budget}");
        assert_eq!(budget, long.chars().count() / 2);
    }
}
