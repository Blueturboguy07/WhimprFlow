// Typed wrappers over the Tauri command surface. In a plain browser (vite dev
// without the shell) the invoke import fails and we fall back to defaults so the
// Hub still renders for iteration.

export type CleanupMode = "raw" | "local" | "publik" | "open_ai" | "anthropic";
export type CleanupLevel = "none" | "light" | "medium" | "high";

export interface Settings {
  cleanup_mode: CleanupMode;
  cleanup_level: CleanupLevel;
  openai_model: string;
  // API root for "OpenAI" mode — leave blank for OpenAI itself, or point at
  // an OpenAI-compatible endpoint like OpenRouter (https://openrouter.ai/api/v1).
  openai_base_url: string;
  anthropic_model: string;
  sound_on_start: boolean;
  // Tauri accelerator that toggles hands-free (locked) dictation — press once to
  // start talking with no key held, again to stop. Default "CmdOrCtrl+Shift+Space".
  // Empty disables it. (Holding Fn and double-tapping Fn always work too.)
  hands_free_hotkey: string;
  // publik API (the pre-provisioned cloud option). Persisted by Rust; the Hub
  // never edits these directly — see `publikAcceptDisclosure`.
  publik_disclosure_version: number;
  publik_install_id: string;
  publik_base_url: string;
  publik_model: string;
  publik_claim_url: string;
}

// Mirrors `permissions::Grant` in src-tauri. A bare boolean couldn't tell
// "nobody has asked yet" from "asked and turned down" — two states with
// completely different instructions for the reader.
export type Grant = "granted" | "not_asked" | "refused";

export interface Status {
  accessibility: boolean;
  microphone: boolean;
  input_monitoring: boolean;
  microphone_grant: Grant;
  // The app macOS is actually judging our microphone request as, when that
  // isn't us (a terminal that launched us, say). Null in the normal case.
  charged_to: string | null;
  // One sentence saying why the microphone row can't go green, when there's
  // something the reader couldn't otherwise have known. Null when there isn't.
  microphone_hint: string | null;
  has_openai_key: boolean;
  has_anthropic_key: boolean;
  has_publik_key: boolean;
  // The on-device worker AND a model are on disk. When false, "Local" pastes
  // the transcript as spoken — Settings says so instead of pretending.
  local_model_present: boolean;
}

// What the Hub falls back to before the first read lands (and in a plain
// browser preview, where there's no shell to ask).
export const UNKNOWN_STATUS: Status = {
  accessibility: false,
  microphone: false,
  input_monitoring: false,
  microphone_grant: "not_asked",
  charged_to: null,
  microphone_hint: null,
  has_openai_key: false,
  has_anthropic_key: false,
  has_publik_key: false,
  local_model_present: false,
};

export interface StatsSummary {
  total_words: number;
  total_sessions: number;
  total_speaking_secs: number;
  avg_wpm: number;
  best_wpm: number;
  words_today: number;
  wpm_today: number;
  day_streak: number;
  time_saved_secs: number;
  last7_words: number[];
}

export const EMPTY_STATS: StatsSummary = {
  total_words: 0,
  total_sessions: 0,
  total_speaking_secs: 0,
  avg_wpm: 0,
  best_wpm: 0,
  words_today: 0,
  wpm_today: 0,
  day_streak: 0,
  time_saved_secs: 0,
  last7_words: [0, 0, 0, 0, 0, 0, 0],
};

export const DEFAULT_SETTINGS: Settings = {
  // Matches the Rust default (`CleanupMode::Local`): WhimprFlow is local-first.
  cleanup_mode: "local",
  cleanup_level: "light",
  openai_model: "gpt-4o-mini",
  openai_base_url: "",
  anthropic_model: "claude-haiku-4-5",
  sound_on_start: true,
  hands_free_hotkey: "CmdOrCtrl+Shift+Space",
  publik_disclosure_version: 0,
  publik_install_id: "",
  publik_base_url: "",
  publik_model: "",
  publik_claim_url: "",
};

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export async function getSettings(): Promise<Settings> {
  try {
    return await invoke<Settings>("get_settings");
  } catch {
    return DEFAULT_SETTINGS;
  }
}

export async function setSettings(settings: Settings): Promise<void> {
  try {
    await invoke<void>("set_settings", { settings });
  } catch {
    /* browser preview — no-op */
  }
}

export async function getStatus(): Promise<Status> {
  try {
    return await invoke<Status>("get_status");
  } catch {
    return UNKNOWN_STATUS;
  }
}

// The permission heartbeat, pushed from Rust (`permissions::watch`) the moment
// macOS changes its mind. This is what makes the setup screen's promise —
// "turns green the moment macOS applies it, no relaunch needed" — true even
// when the Hub's own timer isn't running, which is exactly when the reader is
// off in System Settings doing the granting. Payload is the permission half of
// `Status`; the key fields ride along unchanged.
export type Permissions = Pick<
  Status,
  | "accessibility"
  | "microphone"
  | "input_monitoring"
  | "microphone_grant"
  | "charged_to"
  | "microphone_hint"
>;

export async function onPermissions(cb: (p: Permissions) => void): Promise<() => void> {
  try {
    const { listen } = await import("@tauri-apps/api/event");
    return await listen<Permissions>("whimpr://permissions", (e) => cb(e.payload));
  } catch {
    return () => {};
  }
}

