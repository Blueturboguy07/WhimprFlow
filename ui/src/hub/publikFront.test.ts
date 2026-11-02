// publik first: what a new person sees first (Home, and the macOS setup screen), and whether the own-key fields in
// Settings start open. Run: `pnpm --dir ui test` (node --test, Node 22+ strips the types).

import { test } from "node:test";
import assert from "node:assert/strict";
import { PUBLIK_FRONT, ownKeysOpen, publikFront } from "./publikFront.ts";

const noKeys = { has_openai_key: false, has_anthropic_key: false };
const fresh = { available: true, has_key: false, disconnected: false, first_run: null, can_provision: true };
const firstRun = { balance_line: "Your balance is $0.00", justification: "", cta_label: "Link this computer & pick a plan", claim_url: null, later_label: "Later" };

test("first launch (Local default, no key, publik in this build): the card asks to connect publik", () => {
  assert.equal(publikFront({ cleanup_mode: "local" }, fresh, noKeys), "connect");
  assert.equal(publikFront({ cleanup_mode: "raw" }, fresh, noKeys), "connect");
  assert.equal(PUBLIK_FRONT.title, "WhimprFlow runs on publik API");
  assert.match(PUBLIK_FRONT.lead, /^No key needed\./);
  assert.equal(PUBLIK_FRONT.button, "Connect publik");
  assert.match(PUBLIK_FRONT.short, /^No key needed\./);
  assert.ok(PUBLIK_FRONT.short.length <= 80, "the setup-screen line fits on one line, so the button stays in the window");
  const all = Object.values(PUBLIK_FRONT).join(" ");
  assert.ok(!/Publik/.test(all), "the brand is lowercase publik");
  assert.ok(!/\bcredits?\b/i.test(all), "money is a balance, never credits");
});

test("right after Connect publik: the first-run card (balance, link this computer & pick a plan) shows", () => {
  assert.equal(publikFront({ cleanup_mode: "publik" }, { ...fresh, has_key: true, first_run: firstRun }, noKeys), "first_run");
});

test("connected, or own key in use, or no publik in this build: no card", () => {
  assert.equal(publikFront({ cleanup_mode: "publik" }, { ...fresh, has_key: true }, noKeys), "none");
  assert.equal(publikFront({ cleanup_mode: "open_ai" }, fresh, { ...noKeys, has_openai_key: true }), "none");
  assert.equal(publikFront({ cleanup_mode: "anthropic" }, fresh, { ...noKeys, has_anthropic_key: true }), "none");
  assert.equal(publikFront({ cleanup_mode: "local" }, { ...fresh, available: false, can_provision: false }, noKeys), "none");
});

test("own key picked but never saved, or publik picked with no key: still points to publik", () => {
  assert.equal(publikFront({ cleanup_mode: "open_ai" }, fresh, noKeys), "connect");
  assert.equal(publikFront({ cleanup_mode: "publik" }, fresh, noKeys), "connect");
});

test("this computer was removed from the publik account: connect again", () => {
  assert.equal(publikFront({ cleanup_mode: "publik" }, { ...fresh, has_key: true, disconnected: true }, noKeys), "reconnect");
});

test("own-key fields start closed for a new person, open for one who uses a key", () => {
  assert.equal(ownKeysOpen({ cleanup_mode: "local", asr_mode: "local" }, noKeys), false);
  assert.equal(ownKeysOpen({ cleanup_mode: "publik", asr_mode: "local" }, noKeys), false);
  assert.equal(ownKeysOpen({ cleanup_mode: "open_ai", asr_mode: "local" }, noKeys), true);
  assert.equal(ownKeysOpen({ cleanup_mode: "local", asr_mode: "cloud" }, noKeys), true);
  assert.equal(ownKeysOpen({ cleanup_mode: "local", asr_mode: "local" }, { ...noKeys, has_anthropic_key: true }), true);
});
