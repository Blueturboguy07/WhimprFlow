//! publik API for the shell: where the key comes from, when it is minted, and
//! what the Hub is told. Mirrors the OpenAI/Anthropic key handling in
//! `hotkey.rs` (env var, then keychain) and adds two more rungs from the publik
//! credential convention: the per-app file Iris writes, and a provisioning
//! call with the app token baked into this build.
//!
//! Rules this module keeps (R23 §5, CONTRACT §3.2):
//! - Nothing here ever reads, writes, or deletes `openai_api_key` /
//!   `anthropic_api_key`. The publik key has its own keychain account.
//! - A key the user provided (env, convention file, or an earlier mint) is never
//!   overwritten: provisioning runs only when every other rung is empty.
//! - No network call before the disclosure is accepted. `provision_now` is
//!   reachable only from `publik_accept_disclosure` (the user's own tap) and
//!   from a `401 key_revoked` with `reprovision: true` after that consent.
//! - A 402 renders the gateway's message plus exactly one link (`top_up_url`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use whimpr_cleanup::publik::{self as api, fmt_usd, BalanceSnapshot, Provisioned, PublikError, Wallet};
use whimpr_core::{CleanupMode, Settings, PUBLIK_DISCLOSURE_VERSION};

const SERVICE: &str = "com.whimpr.whimprflow";
/// The publik key's own keychain account — never the user's BYO accounts.
pub const ACCOUNT: &str = "publik_api_key";
/// Baked in at build time (scripts/build-macos.sh refuses a release without
/// it). A dev build without it simply has no provisioning rung — BYO and Local
/// keep working, and the Settings card says "not available in this build".
const APP_TOKEN: Option<&str> = option_env!("PUBLIK_APP_TOKEN");
/// `GET /wallet` at most this often from the Settings pane.
const WALLET_REFRESH_SECS: u64 = 30;

// ── The plan CTA and its justification (CONTRACT §12, founder 2026-09-19) ────

/// The one justification for charging, next to every mention of money. Built
/// from the site's `why-it-costs.ts` sentences — never reworded here, never a
/// new pricing claim.
pub const WHY_IT_COSTS: &str = "A provider charges for every request the app makes; publik pays that bill and passes it on at half the provider's list price. Nothing is charged behind your back — usage only draws from a plan or pack you choose to buy.";
/// The first-run card's primary button (opens `claim_url`).
pub const CTA_LINK_AND_PICK: &str = "Link this computer & pick a plan";
/// The settings card's primary button while the key is still anonymous.
pub const CTA_PICK_PLAN: &str = "Pick a plan";
/// The settings card's primary button once the computer is claimed.
pub const CTA_MANAGE_PLAN: &str = "Manage plan";
/// The first-run card's secondary button: keep the free starter, change nothing.
pub const CTA_LATER: &str = "Later";
/// The gateway's documented anonymous starter (CONTRACT §5), used only as the
/// denominator of the "running low" check when the grant is unknown. The
/// balance line itself always comes from the response.
const DOCUMENTED_STARTER_MICROS: i64 = 250_000;
/// "Running low" = less than a fifth of the starter grant left.
const LOW_STARTER_NUM: i64 = 1;
const LOW_STARTER_DEN: i64 = 5;

// ── Key store seam (real keychain in the app, in-memory in tests) ────────────

/// The credential store the shell already uses (the OS keychain), abstracted
/// so the never-overwrite rule can be proven without a real keychain.
pub trait KeyStore {
    fn get(&self, account: &str) -> Option<String>;
    fn set(&self, account: &str, value: &str) -> Result<(), String>;
    fn delete(&self, account: &str);
}

/// `keyring` against `com.whimpr.whimprflow` — the same store as `set_api_key`.
pub struct Keychain;

impl KeyStore for Keychain {
    fn get(&self, account: &str) -> Option<String> {
        keyring::Entry::new(SERVICE, account)
            .ok()
            .and_then(|e| e.get_password().ok())
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
    }
    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())?;
        // Same delete-then-set as `set_api_key`: an item created by another
        // process (the `security` CLI) is not readable by this app.
        let _ = entry.delete_credential();
        entry.set_password(value).map_err(|e| e.to_string())
    }
    fn delete(&self, account: &str) {
        if let Ok(e) = keyring::Entry::new(SERVICE, account) {
            let _ = e.delete_credential();
        }
    }
}

// ── Status the Hub sees ──────────────────────────────────────────────────────

/// Mirrored by `PublikStatus` in `ui/src/hub/api.ts`.
#[derive(Clone, Serialize, Default, Debug)]
pub struct PublikStatus {
    /// A token is compiled in, or a key is already present.
    pub available: bool,
    pub has_key: bool,
    pub balance_micros: Option<i64>,
    /// "$0.41 left"
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
    /// `401 key_revoked` with `reprovision: false`: removed from the account.
    pub disconnected: bool,
    /// Transport error or `503` on the last call. Nothing was charged.
    pub unreachable: bool,
    pub disclosure_needed: bool,
    /// Provisioning is possible in this build (token compiled in).
    pub can_provision: bool,
    /// The first-run card (CONTRACT §12.1): shown right after provisioning,
    /// until "Later" or the primary button. `None` once dismissed.
    pub first_run: Option<FirstRunCard>,
    /// [`WHY_IT_COSTS`], so every surface reads the same sentence.
    pub justification: String,
    /// The settings card's primary button (CONTRACT §12.2).
    pub plan_cta: PlanCta,
    /// The non-blocking banner: a 402, or the starter running low. Exactly one link.
    pub notice: Option<PublikNotice>,
}

