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

/** "Not now" on the Home card: kept per computer, so the card does not come back on each launch. */
export const FRONT_LATER_KEY = "whimpr_publik_front_later";

/**
 * What the Home screen (and the setup screen) show about publik:
 * - "first_run": publik was just connected; show the card with the balance and "Link this computer & pick a plan".
 * - "connect": publik API is not on yet; show "WhimprFlow runs on publik API" with one "Connect publik" button.
 * - "reconnect": this computer was removed from the publik account; same card, "Connect publik again".
 * - "none": publik is on, the person uses their own key, publik is not in this build, or nothing was read yet.
 */
export type PublikFront = "first_run" | "connect" | "reconnect" | "none";

export function publikFront(
  settings: Pick<Settings, "cleanup_mode">,
  publik: Pick<PublikStatus, "available" | "has_key" | "disconnected" | "first_run" | "can_provision">,
  status: Pick<Status, "has_openai_key" | "has_anthropic_key">,
): PublikFront {
  if (publik.first_run) return "first_run";
  if (!publik.available) return "none";
  const mode = settings.cleanup_mode;
  if (mode === "open_ai" && status.has_openai_key) return "none";
  if (mode === "anthropic" && status.has_anthropic_key) return "none";
  if (publik.disconnected) return "reconnect";
  if (mode === "publik" && publik.has_key) return "none";
  return "connect";
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
