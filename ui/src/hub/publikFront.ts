// publik first: the first thing a new person sees says that WhimprFlow runs on publik API with no key, with one
// "Connect publik" button (the existing disclosure + `publik_accept_disclosure`). Own key and Local stay in Settings.
// Pure, so it is unit tested (publikFront.test.ts, `pnpm test`).

import type { PublikStatus, Settings, Status } from "./api";

export const PUBLIK_FRONT = {
  title: "WhimprFlow runs on publik API",
  lead: "No key needed. publik API cleans up your words before they are typed: filler words out, grammar fixed.",
  // the setup screen on macOS has little room: one line, then the button; the button opens the full card (cost, data)
  short: "No key needed. publik API cleans up your words before they are typed.",
  cost:
    "Pay per use from your publik balance. Most people spend under $2 a month. Linking this computer to your publik account gives $0.05 of free use, once.",
  data: "Your transcript (never audio) goes through publik's servers to a shared model account. publik never trains on it and does not store it.",
  button: "Connect publik",
  reconnect: "Connect publik again",
  ownKey: "Use my own key instead",
  later: "Not now",
} as const;

/** The Help tip "Pick a cleanup engine": publik first, in the card's words. */
export function cleanupEngineTip(keychain: string): string {
  return `${PUBLIK_FRONT.title}. ${PUBLIK_FRONT.lead} Under Settings → Cleanup Engine, press ${PUBLIK_FRONT.button}. ${PUBLIK_FRONT.cost} You can also run fully offline (Local, with a model on this computer), paste exactly what you said (Raw), or use your own OpenAI or Anthropic key ("Use my own key"). Keys are stored in your ${keychain}.`;
}

// ------------------------------------------------------------------ "Not now", one flag for Home, setup and Settings

/** "Not now" (and "Use my own key instead"): kept per computer, so the card does not come back on each launch. */
export const FRONT_LATER_KEY = "whimpr_publik_front_later";
const laterListeners = new Set<() => void>();

export function frontLater(): boolean {
  try {
    return globalThis.localStorage?.getItem(FRONT_LATER_KEY) === "1";
  } catch {
    return false;
  }
}

export function setFrontLater(): void {
  try {
    globalThis.localStorage?.setItem(FRONT_LATER_KEY, "1");
  } catch {
    /* per-computer convenience only */
  }
  for (const f of laterListeners) f();
}

export function subscribeFrontLater(f: () => void): () => void {
  laterListeners.add(f);
  return () => {
    laterListeners.delete(f);
  };
}

// ------------------------------------------------------------------ what shows

/**
 * What the Home screen, the setup screen and Settings show about publik:
 * - "first_run": publik was just connected; show the card with the balance and "Link this computer & pick a plan".
 * - "connect": no cleanup engine works yet (Local with no model on this computer, an own-key mode with no key, or
 *   publik picked with no key); show "WhimprFlow runs on publik API" with one "Connect publik" button.
 * - "reconnect": publik is the engine, but this computer was removed from the publik account.
 * - "none": a cleanup engine works (publik, Local with a model, an own key), Raw was picked on purpose, publik is not
 *   in this build, or nothing was read yet.
 */
export type PublikFront = "first_run" | "connect" | "reconnect" | "none";

type FrontPublik = Pick<PublikStatus, "available" | "has_key" | "disconnected" | "first_run" | "can_provision">;
type FrontStatus = Pick<Status, "has_openai_key" | "has_anthropic_key" | "local_model_present">;

export function publikFront(settings: Pick<Settings, "cleanup_mode">, publik: FrontPublik, status: FrontStatus): PublikFront {
  if (publik.first_run) return "first_run";
  if (!publik.available) return "none";
  switch (settings.cleanup_mode) {
    case "publik":
      if (publik.disconnected) return "reconnect";
      return publik.has_key ? "none" : "connect";
    case "raw":
      return "none";
    case "local":
      return status.local_model_present ? "none" : "connect";
    case "open_ai":
      return status.has_openai_key ? "none" : "connect";
    case "anthropic":
      return status.has_anthropic_key ? "none" : "connect";
    default:
      return "connect";
  }
}

const NOT_RUNNING: Record<Settings["cleanup_mode"], string> = {
  publik: "publik API is picked, but it is not connected yet",
  local: "Local is picked, but this computer has no Local model",
  raw: "",
  open_ai: "OpenAI is picked, but no OpenAI key is saved",
  anthropic: "Anthropic is picked, but no Anthropic key is saved",
};

/**
 * One line on the card while it asks to connect: the engine that is picked now and why it cannot run, so the card,
 * the picked engine in Settings and the badge say the same thing. null when the card does not ask to connect.
 */
export function frontNow(settings: Pick<Settings, "cleanup_mode">, publik: FrontPublik, status: FrontStatus): string | null {
  if (publikFront(settings, publik, status) !== "connect") return null;
  const why = NOT_RUNNING[settings.cleanup_mode];
  return why ? `Now: ${why}. Your words are typed exactly as you said them.` : null;
}

/**
 * The small badge next to "WhimprFlow" (sidebar, setup screen): the cleanup engine that works now, so it never says
 * "Local" next to "runs on publik API". Nothing works yet: no badge when publik is in this build (the card says
 * publik), "Local" in a build without publik (Local is the only built-in way there).
 */
export function engineBadge(settings: Pick<Settings, "cleanup_mode">, publik: FrontPublik, status: FrontStatus): string | null {
  switch (settings.cleanup_mode) {
    case "publik":
      if (publik.has_key && !publik.disconnected) return "publik API";
      break;
    case "local":
      if (status.local_model_present) return "Local";
      break;
    case "raw":
      return "Raw";
    case "open_ai":
      if (status.has_openai_key) return "OpenAI";
      break;
    case "anthropic":
      if (status.has_anthropic_key) return "Anthropic";
      break;
  }
  return publik.available ? null : "Local";
}

// ------------------------------------------------------------------ own-key fields in Settings

/**
 * The person's own choice to show or hide the own-key fields in Settings, kept per computer: "Use my own key instead"
 * (Home, the setup screen, Settings) opens them, and they stay open on the next visit. null: never chosen.
 */
export const OWN_KEYS_KEY = "whimpr_own_keys_open";

export function ownKeysChoice(): boolean | null {
  try {
    const v = globalThis.localStorage?.getItem(OWN_KEYS_KEY);
    return v === "1" ? true : v === "0" ? false : null;
  } catch {
    return null;
  }
}

export function setOwnKeysChoice(open: boolean): void {
  try {
    globalThis.localStorage?.setItem(OWN_KEYS_KEY, open ? "1" : "0");
  } catch {
    /* per-computer convenience only */
  }
}

/** Whether the own-key fields in Settings start open: only when the person already uses or set up their own key. */
export function ownKeysOpen(
  settings: Pick<Settings, "cleanup_mode" | "asr_mode">,
  status: Pick<Status, "has_openai_key" | "has_anthropic_key">,
): boolean {
  return (
    settings.cleanup_mode === "open_ai" ||
    settings.cleanup_mode === "anthropic" ||
    settings.asr_mode === "cloud" ||
    status.has_openai_key ||
    status.has_anthropic_key
  );
}