/// What the app shows the moment `POST /installs` has succeeded, in this
/// order: the balance line, the justification, the primary button, "Later".
#[derive(Clone, Serialize, Debug, PartialEq, Eq)]
pub struct FirstRunCard {
    /// "$0.25 of free starter usage" — from the mint response, never a constant.
    pub balance_line: String,
    pub justification: String,
    /// [`CTA_LINK_AND_PICK`].
    pub cta_label: String,
    /// The response's `claim_url` (already dropped unless on publikhq.com).
    /// `None` = the button falls back to the dashboard, which has a claim box.
    pub claim_url: Option<String>,
    /// [`CTA_LATER`].
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
    /// `exhausted` (402) | `low_starter` (wallet / headers).
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

// ── Key resolution (rungs 1–3; never provisions) ─────────────────────────────

/// Convention file (PLAN §2): macOS `~/Library/Application Support/publik/apps/whimprflow.json`,
/// Windows `%LOCALAPPDATA%\publik\apps\whimprflow.json`. Read-only from the app's side.
pub fn convention_file() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let base = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from)?;
    #[cfg(not(target_os = "windows"))]
    let base = std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Library/Application Support"))?;
    Some(base.join("publik/apps/whimprflow.json"))
}

fn read_convention_file(path: &Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let k = v["key"].as_str()?.trim().to_string();
    if k.starts_with("pk_") {
        Some(k)
    } else {
        None
    }
}

/// The resolution order, with every input explicit so it can be tested:
/// `PUBLIK_API_KEY` env → keychain `publik_api_key` → convention file.
pub fn read_key_from(env_key: Option<&str>, store: &dyn KeyStore, convention: Option<&Path>) -> Option<String> {
    if let Some(k) = env_key.map(str::trim).filter(|k| !k.is_empty()) {
        return Some(k.to_string());
    }
    if let Some(k) = store.get(ACCOUNT) {
        return Some(k);
    }
    convention.and_then(read_convention_file)
}

/// Rungs 1–3 against the real environment. Never provisions (that needs the
/// user's consent — see `publik_accept_disclosure`).
pub fn read_key() -> Option<String> {
    let env_key = std::env::var("PUBLIK_API_KEY").ok();
    read_key_from(env_key.as_deref(), &Keychain, convention_file().as_deref())
}

/// The API root in force: env override → provisioning response → default.
pub fn base_url() -> String {
    api::resolve_base_url(&crate::hotkey::current_settings().publik_base_url)
}

// ── Provisioning (rung 4) ────────────────────────────────────────────────────

/// 16 random bytes as a v4 UUID string. Uniqueness across installs is what
/// matters (it is the mint's idempotency key), not unpredictability. std only.
pub fn new_install_id() -> String {
    let mut bytes = [0u8; 16];
    let mut filled = false;
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            filled = f.read_exact(&mut bytes).is_ok();
        }
    }
    if !filled {
        // Fallback: hash time, pid, thread id and a stack address through the
        // std hasher twice — plenty of uniqueness, no extra crates.
        use std::hash::{Hash, Hasher};
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let probe = 0u8;
        for (i, chunk) in bytes.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            nanos.hash(&mut h);
            std::process::id().hash(&mut h);
            std::thread::current().id().hash(&mut h);
            (&probe as *const u8 as usize).hash(&mut h);
            i.hash(&mut h);
            chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
        }
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sw_vers").arg("-productVersion").output().ok()?;
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return if v.is_empty() { None } else { Some(v) };
    }
    #[allow(unreachable_code)]
    None
}

fn device_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output().ok()?;
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return if v.is_empty() { None } else { Some(v) };
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var("COMPUTERNAME").ok().filter(|s| !s.trim().is_empty());
    }
    #[allow(unreachable_code)]
    None
}

/// Apply a fresh mint: the key goes to its own keychain account (and nowhere
/// else), and the response's `base_url` / `models.fast` / `claim_url` are
/// persisted in settings so they are honoured over the compiled defaults.
pub fn adopt_provisioned(store: &dyn KeyStore, settings: &mut Settings, p: &Provisioned, key: &str) -> Result<(), String> {
    store.set(ACCOUNT, key)?;
    if let Some(id) = p.install_id.as_deref().filter(|s| !s.is_empty()) {
        settings.publik_install_id = id.to_string();
    }
    if let Some(u) = p.base_url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        settings.publik_base_url = u.trim_end_matches('/').to_string();
    }
    if let Some(m) = p.models.as_ref().and_then(|m| m.fast.as_deref()).filter(|s| !s.trim().is_empty()) {
        settings.publik_model = m.trim().to_string();
    }
    settings.publik_claim_url = p.claim_url.clone().unwrap_or_default();
    settings.publik_disclosure_version = PUBLIK_DISCLOSURE_VERSION;
    settings.publik_starter_micros = p.starter_micros.unwrap_or_else(|| p.starting_balance_micros()).max(0);
    // The card is now owed (CONTRACT §12.4): cleared only by its own buttons.
    settings.publik_cta_pending = true;
    Ok(())
}

// ── The card, the button, the banner (pure, so they are tested) ──────────────

/// "$0.25 of free starter usage".
pub fn starter_line(micros: i64) -> String {
    format!("{} of free starter usage", fmt_usd(micros.max(0)))
}

/// The first-run card from the mint response: balance from the response,
/// the one justification, the primary button on `claim_url`, "Later".
pub fn first_run_card(p: &Provisioned) -> FirstRunCard {
    FirstRunCard {
        balance_line: starter_line(p.starting_balance_micros()),
        justification: WHY_IT_COSTS.to_string(),
        cta_label: CTA_LINK_AND_PICK.to_string(),
        claim_url: api::publik_link(p.claim_url.as_deref()),
        later_label: CTA_LATER.to_string(),
    }
}

/// The settings card's primary button: "Pick a plan" → `claim_url` while the
/// key is anonymous; "Manage plan" → the dashboard once claimed. Any link is
/// checked against publikhq.com again here; the dashboard is the fallback.
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

