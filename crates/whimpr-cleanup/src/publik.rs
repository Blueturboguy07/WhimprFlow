//! publik API cleanup provider + the install-time provisioning and wallet calls.
//!
//! Wire format is OpenAI Chat Completions (the gateway's primary dialect), so
//! the request body is byte-identical to `OpenAiProvider`'s (a test proves it).
//! What differs: the response headers carry the charge and the remaining
//! balance, and a 402 / 401 / 429 is a *typed* outcome the shell turns into a
//! banner or a re-provision — not just "cleanup failed, pasting raw".
//!
//! Everything here is plain `reqwest::blocking` + `serde_json`; no new crates.
//! The contract is `~/publik-api-research/CONTRACT.md` §1, §3.2, §5.

use std::sync::Mutex;
use std::time::Duration;

use whimpr_core::cleanup::{build_messages, CleanupContext, CleanupProvider, ProviderId};

/// Compile-time default API root. `POST /installs` hands back `base_url` and
/// the shell persists it; `PUBLIK_API_BASE_URL` in the environment overrides
/// both (dev/staging). See [`resolve_base_url`].
pub const DEFAULT_BASE_URL: &str = "https://publikhq.com/api/v1";
/// The alias the gateway maps to its current fast tier. Never an upstream slug.
pub const DEFAULT_MODEL_ALIAS: &str = "publik-fast";
/// Where the weekly usage bars live. Opened from the Settings card.
pub const DASHBOARD_URL: &str = "https://publikhq.com/dashboard/api";
/// The publik API terms, linked from the disclosure card.
pub const TERMS_URL: &str = "https://publikhq.com/terms#api";
/// This app's slug in the publik catalog. Attribution is bound to the key
/// server-side; this is what `POST /installs` asks for as `app_slug`.
pub const APP_SLUG: &str = "whimprflow";
/// Version of the in-app disclosure copy (mirrors whimpr-core's constant).
pub const DISCLOSURE_VERSION: u32 = whimpr_core::PUBLIK_DISCLOSURE_VERSION;

fn trim_url(s: &str) -> Option<String> {
    let t = s.trim().trim_end_matches('/');
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Resolution order for the API root: `PUBLIK_API_BASE_URL` env (dev/staging)
/// → the `base_url` the provisioning response handed back (persisted in
/// settings) → the compiled default. Trailing slashes are dropped.
pub fn resolve_base_url(from_provisioning: &str) -> String {
    if let Some(u) = std::env::var("PUBLIK_API_BASE_URL").ok().and_then(|s| trim_url(&s)) {
        return u;
    }
    if let Some(u) = trim_url(from_provisioning) {
        return u;
    }
    DEFAULT_BASE_URL.to_string()
}

/// What the gateway stamps on every metered call (CONTRACT §1 headers).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BalanceSnapshot {
    /// `x-publik-balance` — available micros after admission.
    pub balance_micros: i64,
    /// `x-publik-charge-micros` — non-stream only, which is all we do.
    pub last_charge_micros: Option<i64>,
    /// `x-publik-model` — the served slug.
    pub model_served: Option<String>,
    /// `x-publik-claim-state` — `anonymous` | `claimed`.
    pub claim_state: Option<String>,
    pub week_used_micros: Option<i64>,
    /// `x-publik-week-budget` — micros, or `None` when the header says `none`.
    pub week_budget_micros: Option<i64>,
    pub week_resets_at: Option<String>,
    /// `x-publik-starter-remaining` — present only while > 0.
    pub starter_remaining_micros: Option<i64>,
}

/// Typed outcomes of a gateway call. `Http`/`Transport` are "something else
/// broke" and the pipeline just pastes raw, as it always did.
#[derive(Debug, thiserror::Error)]
pub enum PublikError {
    /// `402 insufficient_credit` or `402 model_requires_claim`. The body carries
    /// the message to show and exactly one link to render: `top_up_url`.
    #[error("publik API needs credit: {message}")]
    InsufficientCredit {
        /// `insufficient_credit` | `model_requires_claim`.
        kind: String,
        message: String,
        available_micros: i64,
        /// The one link an app renders (claim link while anonymous, add-credit
        /// link once claimed). The shell falls back to the claim URL from
        /// provisioning when the gateway omits it.
        top_up_url: Option<String>,
        claim_state: Option<String>,
    },
    /// `401 invalid_api_key` — missing / malformed / unknown. The shell
    /// forgets the key; the next "Turn on" mints again.
    #[error("publik API key rejected")]
    InvalidKey,
    /// `401 key_revoked`. `reprovision: true` only comes from the idle sweep —
    /// the app may silently re-run `POST /installs` with its existing
    /// `install_id`. `false` means the user revoked it (dashboard / uninstall
    /// hook): show "disconnected" and never re-mint on our own.
    #[error("publik API key revoked (reprovision={reprovision})")]
    KeyRevoked { reprovision: bool },
    /// `429 rate_limit_exceeded | daily_cap_reached | week_budget_reached`.
    #[error("publik API rate limited ({kind})")]
    RateLimited {
        kind: String,
        retry_after_secs: Option<u64>,
    },
    /// `503 gateway_unavailable` — nothing was charged; try again later.
    #[error("publik API is unavailable right now")]
    Unavailable { retry_after_secs: Option<u64> },
    #[error("publik API HTTP {status}: {detail}")]
    Http { status: u16, detail: String },
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
}