// The most recent loud diagnostic from the dictation pipeline (permission
// missing, hotkey tap dead, paste failed, empty transcript, …). Mirrors
// `diag::ErrorDto` in src-tauri/src/diag.rs.
export interface LastError {
  headline: string;
  detail: string;
}

export async function getLastError(): Promise<LastError | null> {
  try {
    return await invoke<LastError | null>("get_last_error");
  } catch {
    return null;
  }
}

export async function getStats(): Promise<StatsSummary> {
  try {
    const tz = new Date().getTimezoneOffset(); // minutes to add to local -> UTC
    return await invoke<StatsSummary>("get_stats", { tzOffsetMinutes: tz });
  } catch {
    return EMPTY_STATS;
  }
}

export async function requestMicrophone(): Promise<void> {
  try {
    await invoke<void>("request_microphone");
  } catch {
    /* browser preview */
  }
}

export async function requestAccessibility(): Promise<void> {
  try {
    await invoke<void>("request_accessibility");
  } catch {
    /* browser preview */
  }
}

export async function requestInputMonitoring(): Promise<void> {
  try {
    await invoke<void>("request_input_monitoring");
  } catch {
    /* browser preview */
  }
}

export async function setApiKey(provider: "openai" | "anthropic", key: string): Promise<void> {
  try {
    await invoke<void>("set_api_key", { provider, key });
  } catch {
    /* browser preview */
  }
}

// ── publik API ───────────────────────────────────────────────────────────────
// Mirrors `publik::PublikStatus` in src-tauri/src/publik.rs.
export interface PublikStatus {
  available: boolean;
  has_key: boolean;
  balance_micros: number | null;
  balance_label: string | null;
  last_charge_label: string | null;
  week_label: string | null;
  claim_state: string | null;
  claim_url: string | null;
  top_up_url: string | null;
  dashboard_url: string;
  model: string;
  exhausted: boolean;
  disconnected: boolean;
  unreachable: boolean;
  disclosure_needed: boolean;
  can_provision: boolean;
}

export const UNKNOWN_PUBLIK: PublikStatus = {
  available: false,
  has_key: false,
  balance_micros: null,
  balance_label: null,
  last_charge_label: null,
  week_label: null,
  claim_state: null,
  claim_url: null,
  top_up_url: null,
  dashboard_url: "https://publikhq.com/dashboard/api",
  model: "publik-fast",
  exhausted: false,
  disconnected: false,
  unreachable: false,
  disclosure_needed: true,
  can_provision: false,
};

export async function getPublikStatus(): Promise<PublikStatus> {
  try {
    return await invoke<PublikStatus>("get_publik_status");
  } catch {
    return UNKNOWN_PUBLIK;
  }
}

// GET /wallet, throttled in Rust — the Settings pane calls it on open.
export async function publikRefreshWallet(force = false): Promise<PublikStatus> {
  try {
    return await invoke<PublikStatus>("publik_refresh_wallet", { force });
  } catch {
    return UNKNOWN_PUBLIK;
  }
}

// "Turn on publik API": mints the key if needed (this is the ONE network call
// that provisions), stamps the disclosure, selects the mode. Errors surface —
// the card shows them.
export async function publikAcceptDisclosure(): Promise<PublikStatus> {
  return invoke<PublikStatus>("publik_accept_disclosure");
}

export async function publikForgetKey(): Promise<PublikStatus> {
  try {
    return await invoke<PublikStatus>("publik_forget_key");
  } catch {
    return UNKNOWN_PUBLIK;
  }
}

export async function publikOpenLink(kind: "claim" | "dashboard" | "terms" | "top_up"): Promise<void> {
  try {
    await invoke<void>("publik_open_link", { kind });
  } catch {
    /* browser preview */
  }
}

// Pushed from Rust after every cleanup (balance headers) and on 402/401.
export async function onPublik(cb: (p: PublikStatus) => void): Promise<() => void> {
  try {
    const { listen } = await import("@tauri-apps/api/event");
    return await listen<PublikStatus>("whimpr://publik", (e) => cb(e.payload));
  } catch {
    return () => {};
  }
}

// ── History ────────────────────────────────────────────────────────────────
export interface HistoryItem {
  ts_unix: number;
  text: string;
  app: string | null;
  words: number;
}

export async function getHistory(): Promise<HistoryItem[]> {
  try {
    return await invoke<HistoryItem[]>("get_history");
  } catch {
    return [];
  }
}

// ── Dictionary ───────────────────────────────────────────────────────────────
export interface DictEntry {
  correct: string;
  mishears: string[];
  auto: boolean;
}

export async function getDictionary(): Promise<DictEntry[]> {
  try {
    return await invoke<DictEntry[]>("get_dictionary");
  } catch {
    return [];
  }
}

export async function addDictionaryEntry(correct: string, mishears: string[]): Promise<void> {
  try {
    await invoke<void>("add_dictionary_entry", { correct, mishears });
  } catch {
    /* browser preview — no-op */
  }
}

export async function removeDictionaryEntry(correct: string): Promise<void> {
  try {
    await invoke<void>("remove_dictionary_entry", { correct });
  } catch {
    /* browser preview — no-op */
  }
}