/// Below a fifth of the starter grant left → the banner, with the wallet's
/// one link. `granted` 0 = unknown, the documented anonymous starter applies.
pub fn low_starter_notice(remaining: i64, granted: i64, top_up_url: Option<&str>) -> Option<PublikNotice> {
    let granted = if granted > 0 { granted } else { DOCUMENTED_STARTER_MICROS };
    if remaining <= 0 || remaining * LOW_STARTER_DEN >= granted * LOW_STARTER_NUM {
        return None;
    }
    let link_url = api::publik_link(top_up_url).unwrap_or_else(|| api::DASHBOARD_URL.to_string());
    Some(PublikNotice {
        kind: "low_starter".to_string(),
        headline: "publik API starter is running low".to_string(),
        message: format!(
            "{} of your free starter usage is left. {WHY_IT_COSTS} Pick a plan or a pack to keep cloud cleanup going, or use Local or your own key.",
            fmt_usd(remaining)
        ),
        link_label: "Pick a plan".to_string(),
        link_url,
    })
}

/// The 402 banner: the gateway's message (it already says what the link
/// does) plus exactly one link, `top_up_url`, then what the app does about it.
pub fn exhausted_notice(message: &str, claim_state: Option<&str>, link: &str) -> PublikNotice {
    PublikNotice {
        kind: "exhausted".to_string(),
        headline: "publik API needs a plan or a pack".to_string(),
        message: format!(
            "{} Dictation still works; text is pasted without cleanup — or use Local or your own key under Settings → Cleanup Engine.",
            message.trim()
        ),
        link_label: if claim_state == Some("claimed") { "Add a plan or pack" } else { "Link this computer & pick a plan" }.to_string(),
        link_url: api::publik_link(Some(link)).unwrap_or_else(|| api::DASHBOARD_URL.to_string()),
    }
}

/// "Later" / the primary button: the card is no longer owed. Touches nothing
/// else — the key stays where it is, the mode stays what it was.
pub fn dismiss_first_run_in(settings: &mut Settings) {
    settings.publik_cta_pending = false;
}

/// The never-overwrite rule as one function: return the key from rungs 1–3 if
/// any exists, and only otherwise run `mint`. `mint` gets the settings so it can
/// fill `publik_install_id`; whatever it returns is adopted through
/// [`adopt_provisioned`], which touches only the publik account.
pub fn ensure_key_with(
    env_key: Option<&str>,
    store: &dyn KeyStore,
    convention: Option<&Path>,
    settings: &mut Settings,
    mint: impl FnOnce(&mut Settings) -> Result<Provisioned, String>,
) -> Result<(String, Option<Provisioned>), String> {
    if let Some(k) = read_key_from(env_key, store, convention) {
        return Ok((k, None));
    }
    let p = mint(settings)?;
    let key = p.key.clone().ok_or_else(|| "publik did not return a key for this install".to_string())?;
    adopt_provisioned(store, settings, &p, &key)?;
    Ok((key, Some(p)))
}

/// One `POST /installs`, retried once with a fresh `install_id` when the
/// server replays (200, `key: null`) and we hold no credential — CONTRACT §3.2 [B1].
fn mint(settings: &mut Settings) -> Result<Provisioned, String> {
    let token = APP_TOKEN.ok_or_else(|| "publik API is not available in this build (no app token).".to_string())?;
    if settings.publik_install_id.is_empty() {
        settings.publik_install_id = new_install_id();
    }
    let req = |install_id: &str| api::ProvisionRequest {
        app_token: token.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        install_id: install_id.to_string(),
        os_version: os_version(),
        device_name: device_name(),
    };
    let base = api::resolve_base_url(&settings.publik_base_url);
    let mut p = api::provision(&base, &req(&settings.publik_install_id)).map_err(describe)?;
    if p.replay && p.key.is_none() {
        eprintln!("[whimpr] publik replayed install {} without a key — minting a fresh install id once", settings.publik_install_id);
        settings.publik_install_id = new_install_id();
        p = api::provision(&base, &req(&settings.publik_install_id)).map_err(describe)?;
    }
    Ok(p)
}

/// Human-readable failure text for the card (never a stack of Rust types).
fn describe(e: PublikError) -> String {
    match e {
        PublikError::RateLimited { retry_after_secs, .. } => match retry_after_secs {
            Some(s) => format!("publik is busy right now — try again in {} minutes.", (s / 60).max(1)),
            None => "publik is busy right now — try again in a few minutes.".to_string(),
        },
        PublikError::Unavailable { .. } => "publik API is unreachable right now. Nothing is being charged. Try again in a minute, or use your own key.".to_string(),
        PublikError::Transport(_) => "Could not reach publikhq.com. Check your connection and try again, or use your own key.".to_string(),
        PublikError::Http { status, detail } => {
            let msg: serde_json::Value = serde_json::from_str(&detail).unwrap_or(serde_json::Value::Null);
            let m = msg["error"]["message"].as_str().unwrap_or("").trim().to_string();
            if m.is_empty() { format!("publik answered HTTP {status}.") } else { m }
        }
        other => other.to_string(),
    }
}

/// Rung 4, with consent already given. Mints if no key exists, adopts the
/// response, persists settings (which rebuilds providers), updates the card.
fn provision_now() -> Result<(), String> {
    let mut settings = crate::hotkey::current_settings();
    let env_key = std::env::var("PUBLIK_API_KEY").ok();
    let (_key, minted) = ensure_key_with(env_key.as_deref(), &Keychain, convention_file().as_deref(), &mut settings, mint)?;
    settings.publik_disclosure_version = PUBLIK_DISCLOSURE_VERSION;
    crate::hotkey::update_settings(settings.clone());
    let mut s = state().lock().unwrap();
    s.has_key = true;
    s.available = true;
    s.exhausted = false;
    s.disconnected = false;
    s.unreachable = false;
    s.model = if settings.publik_model.is_empty() { api::DEFAULT_MODEL_ALIAS.to_string() } else { settings.publik_model.clone() };
    if let Some(p) = minted {
        s.claim_url = p.claim_url.clone();
        s.claim_state = p.claim_state.clone().or_else(|| Some("anonymous".to_string()));
        set_balance_locked(&mut s, p.starting_balance_micros(), None);
        // The card (CONTRACT §12.1), with the real balance on it — not the
        // disclosure sheet the user just accepted.
        s.first_run = Some(first_run_card(&p));
        s.notice = None;
        eprintln!("[whimpr] publik install provisioned (starting balance {})", fmt_usd(p.starting_balance_micros()));
    }
    Ok(())
}