fn http(timeout: Duration) -> Result<reqwest::blocking::Client, PublikError> {
    Ok(reqwest::blocking::Client::builder().timeout(timeout).build()?)
}

fn header_str(h: &reqwest::header::HeaderMap, k: &str) -> Option<String> {
    h.get(k).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}
fn header_i64(h: &reqwest::header::HeaderMap, k: &str) -> Option<i64> {
    header_str(h, k).and_then(|s| s.parse::<i64>().ok())
}
fn retry_after(h: &reqwest::header::HeaderMap) -> Option<u64> {
    header_str(h, "retry-after").and_then(|s| s.parse::<u64>().ok())
}

/// The `x-publik-*` headers → a snapshot, or `None` when there is no balance
/// header at all (a non-gateway answer). `x-publik-balance-micros` is accepted
/// as the one-release alias of `x-publik-balance` (R21 §2.4, R25 N3).
pub fn parse_balance_headers(h: &reqwest::header::HeaderMap) -> Option<BalanceSnapshot> {
    let balance_micros = header_i64(h, "x-publik-balance").or_else(|| header_i64(h, "x-publik-balance-micros"))?;
    Some(BalanceSnapshot {
        balance_micros,
        last_charge_micros: header_i64(h, "x-publik-charge-micros"),
        model_served: header_str(h, "x-publik-model"),
        claim_state: header_str(h, "x-publik-claim-state"),
        week_used_micros: header_i64(h, "x-publik-week-used"),
        week_budget_micros: header_i64(h, "x-publik-week-budget"),
        week_resets_at: header_str(h, "x-publik-week-resets-at"),
        starter_remaining_micros: header_i64(h, "x-publik-starter-remaining"),
    })
}

/// The gateway's error envelope is `{"error":{"type":…,"message":…,…}}`; the
/// prototype put the extras at the top level. Read both (R23 §2.3.2 note).
fn err_field<'a>(v: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    let nested = &v["error"][key];
    if nested.is_null() {
        &v[key]
    } else {
        nested
    }
}

/// Map a non-2xx gateway answer to a typed error (CONTRACT §1 status codes).
pub fn map_error(status: u16, headers: &reqwest::header::HeaderMap, body: &str) -> PublikError {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let kind = err_field(&v, "type").as_str().unwrap_or("").to_string();
    match status {
        402 => PublikError::InsufficientCredit {
            kind: if kind.is_empty() { "insufficient_credit".to_string() } else { kind },
            message: err_field(&v, "message")
                .as_str()
                .filter(|m| !m.trim().is_empty())
                .unwrap_or("Not enough publik credit for this request.")
                .to_string(),
            available_micros: err_field(&v, "available_micros").as_i64().unwrap_or(0),
            top_up_url: err_field(&v, "top_up_url").as_str().map(str::to_string),
            claim_state: err_field(&v, "claim_state").as_str().map(str::to_string),
        },
        401 | 403 if kind == "key_revoked" => PublikError::KeyRevoked {
            reprovision: err_field(&v, "reprovision").as_bool().unwrap_or(false),
        },
        401 => PublikError::InvalidKey,
        429 => PublikError::RateLimited {
            kind: if kind.is_empty() { "rate_limit_exceeded".to_string() } else { kind },
            retry_after_secs: retry_after(headers),
        },
        503 => PublikError::Unavailable { retry_after_secs: retry_after(headers) },
        _ => PublikError::Http {
            status,
            detail: if body.len() > 400 { body[..400].to_string() } else { body.to_string() },
        },
    }
}

/// The Chat Completions body — shared shape with `OpenAiProvider` (a test pins
/// them byte-identical so the gateway sees exactly what OpenAI would).
pub fn chat_body(model: &str, raw: &str, ctx: &CleanupContext) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = build_messages(raw, ctx)
        .into_iter()
        .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
        .collect();
    serde_json::json!({
        "model": model,
        "temperature": 0.2,
        "max_tokens": super::cleanup_max_tokens(raw, 512),
        "messages": messages,
    })
}

/// Cleanup via the publik API gateway.
pub struct PublikProvider {
    client: reqwest::blocking::Client,
    api_key: String,
    model: String,
    url: String,
    /// Written after every response so the shell can read the balance line
    /// without a second request. `Mutex` because `CleanupProvider: Sync`.
    last: Mutex<Option<BalanceSnapshot>>,
}

