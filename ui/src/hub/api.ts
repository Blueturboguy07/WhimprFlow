// Typed wrappers over the Tauri command surface. In a plain browser (vite dev
// without the shell) the invoke import fails and we fall back to defaults so the
// Hub still renders for iteration.

export type CleanupMode = "raw" | "local" | "open_ai" | "anthropic";
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
  // Keep the Flow Bar visible when idle. Off = pill only appears while dictating.
  show_pill_always: boolean;
  // Gap in points between the screen bottom and the pill (clears the Dock).
  pill_bottom_inset: number;
  // User-dragged pill position in physical pixels; null = computed anchor.
  pill_pos: [number, number] | null;
  // Follow the display the frontmost window is on rather than pinning to primary.
  pill_follows_active_display: boolean;
  // Whisper language code, or "auto" to detect. Needs a multilingual model.
  language: string;
  push_to_talk_key: PushToTalkKey;
  launch_at_login: boolean;
  show_in_dock: boolean;
  // Input device name; "" means the system default.
  microphone: string;
  // Free-text style preferences appended to the cleanup prompt.
  style_instructions: string;
}

export type PushToTalkKey = "fn" | "right_command" | "right_option" | "right_control";

export const PTT_KEYS: { value: PushToTalkKey; label: string }[] = [
  { value: "fn", label: "Fn / Globe" },
  { value: "right_command", label: "Right ⌘" },
  { value: "right_option", label: "Right ⌥" },
  { value: "right_control", label: "Right ⌃" },
];

// Whisper's own language codes. "auto" lets the model detect, which costs a
// little accuracy but handles code-switching mid-sentence.
export const LANGUAGES: { value: string; label: string }[] = [
  { value: "en", label: "English" },
  { value: "hi", label: "Hindi" },
  { value: "gu", label: "Gujarati" },
  { value: "es", label: "Spanish" },
  { value: "fr", label: "French" },
  { value: "de", label: "German" },
  { value: "pt", label: "Portuguese" },
  { value: "it", label: "Italian" },
  { value: "nl", label: "Dutch" },
  { value: "ja", label: "Japanese" },
  { value: "ko", label: "Korean" },
  { value: "zh", label: "Chinese" },
  { value: "ar", label: "Arabic" },
  { value: "ru", label: "Russian" },
  { value: "auto", label: "Detect automatically" },
];

export async function listMicrophones(): Promise<string[]> {
  try {
    return await invoke<string[]>("list_microphones");
  } catch {
    return [];
  }
}

export interface Status {
  accessibility: boolean;
  microphone: boolean;
  input_monitoring: boolean;
  has_openai_key: boolean;
  has_anthropic_key: boolean;
}

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
  cleanup_mode: "open_ai",
  cleanup_level: "light",
  openai_model: "gpt-4o-mini",
  openai_base_url: "",
  anthropic_model: "claude-haiku-4-5",
  sound_on_start: true,
  show_pill_always: true,
  pill_bottom_inset: 96,
  pill_pos: null,
  pill_follows_active_display: true,
  language: "en",
  push_to_talk_key: "fn",
  launch_at_login: false,
  show_in_dock: true,
  microphone: "",
  style_instructions: "",
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
    return {
      accessibility: false,
      microphone: false,
      input_monitoring: false,
      has_openai_key: false,
      has_anthropic_key: false,
    };
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

/// Quit and relaunch. macOS fixes an app's microphone authorisation at launch,
/// so a grant made while running only takes effect after a restart.
export async function restartApp(): Promise<void> {
  try {
    await invoke<void>("restart_app");
  } catch {
    /* browser preview — no-op */
  }
}

/// Persist a pill position the user dragged to (physical px, window top-left).
export async function setPillPosition(x: number, y: number): Promise<void> {
  try {
    await invoke<void>("set_pill_position", { x, y });
  } catch {
    /* browser preview — no-op */
  }
}

/// Forget the dragged position; the pill returns to its computed anchor.
export async function resetPillPosition(): Promise<void> {
  try {
    await invoke<void>("reset_pill_position");
  } catch {
    /* browser preview — no-op */
  }
}

/// Copy text to the system clipboard. Routed through the Rust side (arboard)
/// rather than navigator.clipboard, which needs a secure context and a user
/// gesture the webview does not always report.
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    await invoke<void>("copy_to_clipboard", { text });
    return true;
  } catch {
    // Browser preview fallback.
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      return false;
    }
  }
}

// ── Transforms ───────────────────────────────────────────────────────────────
export interface Transform {
  id: string;
  name: string;
  triggers: string[];
  prompt: string;
  enabled: boolean;
}

export async function getTransforms(): Promise<Transform[]> {
  try {
    return await invoke<Transform[]>("get_transforms");
  } catch {
    return [];
  }
}

export async function setTransformEnabled(id: string, enabled: boolean): Promise<void> {
  try {
    await invoke<void>("set_transform_enabled", { id, enabled });
  } catch {
    /* browser preview */
  }
}

// ── Snippets ─────────────────────────────────────────────────────────────────
export interface Snippet {
  trigger: string;
  expansion: string;
}

export async function getSnippets(): Promise<Snippet[]> {
  try {
    return await invoke<Snippet[]>("get_snippets");
  } catch {
    return [];
  }
}

export async function addSnippet(trigger: string, expansion: string): Promise<void> {
  try {
    await invoke<void>("add_snippet", { trigger, expansion });
  } catch {
    /* browser preview */
  }
}

export async function removeSnippet(trigger: string): Promise<void> {
  try {
    await invoke<void>("remove_snippet", { trigger });
  } catch {
    /* browser preview */
  }
}

// ── Scratchpad ───────────────────────────────────────────────────────────────
export async function getScratchpad(): Promise<string> {
  try {
    return await invoke<string>("get_scratchpad");
  } catch {
    return "";
  }
}

export async function setScratchpad(text: string): Promise<void> {
  try {
    await invoke<void>("set_scratchpad", { text });
  } catch {
    /* browser preview */
  }
}

export async function getScratchpadCapture(): Promise<boolean> {
  try {
    return await invoke<boolean>("get_scratchpad_capture");
  } catch {
    return false;
  }
}

export async function setScratchpadCapture(on: boolean): Promise<void> {
  try {
    await invoke<void>("set_scratchpad_capture", { on });
  } catch {
    /* browser preview */
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