// ── Balance line ─────────────────────────────────────────────────────────────

fn set_balance_locked(s: &mut PublikStatus, micros: i64, charge: Option<i64>) {
    s.balance_micros = Some(micros);
    s.balance_label = Some(format!("{} left", fmt_usd(micros)));
    if let Some(c) = charge {
        s.last_charge_label = Some(format!("last cleanup {}", fmt_usd(c)));
    }
    s.exhausted = micros <= 0;
}

/// Starter below a fifth → raise the banner once per dip (a top-up that lifts
/// the starter back above the line re-arms it). Never replaces a 402 banner.
fn note_low_starter_locked(s: &mut PublikStatus, remaining: Option<i64>, granted: i64, top_up_url: Option<&str>) {
    let Some(remaining) = remaining else { return };
    let link = top_up_url
        .map(str::to_string)
        .or_else(|| s.top_up_url.clone())
        .or_else(|| if s.claim_state.as_deref() == Some("claimed") { None } else { s.claim_url.clone() });
    match low_starter_notice(remaining, granted, link.as_deref()) {
        Some(n) => {
            let already = LOW_STARTER_SHOWN.swap(true, std::sync::atomic::Ordering::SeqCst);
            if !already && s.notice.is_none() {
                s.notice = Some(n);
            }
        }
        None => LOW_STARTER_SHOWN.store(false, std::sync::atomic::Ordering::SeqCst),
    }
}
static LOW_STARTER_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn week_label(used: Option<i64>, budget: Option<i64>) -> Option<String> {
    match (used, budget) {
        (Some(u), Some(b)) if b > 0 => Some(format!("This week {} of {}", fmt_usd(u), fmt_usd(b))),
        (Some(u), _) => Some(format!("{} used this week", fmt_usd(u))),
        _ => None,
    }
}

/// After a successful cleanup: the `x-publik-*` headers move the card's
/// balance line live (`whimpr://publik`).
pub fn set_balance_from_snapshot(app: &AppHandle, snap: &BalanceSnapshot) {
    let dto = {
        let mut s = state().lock().unwrap();
        set_balance_locked(&mut s, snap.balance_micros, snap.last_charge_micros);
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
        let granted = crate::hotkey::current_settings().publik_starter_micros;
        note_low_starter_locked(&mut s, snap.starter_remaining_micros, granted, None);
        s.clone()
    };
    let _ = app.emit("whimpr://publik", dto);
}

fn apply_wallet(s: &mut PublikStatus, w: &Wallet, starter_granted: i64) {
    set_balance_locked(s, w.balance_micros, None);
    s.claim_state = w.claim_state.clone();
    if w.claim_state.as_deref() == Some("claimed") {
        s.claim_url = None;
    } else if w.claim_url.is_some() {
        s.claim_url = w.claim_url.clone();
    }
    s.top_up_url = w.top_up_url.clone().or_else(|| {
        if w.claim_state.as_deref() == Some("claimed") { w.add_credit_url.clone() } else { w.claim_url.clone() }
    });
    s.week_label = week_label(w.week_used_micros, w.week_budget_micros);
    s.unreachable = false;
    s.disconnected = false;
    if !s.exhausted {
        if let Some(n) = s.notice.as_ref() {
            if n.kind == "exhausted" {
                s.notice = None;
            }
        }
    }
    note_low_starter_locked(s, w.starter_remaining_micros, starter_granted, s.top_up_url.clone().as_deref());
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
    match api::fetch_wallet(&base_url(), &key) {
        Ok(w) => {
            let mut settings = crate::hotkey::current_settings();
            let mut s = state().lock().unwrap();
            apply_wallet(&mut s, &w, settings.publik_starter_micros);
            let claim = w.claim_url.clone().unwrap_or_default();
            if w.claim_state.as_deref() == Some("claimed") || (!claim.is_empty() && claim != settings.publik_claim_url) {
                settings.publik_claim_url = if w.claim_state.as_deref() == Some("claimed") { String::new() } else { claim };
                drop(s);
                crate::hotkey::update_settings(settings);
            }
        }
        Err(e) => {
            eprintln!("[whimpr] publik wallet refresh failed: {e}");
            note_error(&e);
        }
    }
}

// ── Error handling shared by the dictation pipeline ─────────────────────────

/// 402 from the gateway: remember it, and put the message plus its one link in
/// front of the user as a non-blocking banner (`whimpr://publik`). Dictation
/// carries on — raw text is pasted, and BYO / Local are untouched.
fn report_exhausted(app: &AppHandle, message: &str, available_micros: i64, top_up_url: Option<String>, claim_state: Option<String>) {
    let claim_from_settings = crate::hotkey::current_settings().publik_claim_url;
    let mut s = state().lock().unwrap();
    set_balance_locked(&mut s, available_micros, None);
    s.exhausted = true;
    if claim_state.is_some() {
        s.claim_state = claim_state.clone();
    }
    if top_up_url.is_some() {
        s.top_up_url = top_up_url.clone();
    }
    // The one link: the gateway's top_up_url, else the claim link from
    // provisioning, else the dashboard.
    let link = s
        .top_up_url
        .clone()
        .or_else(|| s.claim_url.clone())
        .or_else(|| if claim_from_settings.is_empty() { None } else { Some(claim_from_settings) })
        .unwrap_or_else(|| api::DASHBOARD_URL.to_string());
    let notice = exhausted_notice(message, s.claim_state.as_deref(), &link);
    eprintln!("[whimpr] ⚠ {}: {}", notice.headline, notice.message);
    s.notice = Some(notice);
    let _ = app.emit("whimpr://publik", s.clone());
}