impl PublikProvider {
    /// `base_url` is the resolved API root (see [`resolve_base_url`]); `model`
    /// empty means [`DEFAULT_MODEL_ALIAS`].
    pub fn new(api_key: String, base_url: &str, model: &str) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15)) // same budget as OpenAiProvider
            .build()
            .expect("failed to build HTTP client");
        let base = trim_url(base_url).unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let model = model.trim();
        Self {
            client,
            api_key,
            model: if model.is_empty() { DEFAULT_MODEL_ALIAS.to_string() } else { model.to_string() },
            url: format!("{base}/chat/completions"),
            last: Mutex::new(None),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn last_balance(&self) -> Option<BalanceSnapshot> {
        self.last.lock().unwrap().clone()
    }

    /// Typed form of `cleanup` — the trait method wraps this in `anyhow`.
    pub fn cleanup_typed(&self, raw: &str, ctx: &CleanupContext) -> Result<String, PublikError> {
        let body = chat_body(&self.model, raw, ctx);
        let resp = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .header("X-Publik-App", APP_SLUG)
            .json(&body)
            .send()?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        if let Some(s) = parse_balance_headers(&headers) {
            *self.last.lock().unwrap() = Some(s);
        }
        if !(200..300).contains(&status) {
            let text = resp.text().unwrap_or_default();
            return Err(map_error(status, &headers, &text));
        }
        let v: serde_json::Value = resp.json()?;
        let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("").trim().to_string();
        if text.is_empty() {
            return Err(PublikError::Http { status, detail: "empty content".into() });
        }
        Ok(text)
    }
}

impl CleanupProvider for PublikProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Publik
    }

    fn cleanup(&self, raw: &str, ctx: &CleanupContext) -> anyhow::Result<String> {
        self.cleanup_typed(raw, ctx).map_err(anyhow::Error::from)
    }
}

// ── Provisioning: POST /installs (CONTRACT §3.2) ─────────────────────────────

/// What the app sends. The app token is an identifier with abuse limits, not a
/// secret (it ships inside the binary); the server rate-limits this route.
#[derive(Debug, Clone)]
pub struct ProvisionRequest {
    pub app_token: String,
    pub app_version: String,
    /// A v4 UUID minted by the app; the idempotency key of the mint.
    pub install_id: String,
    pub os_version: Option<String>,
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct ProvisionedModels {
    #[serde(default)]
    pub fast: Option<String>,
    #[serde(default)]
    pub balanced: Option<String>,
    #[serde(default)]
    pub smart: Option<String>,
}

/// `201` (fresh mint) or `200` (replay of an existing unrevoked `install_id`,
/// where `key` is `null` and `starter_micros` is 0). Every field the contract
/// may omit is optional so a lenient server never breaks the app.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Provisioned {
    #[serde(default)]
    pub install_id: Option<String>,
    /// `pk_live_<12>_<32>` — appears exactly once, on the 201.
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub key_id: Option<String>,
    /// Honoured over the compiled default (CONTRACT §1, S8).
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub models: Option<ProvisionedModels>,
    #[serde(default)]
    pub claim_code: Option<String>,
    #[serde(default)]
    pub claim_url: Option<String>,
    #[serde(default)]
    pub claim_expires_at: Option<String>,
    #[serde(default)]
    pub starter_micros: Option<i64>,
    /// = starter on a 201 (CONTRACT §3.2 [B1]).
    #[serde(default)]
    pub balance_micros: Option<i64>,
    #[serde(default)]
    pub starting_credit_micros: Option<i64>,
    /// Present on the 200 replay.
    #[serde(default)]
    pub claim_state: Option<String>,
    #[serde(default)]
    pub wallet: Option<serde_json::Value>,
    /// `true` when the server answered 200 (replay) instead of 201.
    #[serde(skip)]
    pub replay: bool,
}

impl Provisioned {
    /// Starting balance: `balance_micros`, else `starting_credit_micros`, else
    /// `starter_micros`, else the wallet's balance, else 0.
    pub fn starting_balance_micros(&self) -> i64 {
        self.balance_micros
            .or(self.starting_credit_micros)
            .or(self.starter_micros)
            .or_else(|| self.wallet.as_ref().and_then(|w| w["balance_micros"].as_i64()))
            .unwrap_or(0)
    }
}

/// `std::env::consts::OS` already spells `macos` / `windows` / `linux`.
pub fn os_name() -> &'static str {
    std::env::consts::OS
}

/// `arm64` / `x64` the way the contract example spells them.
pub fn arch_name() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}

