// publik first: what a new person sees first (Home, the macOS setup screen, Settings), the engine badge, the Help tip,
// and whether the own-key fields in Settings start open. Run: `pnpm --dir ui test` (node --test, Node 22+ strips the
// types). The Rust copy rule (publik::tests::copy_rule_holds_in_every_publik_surface) also reads this file, so the
// money-word check below is a pattern, never the word itself.

import { test } from "node:test";
import assert from "node:assert/strict";
import { PUBLIK_FRONT, cleanupEngineTip, engineBadge, frontLater, frontNow, ownKeysChoice, ownKeysOpen, publikFront, setFrontLater, setOwnKeysChoice, subscribeFrontLater } from "./publikFront.ts";

const noKeys = { has_openai_key: false, has_anthropic_key: false, local_model_present: false };
const withModel = { ...noKeys, local_model_present: true };
const fresh = { available: true, has_key: false, disconnected: false, first_run: null, can_provision: true };
const noToken = { ...fresh, available: false, can_provision: false };
const connected = { ...fresh, has_key: true };
const firstRun = { balance_line: "Your balance is $0.00", justification: "", cta_label: "Link this computer & pick a plan", claim_url: null, later_label: "Later" };

test("first launch (Local default, no model, no key, publik in this build): the card asks to connect publik", () => {
  assert.equal(publikFront({ cleanup_mode: "local" }, fresh, noKeys), "connect");
  assert.equal(PUBLIK_FRONT.title, "WhimprFlow runs on publik API");
  assert.match(PUBLIK_FRONT.lead, /^No key needed\./);
  assert.equal(PUBLIK_FRONT.button, "Connect publik");
  assert.match(PUBLIK_FRONT.short, /^No key needed\./);
  assert.ok(PUBLIK_FRONT.short.length <= 80, "the setup-screen line fits on one line, so the button stays in the window");
  const all = Object.values(PUBLIK_FRONT).join(" ");
  assert.ok(!/Publik/.test(all), "the brand is lowercase publik");
  assert.ok(!/\bcre[d]its?\b/i.test(all), "money is a balance in dollars, never a unit word");
});

test("a working engine: no card (Local with a model, an own key, publik connected, Raw on purpose)", () => {
  assert.equal(publikFront({ cleanup_mode: "local" }, fresh, withModel), "none", "Local with a model on this computer works");
  assert.equal(publikFront({ cleanup_mode: "open_ai" }, fresh, { ...noKeys, has_openai_key: true }), "none");
  assert.equal(publikFront({ cleanup_mode: "anthropic" }, fresh, { ...noKeys, has_anthropic_key: true }), "none");
  assert.equal(publikFront({ cleanup_mode: "publik" }, connected, noKeys), "none");
  assert.equal(publikFront({ cleanup_mode: "raw" }, fresh, noKeys), "none", "Raw is a choice, not a missing engine");
  assert.equal(publikFront({ cleanup_mode: "local" }, noToken, noKeys), "none", "no publik in this build");
});

test("no working engine: the card points to publik (own key picked but never saved, publik picked with no key)", () => {
  assert.equal(publikFront({ cleanup_mode: "open_ai" }, fresh, noKeys), "connect");
  assert.equal(publikFront({ cleanup_mode: "anthropic" }, fresh, withModel), "connect", "a Local model does not run Anthropic mode");
  assert.equal(publikFront({ cleanup_mode: "publik" }, fresh, noKeys), "connect");
});

test("right after Connect publik: the first-run card (balance, link this computer & pick a plan) shows", () => {
  assert.equal(publikFront({ cleanup_mode: "publik" }, { ...connected, first_run: firstRun }, noKeys), "first_run");
});

test("this computer was removed from the publik account: reconnect only while publik is the engine", () => {
  assert.equal(publikFront({ cleanup_mode: "publik" }, { ...connected, disconnected: true }, noKeys), "reconnect");
  assert.equal(publikFront({ cleanup_mode: "local" }, { ...connected, disconnected: true }, withModel), "none", "moved to Local with a model");
});