/// Drop the publik key (only its own account). The next "Turn on" mints again.
pub fn forget_key() {
    Keychain.delete(ACCOUNT);
    let mut s = state().lock().unwrap();
    s.has_key = false;
    s.balance_micros = None;
    s.balance_label = None;
    s.last_charge_label = None;
    s.week_label = None;
    s.exhausted = false;
    // No key, no starter to spend: nothing is owed and nothing is running low.
    s.first_run = None;
    s.notice = None;
    let mut settings = crate::hotkey::current_settings();
    if settings.publik_cta_pending {
        dismiss_first_run_in(&mut settings);
        drop(s);
        crate::hotkey::update_settings(settings);
    }
}

fn note_error(e: &PublikError) {
    let mut s = state().lock().unwrap();
    match e {
        PublikError::Unavailable { .. } | PublikError::Transport(_) => s.unreachable = true,
        PublikError::KeyRevoked { reprovision: false } => s.disconnected = true,
        _ => {}
    }
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
            forget_key();
            crate::hotkey::rebuild_providers();
        }
        PublikError::KeyRevoked { reprovision: true } => {
            // Idle sweep: the user consented once; silently mint again with
            // the same install_id, off the dictation thread.
            eprintln!("[whimpr] publik key revoked by the idle sweep — re-provisioning");
            forget_key();
            crate::hotkey::rebuild_providers();
            if crate::hotkey::current_settings().publik_disclosure_version >= PUBLIK_DISCLOSURE_VERSION {
                let app2 = app.clone();
                std::thread::spawn(move || match provision_now() {
                    Ok(()) => {
                        let _ = app2.emit("whimpr://publik", state().lock().unwrap().clone());
                    }
                    Err(err) => eprintln!("[whimpr] publik re-provision failed: {err}"),
                });
            }
        }
        PublikError::KeyRevoked { reprovision: false } => {
            eprintln!("[whimpr] publik key revoked from the account — disconnected");
            forget_key();
            crate::hotkey::rebuild_providers();
            {
                let mut s = state().lock().unwrap();
                s.disconnected = true;
                let _ = app.emit("whimpr://publik", s.clone());
            }
            crate::diag::report_text(
                app,
                "publik API is disconnected".to_string(),
                "This computer was removed from your publik account. Reconnect under Settings → Cleanup Engine, or use your own key. Dictation still works; text is pasted without cleanup.".to_string(),
            );
        }
        PublikError::RateLimited { kind, retry_after_secs } => {
            eprintln!("[whimpr] publik rate limited ({kind}, retry after {retry_after_secs:?}) — pasting raw");
        }
        PublikError::Unavailable { .. } | PublikError::Transport(_) => {
            eprintln!("[whimpr] publik unreachable ({e}) — pasting raw, nothing charged");
            note_error(e);
            let _ = app.emit("whimpr://publik", state().lock().unwrap().clone());
        }
        PublikError::Http { .. } => {
            eprintln!("[whimpr] publik error ({e}) — pasting raw");
        }
    }
}

pub fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

fn snapshot() -> PublikStatus {
    let settings = crate::hotkey::current_settings();
    let mut s = state().lock().unwrap().clone();
    s.has_key = read_key().is_some();
    s.can_provision = APP_TOKEN.is_some();
    s.available = s.has_key || s.can_provision;
    s.disclosure_needed = settings.publik_disclosure_version < PUBLIK_DISCLOSURE_VERSION;
    s.dashboard_url = api::DASHBOARD_URL.to_string();
    if s.model.is_empty() {
        s.model = if settings.publik_model.is_empty() { api::DEFAULT_MODEL_ALIAS.to_string() } else { settings.publik_model.clone() };
    }
    if s.claim_url.is_none() && !settings.publik_claim_url.is_empty() && s.claim_state.as_deref() != Some("claimed") {
        s.claim_url = Some(settings.publik_claim_url.clone());
    }
    s.justification = WHY_IT_COSTS.to_string();
    s.plan_cta = plan_cta(s.claim_state.as_deref(), s.claim_url.as_deref());
    // A card still owed from an earlier launch (provisioned, never dismissed):
    // rebuild it from what was persisted, with the freshest balance known.
    if s.first_run.is_none() && settings.publik_cta_pending && s.has_key {
        s.first_run = Some(FirstRunCard {
            balance_line: starter_line(s.balance_micros.unwrap_or(settings.publik_starter_micros)),
            justification: WHY_IT_COSTS.to_string(),
            cta_label: CTA_LINK_AND_PICK.to_string(),
            claim_url: s.claim_url.clone(),
            later_label: CTA_LATER.to_string(),
        });
    }
    if !settings.publik_cta_pending {
        s.first_run = None;
    }
    s
}