/// One call, idempotent on `install_id`. The token goes both as the bearer and
/// in the body (the contract accepts either). Never called before the user has
/// accepted the disclosure — that is the shell's job (`publik_accept_disclosure`).
pub fn provision(base_url: &str, req: &ProvisionRequest) -> Result<Provisioned, PublikError> {
    let client = http(Duration::from_secs(20))?;
    let mut body = serde_json::json!({
        "app_token": req.app_token,
        "app_slug": APP_SLUG,
        "app_version": req.app_version,
        "os": os_name(),
        "arch": arch_name(),
        "install_id": req.install_id,
        "disclosure_version": DISCLOSURE_VERSION,
        "dialects": ["chat_completions"],
    });
    if let Some(v) = req.os_version.as_deref().filter(|s| !s.trim().is_empty()) {
        body["os_version"] = serde_json::Value::String(v.trim().to_string());
    }
    if let Some(d) = req.device_name.as_deref().filter(|s| !s.trim().is_empty()) {
        // 1–120 chars per the contract's field check.
        let d: String = d.trim().chars().take(120).collect();
        body["device_name"] = serde_json::Value::String(d);
    }
    let base = trim_url(base_url).unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let resp = client
        .post(format!("{base}/installs"))
        .bearer_auth(&req.app_token)
        .header("Idempotency-Key", &req.install_id)
        .json(&body)
        .send()?;
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    if !(200..300).contains(&status) {
        let text = resp.text().unwrap_or_default();
        // An unknown/revoked app token is `401 invalid_app_token`; there is no
        // key to forget, so surface it as a plain HTTP error with the detail.
        return Err(match map_error(status, &headers, &text) {
            PublikError::InvalidKey | PublikError::KeyRevoked { .. } => PublikError::Http { status, detail: text },
            other => other,
        });
    }
    let mut p: Provisioned = resp.json()?;
    p.replay = status == 200 || p.key.is_none();
    Ok(p)
}

// ── Wallet: GET /wallet (CONTRACT §3.2, R21 §2.3) ────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wallet {
    pub balance_micros: i64,
    pub claim_state: Option<String>,
    pub claim_url: Option<String>,
    pub add_credit_url: Option<String>,
    pub top_up_url: Option<String>,
    pub week_used_micros: Option<i64>,
    pub week_budget_micros: Option<i64>,
    pub week_resets_at: Option<String>,
    pub starter_remaining_micros: Option<i64>,
    pub daily_cap_micros: Option<i64>,
    pub spent_today_micros: Option<i64>,
}

/// Parse the `GET /wallet` body (also accepts the `/balance` alias shape).
pub fn parse_wallet(v: &serde_json::Value) -> Option<Wallet> {
    let balance_micros = v["balance_micros"].as_i64().or_else(|| v["available_micros"].as_i64())?;
    let s = |k: &str| v[k].as_str().map(str::to_string);
    Some(Wallet {
        balance_micros,
        claim_state: s("claim_state"),
        claim_url: s("claim_url"),
        add_credit_url: s("add_credit_url"),
        top_up_url: s("top_up_url"),
        week_used_micros: v["week"]["used_micros"].as_i64(),
        week_budget_micros: v["week"]["budget_micros"].as_i64(),
        week_resets_at: v["week"]["resets_at"].as_str().map(str::to_string),
        starter_remaining_micros: v["starter"]["remaining_micros"].as_i64(),
        daily_cap_micros: v["daily_cap_micros"].as_i64(),
        spent_today_micros: v["spent_today_micros"].as_i64(),
    })
}

/// The balance line's source of truth when there is no fresh header (Settings
/// pane opened, disclosure just accepted, streams reconciled).
pub fn fetch_wallet(base_url: &str, api_key: &str) -> Result<Wallet, PublikError> {
    let client = http(Duration::from_secs(10))?;
    let base = trim_url(base_url).unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let resp = client.get(format!("{base}/wallet")).bearer_auth(api_key).send()?;
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    if !(200..300).contains(&status) {
        let text = resp.text().unwrap_or_default();
        return Err(map_error(status, &headers, &text));
    }
    let v: serde_json::Value = resp.json()?;
    parse_wallet(&v).ok_or(PublikError::Http { status, detail: "wallet body has no balance_micros".into() })
}