test("the badge names the engine that works now, never Local next to 'runs on publik API'", () => {
  assert.equal(engineBadge({ cleanup_mode: "local" }, fresh, noKeys), null, "the card says publik; no badge");
  assert.equal(engineBadge({ cleanup_mode: "local" }, fresh, withModel), "Local");
  assert.equal(engineBadge({ cleanup_mode: "publik" }, connected, noKeys), "publik API");
  assert.equal(engineBadge({ cleanup_mode: "publik" }, { ...connected, disconnected: true }, noKeys), null);
  assert.equal(engineBadge({ cleanup_mode: "open_ai" }, fresh, { ...noKeys, has_openai_key: true }), "OpenAI");
  assert.equal(engineBadge({ cleanup_mode: "raw" }, fresh, noKeys), "Raw");
  assert.equal(engineBadge({ cleanup_mode: "local" }, noToken, noKeys), "Local", "no app token: no card, so the Local badge shows");
  for (const mode of ["local", "open_ai", "anthropic", "publik"] as const) {
    const front = publikFront({ cleanup_mode: mode }, fresh, noKeys);
    if (front === "connect") assert.equal(engineBadge({ cleanup_mode: mode }, fresh, noKeys), null, `${mode}: no badge beside the card`);
  }
});

test("the card's 'Now:' line says which engine is picked and why it cannot run, so the card and the picker agree", () => {
  assert.equal(frontNow({ cleanup_mode: "local" }, fresh, noKeys), "Now: Local is picked, but this computer has no Local model. Your words are typed exactly as you said them.");
  assert.match(frontNow({ cleanup_mode: "open_ai" }, fresh, noKeys) ?? "", /^Now: OpenAI is picked, but no OpenAI key is saved\./);
  assert.match(frontNow({ cleanup_mode: "publik" }, fresh, noKeys) ?? "", /^Now: publik API is picked, but it is not connected yet\./);
  assert.equal(frontNow({ cleanup_mode: "local" }, fresh, withModel), null, "no card, no line");
  assert.equal(frontNow({ cleanup_mode: "raw" }, fresh, noKeys), null);
});

test("Help 'Pick a cleanup engine' names publik first, in the card's words, and does not say it is already set up", () => {
  const tip = cleanupEngineTip("macOS Keychain");
  assert.ok(tip.startsWith(`${PUBLIK_FRONT.title}. ${PUBLIK_FRONT.lead}`), tip);
  assert.ok(tip.includes(PUBLIK_FRONT.button));
  assert.ok(tip.indexOf("publik") < tip.indexOf("Local"), "publik before Local");
  assert.ok(!/already set up/i.test(tip));
  assert.ok(tip.endsWith("Keys are stored in your macOS Keychain."));
});

test("'Not now' and 'Use my own key instead' are one stored flag that Home, the setup screen and Settings share; the own-key choice is kept", () => {
  const store = new Map<string, string>();
  const g = globalThis as { localStorage?: unknown };
  const before = g.localStorage;
  g.localStorage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) };
  try {
    let calls = 0;
    const off = subscribeFrontLater(() => calls++);
    assert.equal(frontLater(), false);
    setFrontLater();
    assert.equal(frontLater(), true);
    assert.equal(calls, 1, "every open card hears it at once");
    off();
    setFrontLater();
    assert.equal(calls, 1);
    // "Use my own key instead": the own-key fields stay open on the next visit to Settings
    assert.equal(ownKeysChoice(), null, "never chosen: the rule in ownKeysOpen decides");
    setOwnKeysChoice(true);
    assert.equal(ownKeysChoice(), true);
    setOwnKeysChoice(false);
    assert.equal(ownKeysChoice(), false);
  } finally {
    g.localStorage = before;
  }
});

test("own-key fields start closed for a new person, open for one who uses a key", () => {
  assert.equal(ownKeysOpen({ cleanup_mode: "local", asr_mode: "local" }, noKeys), false);
  assert.equal(ownKeysOpen({ cleanup_mode: "publik", asr_mode: "local" }, noKeys), false);
  assert.equal(ownKeysOpen({ cleanup_mode: "open_ai", asr_mode: "local" }, noKeys), true);
  assert.equal(ownKeysOpen({ cleanup_mode: "local", asr_mode: "cloud" }, noKeys), true);
  assert.equal(ownKeysOpen({ cleanup_mode: "local", asr_mode: "local" }, { ...noKeys, has_anthropic_key: true }), true);
});