/// Only ever hand the system browser a publikhq.com link (CONTRACT §11.4).
fn open_publik_url(url: &str) {
    match api::publik_link(Some(url)) {
        Some(u) => open_url(&u),
        None => eprintln!("[whimpr] refused to open a link off publikhq.com"),
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

/// The disclosure card's "Turn on publik API": the one place a key is minted.
/// Mints if needed, stamps the disclosure version, selects the mode.
#[tauri::command]
pub async fn publik_accept_disclosure() -> Result<PublikStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        provision_now()?;
        let mut settings = crate::hotkey::current_settings();
        settings.cleanup_mode = CleanupMode::Publik;
        crate::hotkey::update_settings(settings);
        refresh_wallet(true);
        Ok(snapshot())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "Forget this key": drops the publik key and leaves the mode where the
/// caller puts it. "Use my own key instead" does NOT call this — the key
/// stays for when they come back.
#[tauri::command]
pub fn publik_forget_key() -> PublikStatus {
    forget_key();
    crate::hotkey::rebuild_providers();
    snapshot()
}

/// "Later" on the first-run card (and the primary button, once tapped): the
/// card is no longer owed. The key stays; the mode stays; nothing else moves.
#[tauri::command]
pub fn publik_dismiss_first_run() -> PublikStatus {
    let mut settings = crate::hotkey::current_settings();
    dismiss_first_run_in(&mut settings);
    crate::hotkey::update_settings(settings);
    state().lock().unwrap().first_run = None;
    snapshot()
}

/// Close the non-blocking banner. A later 402 or a new dip re-raises it.
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
        // The first-run card's primary button: claim_url, else the dashboard
        // (which has a claim box). Tapping it settles the card.
        "first_run" => {
            let u = s.first_run.as_ref().and_then(|c| c.claim_url.clone()).or_else(|| s.claim_url.clone());
            let _ = publik_dismiss_first_run();
            u.unwrap_or_else(|| s.dashboard_url.clone())
        }
        "plan" => s.plan_cta.url.clone(),
        "notice" => s.notice.as_ref().map(|n| n.link_url.clone()).unwrap_or_else(|| s.dashboard_url.clone()),
        "top_up" => s
            .top_up_url
            .clone()
            .or_else(|| s.claim_url.clone())
            .unwrap_or_else(|| s.dashboard_url.clone()),
        "terms" => api::TERMS_URL.to_string(),
        _ => s.dashboard_url.clone(),
    };
    open_publik_url(&url);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// In-memory store pre-seeded like a user who typed their own keys.
    struct MemStore(RefCell<BTreeMap<String, String>>);
    impl MemStore {
        fn seeded() -> Self {
            let mut m = BTreeMap::new();
            m.insert("openai_api_key".to_string(), "sk-user-typed-this".to_string());
            m.insert("anthropic_api_key".to_string(), "sk-ant-user-typed-this".to_string());
            Self(RefCell::new(m))
        }
        fn get_raw(&self, k: &str) -> Option<String> {
            self.0.borrow().get(k).cloned()
        }
    }
    impl KeyStore for MemStore {
        fn get(&self, account: &str) -> Option<String> {
            self.0.borrow().get(account).cloned().filter(|k| !k.trim().is_empty())
        }
        fn set(&self, account: &str, value: &str) -> Result<(), String> {
            self.0.borrow_mut().insert(account.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, account: &str) {
            self.0.borrow_mut().remove(account);
        }
    }

    fn minted() -> Provisioned {
        serde_json::from_str(
            r#"{"install_id":"3f1c9b5e-7a2d-4c8e-9f0b-1d2e3f4a5b6c","key":"pk_live_a8k2m9x4q7v1_h3n6r9t2w5y8z1b4c7d0f3g6j9k2m5p8",
                "base_url":"https://api.publikhq.com/v1/","models":{"fast":"publik-fast"},
                "claim_url":"https://publikhq.com/claim/HK7F-2QWD","balance_micros":250000}"#,
        )
        .unwrap()
    }

    #[test]
    fn never_overwrites_a_user_entered_key() {
        // Rung 1: PUBLIK_API_KEY in the environment wins and mint never runs.
        let store = MemStore::seeded();
        let mut settings = Settings::default();
        let (k, p) = ensure_key_with(Some(" pk_live_from_env "), &store, None, &mut settings, |_| panic!("mint must not run")).unwrap();
        assert_eq!(k, "pk_live_from_env");
        assert!(p.is_none());

        // Rung 2: a key already in the keychain wins and mint never runs.
        store.set(ACCOUNT, "pk_live_earlier_mint").unwrap();
        let (k, _) = ensure_key_with(None, &store, None, &mut settings, |_| panic!("mint must not run")).unwrap();
        assert_eq!(k, "pk_live_earlier_mint");
        assert_eq!(store.get_raw(ACCOUNT).as_deref(), Some("pk_live_earlier_mint"));

        // Rung 3: the convention file wins over provisioning.
        store.delete(ACCOUNT);
        let dir = std::env::temp_dir().join(format!("whimpr-publik-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("whimprflow.json");
        std::fs::write(&file, r#"{"key":"pk_live_from_iris_file","install_id":"x"}"#).unwrap();
        let (k, _) = ensure_key_with(None, &store, Some(&file), &mut settings, |_| panic!("mint must not run")).unwrap();
        assert_eq!(k, "pk_live_from_iris_file");
        // A file whose key is not a pk_ key is ignored (and then we would mint).
        std::fs::write(&file, r#"{"key":"sk-not-ours"}"#).unwrap();
        assert!(read_key_from(None, &store, Some(&file)).is_none());
        let _ = std::fs::remove_dir_all(&dir);

        // Through every rung, the user's own keys were never touched.
        assert_eq!(store.get_raw("openai_api_key").as_deref(), Some("sk-user-typed-this"));
        assert_eq!(store.get_raw("anthropic_api_key").as_deref(), Some("sk-ant-user-typed-this"));
    }

    #[test]
    fn provisioning_writes_only_the_publik_account_and_honours_the_response() {
        let store = MemStore::seeded();
        let mut settings = Settings::default();
        let mut ran = false;
        let (k, p) = ensure_key_with(None, &store, None, &mut settings, |s| {
            ran = true;
            s.publik_install_id = "3f1c9b5e-7a2d-4c8e-9f0b-1d2e3f4a5b6c".to_string();
            Ok(minted())
        })
        .unwrap();
        assert!(ran);
        assert!(p.is_some());
        assert_eq!(k, "pk_live_a8k2m9x4q7v1_h3n6r9t2w5y8z1b4c7d0f3g6j9k2m5p8");
        assert_eq!(store.get_raw(ACCOUNT).as_deref(), Some(k.as_str()));
        // BYO accounts untouched; nothing else was written.
        assert_eq!(store.get_raw("openai_api_key").as_deref(), Some("sk-user-typed-this"));
        assert_eq!(store.get_raw("anthropic_api_key").as_deref(), Some("sk-ant-user-typed-this"));
        assert_eq!(store.0.borrow().len(), 3);
        // S8: base_url from the response is honoured (trailing slash dropped),
        // the fast alias and claim link are kept, the disclosure is stamped.
        assert_eq!(settings.publik_base_url, "https://api.publikhq.com/v1");
        assert_eq!(settings.publik_model, "publik-fast");
        assert_eq!(settings.publik_claim_url, "https://publikhq.com/claim/HK7F-2QWD");
        assert_eq!(settings.publik_install_id, "3f1c9b5e-7a2d-4c8e-9f0b-1d2e3f4a5b6c");
        assert_eq!(settings.publik_disclosure_version, PUBLIK_DISCLOSURE_VERSION);
        // The user's own BYO settings are untouched.
        assert_eq!(settings.openai_base_url, "");
        assert_eq!(settings.openai_model, "gpt-4o-mini");
        // And the mode is still whatever it was — only the user's tap selects Publik.
        assert_eq!(settings.cleanup_mode, CleanupMode::Local);
    }

    #[test]
    fn a_replay_without_a_key_is_an_error_not_a_blank_key() {
        let store = MemStore::seeded();
        let mut settings = Settings::default();
        let replay: Provisioned = serde_json::from_str(r#"{"install_id":"x","key":null,"starter_micros":0,"claim_state":"anonymous"}"#).unwrap();
        let err = ensure_key_with(None, &store, None, &mut settings, |_| Ok(replay)).unwrap_err();
        assert!(err.contains("did not return a key"), "{err}");
        assert!(store.get_raw(ACCOUNT).is_none());
    }

    #[test]
    fn install_ids_are_v4_uuids_and_unique() {
        let a = new_install_id();
        let b = new_install_id();
        assert_ne!(a, b);
        for id in [&a, &b] {
            assert_eq!(id.len(), 36, "{id}");
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12], "{id}");
            assert!(id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()), "{id}");
            assert_eq!(&parts[2][..1], "4", "version nibble: {id}");
            assert!(matches!(&parts[3][..1], "8" | "9" | "a" | "b"), "variant: {id}");
        }
    }

    #[test]
    fn the_402_notice_renders_the_message_and_exactly_one_link() {
        let msg = "Not enough publik credit for this request. Link this computer and pick a plan at the link below, or use your own key.";
        let n = exhausted_notice(msg, Some("anonymous"), "https://publikhq.com/claim/HK7F-2QWD");
        assert_eq!(n.kind, "exhausted");
        assert!(n.message.starts_with(msg), "{}", n.message);
        // The message carries no URL of its own; the one link is the field.
        assert_eq!(n.message.matches("http").count(), 0, "{}", n.message);
        assert_eq!(n.link_url, "https://publikhq.com/claim/HK7F-2QWD");
        assert_eq!(n.link_label, "Link this computer & pick a plan");
        assert!(n.message.contains("Dictation still works"));

        let claimed = exhausted_notice("m", Some("claimed"), "https://publikhq.com/dashboard/api/add");
        assert_eq!(claimed.link_url, "https://publikhq.com/dashboard/api/add");
        assert_eq!(claimed.link_label, "Add a plan or pack");

        // A 402 whose top_up_url is off publikhq.com is not followed: the
        // dashboard is the one link instead.
        let off = exhausted_notice("m", Some("anonymous"), "https://evil.example/claim/HK7F-2QWD");
        assert_eq!(off.link_url, api::DASHBOARD_URL);
    }

    // ── CONTRACT §12: the card, the button, the banner ──────────────────────

    #[test]
    fn the_first_run_card_carries_the_responses_balance_and_claim_url() {
        let p = minted();
        let card = first_run_card(&p);
        // (a) the balance line comes from the response, in dollars.
        assert_eq!(card.balance_line, "$0.25 of free starter usage");
        let ten_cents: Provisioned = serde_json::from_str(r#"{"key":"pk_live_x","starter_micros":100000,"claim_url":"https://publikhq.com/claim/AAAA-BBBB"}"#).unwrap();
        assert_eq!(first_run_card(&ten_cents).balance_line, "$0.10 of free starter usage");
        // (b) the one justification, verbatim.
        assert_eq!(card.justification, WHY_IT_COSTS);
        // (c) the primary button opens the response's claim_url; "Later" keeps the starter.
        assert_eq!(card.cta_label, "Link this computer & pick a plan");
        assert_eq!(card.claim_url.as_deref(), Some("https://publikhq.com/claim/HK7F-2QWD"));
        assert_eq!(card.later_label, "Later");
        // The card exists as JSON for the Hub with exactly these keys.
        let json = serde_json::to_value(&card).unwrap();
        for k in ["balance_line", "justification", "cta_label", "claim_url", "later_label"] {
            assert!(json.get(k).is_some(), "missing {k}");
        }
    }

    #[test]
    fn links_off_publikhq_com_are_dropped_everywhere() {
        // The mint's claim_url: dropped by the parser, and dropped again here.
        let mut p = minted();
        p.claim_url = Some("https://publikhq.com.evil.example/claim/HK7F-2QWD".to_string());
        assert_eq!(first_run_card(&p).claim_url, None);
        p.claim_url = Some("http://publikhq.com/claim/HK7F-2QWD".to_string()); // not https
        assert_eq!(first_run_card(&p).claim_url, None);
        p.claim_url = Some("https://publikhq.com/claim/HK7F-2QWD".to_string());
        assert!(first_run_card(&p).claim_url.is_some());

        // The settings button never points anywhere but publikhq.com.
        let cta = plan_cta(Some("anonymous"), Some("https://phish.example/claim/HK7F-2QWD"));
        assert_eq!(cta.url, api::DASHBOARD_URL);
        assert_eq!(cta.label, "Pick a plan");
        let cta = plan_cta(Some("anonymous"), Some("https://publikhq.com/claim/HK7F-2QWD"));
        assert_eq!(cta.url, "https://publikhq.com/claim/HK7F-2QWD");
        assert!(!cta.claimed);
        let cta = plan_cta(Some("claimed"), Some("https://publikhq.com/claim/HK7F-2QWD"));
        assert_eq!(cta.label, "Manage plan");
        assert_eq!(cta.url, "https://publikhq.com/dashboard/api");
        assert!(cta.claimed);

        // The low-starter banner's one link, likewise.
        let n = low_starter_notice(10_000, 250_000, Some("https://not-publik.example/x")).unwrap();
        assert_eq!(n.link_url, api::DASHBOARD_URL);
        let n = low_starter_notice(10_000, 250_000, Some("https://publikhq.com/claim/HK7F-2QWD")).unwrap();
        assert_eq!(n.link_url, "https://publikhq.com/claim/HK7F-2QWD");
        for u in [cta.url.as_str(), n.link_url.as_str()] {
            assert!(u.starts_with(api::LINK_HOST_PREFIX), "{u}");
        }
    }

    #[test]
    fn later_keeps_the_key_and_the_mode_and_only_settles_the_card() {
        let store = MemStore::seeded();
        let mut settings = Settings::default();
        let (k, p) = ensure_key_with(None, &store, None, &mut settings, |_| Ok(minted())).unwrap();
        let card = first_run_card(p.as_ref().unwrap());
        // Provisioning owes the card and remembers the grant.
        assert!(settings.publik_cta_pending);
        assert_eq!(settings.publik_starter_micros, 250_000);
        assert_eq!(card.claim_url.as_deref(), Some("https://publikhq.com/claim/HK7F-2QWD"));

        let before_store = store.0.borrow().clone();
        let before_mode = settings.cleanup_mode;
        let before_claim = settings.publik_claim_url.clone();
        dismiss_first_run_in(&mut settings);

        // "Later": the card is settled, and that is all that changed.
        assert!(!settings.publik_cta_pending);
        assert_eq!(*store.0.borrow(), before_store, "the key store was not touched");
        assert_eq!(store.get_raw(ACCOUNT).as_deref(), Some(k.as_str()), "the publik key is still there");
        assert_eq!(settings.cleanup_mode, before_mode);
        assert_eq!(settings.publik_claim_url, before_claim, "the claim link is kept for the settings button");
        assert_eq!(settings.publik_starter_micros, 250_000);
        assert_eq!(settings.publik_disclosure_version, PUBLIK_DISCLOSURE_VERSION);
    }

    #[test]
    fn the_starter_banner_appears_below_a_fifth_and_only_then() {
        // 20% of $0.25 is $0.05: at or above it, nothing; below it, the banner.
        assert!(low_starter_notice(50_000, 250_000, None).is_none());
        assert!(low_starter_notice(250_000, 250_000, None).is_none());
        let n = low_starter_notice(49_999, 250_000, None).unwrap();
        assert_eq!(n.kind, "low_starter");
        assert!(n.message.starts_with("$0.05 of your free starter usage is left."), "{}", n.message);
        assert!(n.message.contains(WHY_IT_COSTS));
        assert_eq!(n.message.matches("http").count(), 0, "the one link is the field, not the text");
        assert_eq!(n.link_url, api::DASHBOARD_URL);
        // Nothing left is the 402's job, not this banner's.
        assert!(low_starter_notice(0, 250_000, None).is_none());
        // An unknown grant falls back to the documented anonymous starter.
        assert!(low_starter_notice(60_000, 0, None).is_none());
        assert!(low_starter_notice(40_000, 0, None).is_some());
    }

    /// The copy rule on the CTA surfaces specifically (CONTRACT §12.5): the
    /// texts the card and banners render are checked as values, not only as
    /// source lines, and the money words are dollars.
    #[test]
    fn cta_copy_says_publik_api_in_dollars_and_never_credits() {
        let p = minted();
        let card = first_run_card(&p);
        let n = exhausted_notice("Not enough publik credit for this request.", Some("anonymous"), "https://publikhq.com/claim/HK7F-2QWD");
        let low = low_starter_notice(10_000, 250_000, None).unwrap();
        let pick = plan_cta(Some("anonymous"), None);
        let manage = plan_cta(Some("claimed"), None);
        let texts = [
            card.balance_line.as_str(),
            card.justification.as_str(),
            card.cta_label.as_str(),
            card.later_label.as_str(),
            n.headline.as_str(),
            n.message.as_str(),
            n.link_label.as_str(),
            low.headline.as_str(),
            low.message.as_str(),
            low.link_label.as_str(),
            pick.label.as_str(),
            manage.label.as_str(),
            WHY_IT_COSTS,
        ];
        for t in texts {
            let lower = t.to_ascii_lowercase();
            assert!(!t.contains("OpenAI API access"), "{t}");
            assert!(!t.contains("ChatGPT"), "{t}");
            assert!(!lower.contains("credits"), "'credits' as a unit in: {t}");
            assert!(!lower.contains("token"), "tokens, not dollars, in: {t}");
            assert!(!lower.contains("openai") && !lower.contains("anthropic"), "provider named in: {t}");
            assert!(!lower.contains("publik credits") && !lower.contains("publik api credits"), "{t}");
        }
        assert!(card.balance_line.starts_with('$'));
        assert!(card.justification.contains("half the provider's list price"));
        assert!(card.justification.contains("Nothing is charged behind your back"));
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