/// "$0.0004" / "$1.23" — micro-USD to a short dollar string for the balance line.
pub fn fmt_usd(micros: i64) -> String {
    let d = micros as f64 / 1_000_000.0;
    if d.abs() < 0.01 {
        format!("${d:.4}")
    } else {
        format!("${d:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Env is process-global; every test that touches it takes this lock.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// One captured HTTP request from the fixture server.
    #[derive(Debug, Clone)]
    struct Captured {
        request_line: String,
        headers: Vec<(String, String)>,
        body: String,
    }
    impl Captured {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
        fn json(&self) -> serde_json::Value {
            serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
        }
    }

    /// A dependency-free HTTP fixture: a `TcpListener` on 127.0.0.1:0 that
    /// serves exactly one request with the canned status/headers/body, then
    /// hands the captured request back. No httpmock/wiremock.
    fn fake_server(status: u16, headers: &[(&str, &str)], body: &str) -> (String, std::thread::JoinHandle<Captured>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let headers: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let body = body.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let header_end;
            loop {
                let n = stream.read(&mut tmp).unwrap();
                if n == 0 {
                    panic!("client closed before headers");
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    header_end = i + 4;
                    break;
                }
            }
            let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
            let mut lines = head.split("\r\n");
            let request_line = lines.next().unwrap_or("").to_string();
            let hdrs: Vec<(String, String)> = lines
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .collect();
            let content_length: usize = hdrs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0);
            let mut body_bytes = buf[header_end..].to_vec();
            while body_bytes.len() < content_length {
                let n = stream.read(&mut tmp).unwrap();
                if n == 0 {
                    break;
                }
                body_bytes.extend_from_slice(&tmp[..n]);
            }
            let reason = match status {
                200 => "OK",
                201 => "Created",
                401 => "Unauthorized",
                402 => "Payment Required",
                429 => "Too Many Requests",
                503 => "Service Unavailable",
                _ => "Status",
            };
            let mut resp = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
            for (k, v) in &headers {
                resp.push_str(&format!("{k}: {v}\r\n"));
            }
            resp.push_str("\r\n");
            resp.push_str(&body);
            stream.write_all(resp.as_bytes()).unwrap();
            let _ = stream.flush();
            Captured {
                request_line,
                headers: hdrs,
                body: String::from_utf8_lossy(&body_bytes).to_string(),
            }
        });
        (format!("http://{addr}"), handle)
    }

    fn ctx() -> CleanupContext {
        CleanupContext::default()
    }

    const OK_BODY: &str = r#"{"choices":[{"message":{"role":"assistant","content":"  Hello, world.  "}}]}"#;

    #[test]
    fn parses_balance_headers_per_contract() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-publik-balance", "4870000".parse().unwrap());
        h.insert("x-publik-charge-micros", "412".parse().unwrap());
        h.insert("x-publik-model", "gpt-5.6-luna".parse().unwrap());
        h.insert("x-publik-claim-state", "anonymous".parse().unwrap());
        h.insert("x-publik-week-used", "248760".parse().unwrap());
        h.insert("x-publik-week-budget", "none".parse().unwrap());
        h.insert("x-publik-week-resets-at", "2026-09-25T17:04:11Z".parse().unwrap());
        h.insert("x-publik-starter-remaining", "1240".parse().unwrap());
        let s = parse_balance_headers(&h).unwrap();
        assert_eq!(s.balance_micros, 4_870_000);
        assert_eq!(s.last_charge_micros, Some(412));
        assert_eq!(s.model_served.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(s.claim_state.as_deref(), Some("anonymous"));
        assert_eq!(s.week_used_micros, Some(248_760));
        assert_eq!(s.week_budget_micros, None, "`none` is not a number");
        assert_eq!(s.week_resets_at.as_deref(), Some("2026-09-25T17:04:11Z"));
        assert_eq!(s.starter_remaining_micros, Some(1240));

        // The one-release alias still works; a non-gateway answer is None.
        let mut alias = reqwest::header::HeaderMap::new();
        alias.insert("x-publik-balance-micros", "7".parse().unwrap());
        assert_eq!(parse_balance_headers(&alias).unwrap().balance_micros, 7);
        assert!(parse_balance_headers(&reqwest::header::HeaderMap::new()).is_none());
    }

    #[test]
    fn fmt_usd_short_and_long() {
        assert_eq!(fmt_usd(412), "$0.0004");
        assert_eq!(fmt_usd(4_870_000), "$4.87");
        assert_eq!(fmt_usd(0), "$0.0000");
        assert_eq!(fmt_usd(250_000), "$0.25");
    }

    #[test]
    fn base_url_env_then_provisioning_then_default() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("PUBLIK_API_BASE_URL");
        assert_eq!(resolve_base_url(""), DEFAULT_BASE_URL);
        assert_eq!(resolve_base_url("   "), DEFAULT_BASE_URL);
        // S8: the provisioning response wins over the compiled default.
        assert_eq!(resolve_base_url("https://api.publikhq.com/v1/"), "https://api.publikhq.com/v1");
        // A dev/staging override wins over both.
        std::env::set_var("PUBLIK_API_BASE_URL", "http://127.0.0.1:1/");
        assert_eq!(resolve_base_url("https://api.publikhq.com/v1"), "http://127.0.0.1:1");
        std::env::remove_var("PUBLIK_API_BASE_URL");
    }

    #[test]
    fn request_body_uses_the_alias_and_the_same_shape_as_openai() {
        // Capture what OpenAiProvider sends for this input, untouched.
        let raw = "um so this is a test new line second line";
        let (url, h) = fake_server(200, &[], OK_BODY);
        let openai = super::super::OpenAiProvider::with_base_url("sk-test".into(), "publik-fast", Some(url));
        let _ = openai.cleanup(raw, &ctx());
        let theirs = h.join().unwrap().json();

        let (url, h) = fake_server(200, &[("x-publik-balance", "1")], OK_BODY);
        let p = PublikProvider::new("pk_live_x".into(), &url, "");
        let _ = p.cleanup_typed(raw, &ctx());
        let ours = h.join().unwrap().json();

        assert_eq!(ours["model"], "publik-fast");
        assert_eq!(ours["temperature"], 0.2);
        assert_eq!(ours["max_tokens"], super::super::cleanup_max_tokens(raw, 512));
        assert_eq!(ours, theirs, "the gateway must see byte-for-byte what OpenAI would");
        assert_eq!(ours, chat_body(DEFAULT_MODEL_ALIAS, raw, &ctx()));
    }

    #[test]
    fn cleanup_200_returns_text_and_records_the_balance() {
        let (url, h) = fake_server(
            200,
            &[
                ("x-publik-balance", "249588"),
                ("x-publik-charge-micros", "412"),
                ("x-publik-model", "gpt-5.6-luna"),
                ("x-publik-claim-state", "anonymous"),
            ],
            OK_BODY,
        );
        let p = PublikProvider::new("pk_live_abc".into(), &url, "publik-fast");
        let out = p.cleanup_typed("hello world", &ctx()).unwrap();
        assert_eq!(out, "Hello, world.");
        let req = h.join().unwrap();
        assert!(req.request_line.starts_with("POST /chat/completions "), "{}", req.request_line);
        assert_eq!(req.header("authorization"), Some("Bearer pk_live_abc"));
        assert_eq!(req.header("x-publik-app"), Some("whimprflow"));
        let snap = p.last_balance().unwrap();
        assert_eq!(snap.balance_micros, 249_588);
        assert_eq!(snap.last_charge_micros, Some(412));
        assert_eq!(snap.claim_state.as_deref(), Some("anonymous"));
    }

    const BODY_402: &str = r#"{"error":{"type":"insufficient_credit","message":"Not enough publik credit for this request.",
        "available_micros":1240,"required_micros":41000,"claim_state":"anonymous",
        "top_up_url":"https://publikhq.com/claim/HK7F-2QWD",
        "claim_url":"https://publikhq.com/claim/HK7F-2QWD","add_credit_url":"https://publikhq.com/dashboard/api/add",
        "plans_url":"https://publikhq.com/developers#plans",
        "week":{"used_micros":248760,"budget_micros":null,"resets_at":"2026-09-25T17:04:11Z"}}}"#;

    #[test]
    fn cleanup_402_maps_to_insufficient_credit_with_the_one_link() {
        let (url, h) = fake_server(402, &[("x-publik-balance", "1240")], BODY_402);
        let p = PublikProvider::new("pk_live_abc".into(), &url, "");
        let err = p.cleanup_typed("hello", &ctx()).unwrap_err();
        h.join().unwrap();
        match err {
            PublikError::InsufficientCredit { kind, message, available_micros, top_up_url, claim_state } => {
                assert_eq!(kind, "insufficient_credit");
                assert_eq!(message, "Not enough publik credit for this request.");
                assert_eq!(available_micros, 1240);
                assert_eq!(top_up_url.as_deref(), Some("https://publikhq.com/claim/HK7F-2QWD"));
                assert_eq!(claim_state.as_deref(), Some("anonymous"));
            }
            other => panic!("expected InsufficientCredit, got {other:?}"),
        }
        // The balance header on the 402 is still recorded.
        assert_eq!(p.last_balance().unwrap().balance_micros, 1240);
    }

    #[test]
    fn map_error_covers_every_contract_status() {
        let h = reqwest::header::HeaderMap::new();
        // 402 model_requires_claim is the same typed outcome with its own kind.
        match map_error(402, &h, r#"{"error":{"type":"model_requires_claim","message":"Link this computer to use publik-smart.","top_up_url":"https://publikhq.com/claim/AB12-CD34"}}"#) {
            PublikError::InsufficientCredit { kind, top_up_url, .. } => {
                assert_eq!(kind, "model_requires_claim");
                assert_eq!(top_up_url.as_deref(), Some("https://publikhq.com/claim/AB12-CD34"));
            }
            other => panic!("{other:?}"),
        }
        // The prototype's top-level extras are read too.
        match map_error(402, &h, r#"{"error":{"type":"insufficient_credit","message":"m"},"available_micros":5,"top_up_url":"https://publikhq.com/x"}"#) {
            PublikError::InsufficientCredit { available_micros, top_up_url, .. } => {
                assert_eq!(available_micros, 5);
                assert_eq!(top_up_url.as_deref(), Some("https://publikhq.com/x"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(map_error(401, &h, r#"{"error":{"type":"invalid_api_key","message":"x"}}"#), PublikError::InvalidKey));
        assert!(matches!(map_error(401, &h, ""), PublikError::InvalidKey));
        assert!(matches!(
            map_error(401, &h, r#"{"error":{"type":"key_revoked","message":"x","reprovision":true}}"#),
            PublikError::KeyRevoked { reprovision: true }
        ));
        assert!(matches!(
            map_error(401, &h, r#"{"error":{"type":"key_revoked","message":"x","reprovision":false}}"#),
            PublikError::KeyRevoked { reprovision: false }
        ));
        let mut ra = reqwest::header::HeaderMap::new();
        ra.insert("retry-after", "30".parse().unwrap());
        match map_error(429, &ra, r#"{"error":{"type":"daily_cap_reached","message":"x"}}"#) {
            PublikError::RateLimited { kind, retry_after_secs } => {
                assert_eq!(kind, "daily_cap_reached");
                assert_eq!(retry_after_secs, Some(30));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(map_error(503, &ra, r#"{"error":{"type":"gateway_unavailable"}}"#), PublikError::Unavailable { retry_after_secs: Some(30) }));
        assert!(matches!(map_error(500, &h, "boom"), PublikError::Http { status: 500, .. }));
    }

    #[test]
    fn cleanup_401_revoked_and_429_are_typed() {
        let (url, h) = fake_server(401, &[], r#"{"error":{"type":"key_revoked","message":"revoked","reprovision":true}}"#);
        let p = PublikProvider::new("pk_live_abc".into(), &url, "");
        assert!(matches!(p.cleanup_typed("hi", &ctx()), Err(PublikError::KeyRevoked { reprovision: true })));
        h.join().unwrap();

        let (url, h) = fake_server(429, &[("Retry-After", "30")], r#"{"error":{"type":"rate_limit_exceeded","message":"slow down"}}"#);
        let p = PublikProvider::new("pk_live_abc".into(), &url, "");
        match p.cleanup_typed("hi", &ctx()) {
            Err(PublikError::RateLimited { retry_after_secs, .. }) => assert_eq!(retry_after_secs, Some(30)),
            other => panic!("{other:?}"),
        }
        h.join().unwrap();

        let (url, h) = fake_server(200, &[], r#"{"choices":[{"message":{"content":"   "}}]}"#);
        let p = PublikProvider::new("pk_live_abc".into(), &url, "");
        assert!(matches!(p.cleanup_typed("hi", &ctx()), Err(PublikError::Http { .. })));
        h.join().unwrap();
    }

    const BODY_201: &str = r#"{
        "install_id": "3f1c9b5e-7a2d-4c8e-9f0b-1d2e3f4a5b6c",
        "key": "pk_live_a8k2m9x4q7v1_h3n6r9t2w5y8z1b4c7d0f3g6j9k2m5p8",
        "key_id": "a8k2m9x4q7v1",
        "base_url": "https://publikhq.com/api/v1",
        "models": { "fast": "publik-fast", "balanced": "publik-balanced", "smart": "publik-smart" },
        "dialects": ["chat_completions", "responses", "messages"],
        "claim_code": "HK7F-2QWD",
        "claim_url": "https://publikhq.com/claim/HK7F-2QWD",
        "claim_expires_at": "2026-10-18T17:04:11Z",
        "starter_micros": 250000,
        "balance_micros": 250000,
        "starting_credit_micros": 250000,
        "wallet": { "balance_micros": 250000, "claim_state": "anonymous" },
        "disclosure": { "version": 1, "cost": "…", "data_path": "…" }
    }"#;

    fn req() -> ProvisionRequest {
        ProvisionRequest {
            app_token: "pat_whimprflow_k3m9x2q7v5n8r4t6w1y0z2b5c8d1f4g7".into(),
            app_version: "0.2.0".into(),
            install_id: "3f1c9b5e-7a2d-4c8e-9f0b-1d2e3f4a5b6c".into(),
            os_version: Some("15.6".into()),
            device_name: Some("Mann's MacBook Pro".into()),
        }
    }

    #[test]
    fn provision_against_a_local_fake_server() {
        let (url, h) = fake_server(201, &[], BODY_201);
        let p = provision(&url, &req()).unwrap();
        let captured = h.join().unwrap();

        // The request carried everything CONTRACT §3.2 lists.
        assert!(captured.request_line.starts_with("POST /installs "), "{}", captured.request_line);
        assert_eq!(captured.header("authorization"), Some("Bearer pat_whimprflow_k3m9x2q7v5n8r4t6w1y0z2b5c8d1f4g7"));
        assert_eq!(captured.header("idempotency-key"), Some("3f1c9b5e-7a2d-4c8e-9f0b-1d2e3f4a5b6c"));
        let body = captured.json();
        assert_eq!(body["app_token"], "pat_whimprflow_k3m9x2q7v5n8r4t6w1y0z2b5c8d1f4g7");
        assert_eq!(body["app_slug"], "whimprflow");
        assert_eq!(body["app_version"], "0.2.0");
        assert_eq!(body["os"], os_name());
        assert!(matches!(body["os"].as_str(), Some("macos" | "windows" | "linux")));
        assert_eq!(body["arch"], arch_name());
        assert_eq!(body["os_version"], "15.6");
        assert_eq!(body["device_name"], "Mann's MacBook Pro");
        assert_eq!(body["install_id"], "3f1c9b5e-7a2d-4c8e-9f0b-1d2e3f4a5b6c");
        assert_eq!(body["disclosure_version"], DISCLOSURE_VERSION);
        assert_eq!(body["dialects"], serde_json::json!(["chat_completions"]));

        // The response was honoured field by field.
        assert!(!p.replay);
        assert_eq!(p.key.as_deref(), Some("pk_live_a8k2m9x4q7v1_h3n6r9t2w5y8z1b4c7d0f3g6j9k2m5p8"));
        assert_eq!(p.base_url.as_deref(), Some("https://publikhq.com/api/v1"));
        assert_eq!(p.models.as_ref().unwrap().fast.as_deref(), Some("publik-fast"));
        assert_eq!(p.claim_url.as_deref(), Some("https://publikhq.com/claim/HK7F-2QWD"));
        assert_eq!(p.starting_balance_micros(), 250_000);
    }

    #[test]
    fn provision_replay_200_has_no_key() {
        let (url, h) = fake_server(
            200,
            &[],
            r#"{"install_id":"3f1c9b5e-7a2d-4c8e-9f0b-1d2e3f4a5b6c","key":null,"starter_micros":0,"claim_state":"anonymous","claim_url":"https://publikhq.com/claim/HK7F-2QWD"}"#,
        );
        let p = provision(&url, &req()).unwrap();
        h.join().unwrap();
        assert!(p.replay);
        assert!(p.key.is_none());
        assert_eq!(p.claim_state.as_deref(), Some("anonymous"));
        assert_eq!(p.starting_balance_micros(), 0);
    }

    #[test]
    fn provision_errors_are_typed() {
        // An unknown app token has no key to forget: a plain Http error.
        let (url, h) = fake_server(401, &[], r#"{"error":{"type":"invalid_app_token","message":"no"}}"#);
        assert!(matches!(provision(&url, &req()), Err(PublikError::Http { status: 401, .. })));
        h.join().unwrap();
        let (url, h) = fake_server(429, &[("Retry-After", "600")], r#"{"error":{"type":"rate_limited","message":"no"}}"#);
        match provision(&url, &req()) {
            Err(PublikError::RateLimited { retry_after_secs, .. }) => assert_eq!(retry_after_secs, Some(600)),
            other => panic!("{other:?}"),
        }
        h.join().unwrap();
        // Nothing listening: a transport error, never a panic.
        assert!(matches!(provision("http://127.0.0.1:1", &req()), Err(PublikError::Transport(_))));
    }

    #[test]
    fn fetch_wallet_parses_the_contract_shape() {
        let body = r#"{
            "install_id":"3f1c9b5e-…","app_slug":"whimprflow","claim_state":"anonymous",
            "balance_micros":181240,
            "starter":{"remaining_micros":181240,"expires_at":"2026-10-18T17:04:11Z"},
            "plan":{"id":"none","label":"No plan","monthly_micros":0},
            "week":{"used_micros":68760,"budget_micros":null,"resets_at":"2026-09-25T17:04:11Z","window_days":7},
            "daily_cap_micros":250000,"spent_today_micros":68760,
            "claim_code":"HK7F-2QWD","claim_url":"https://publikhq.com/claim/HK7F-2QWD",
            "add_credit_url":"https://publikhq.com/dashboard/api/add","plans_url":"https://publikhq.com/developers#plans"
        }"#;
        let (url, h) = fake_server(200, &[], body);
        let w = fetch_wallet(&url, "pk_live_abc").unwrap();
        let captured = h.join().unwrap();
        assert!(captured.request_line.starts_with("GET /wallet "), "{}", captured.request_line);
        assert_eq!(captured.header("authorization"), Some("Bearer pk_live_abc"));
        assert_eq!(w.balance_micros, 181_240);
        assert_eq!(w.claim_state.as_deref(), Some("anonymous"));
        assert_eq!(w.claim_url.as_deref(), Some("https://publikhq.com/claim/HK7F-2QWD"));
        assert_eq!(w.week_used_micros, Some(68_760));
        assert_eq!(w.week_budget_micros, None);
        assert_eq!(w.starter_remaining_micros, Some(181_240));
        assert_eq!(w.daily_cap_micros, Some(250_000));

        // The /balance alias shape (available_micros) is accepted too.
        let alias = parse_wallet(&serde_json::json!({"available_micros": 7, "top_up_url": "https://publikhq.com/claim/X"})).unwrap();
        assert_eq!(alias.balance_micros, 7);
        assert_eq!(alias.top_up_url.as_deref(), Some("https://publikhq.com/claim/X"));

        // A revoked key on /wallet is the same typed outcome as on cleanup.
        let (url, h) = fake_server(401, &[], r#"{"error":{"type":"key_revoked","reprovision":false}}"#);
        assert!(matches!(fetch_wallet(&url, "pk_live_abc"), Err(PublikError::KeyRevoked { reprovision: false })));
        h.join().unwrap();
    }
}
