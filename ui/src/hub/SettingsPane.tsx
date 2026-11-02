import { useEffect, useState } from "react";
import { font, palette } from "../tokens/values";
import { theme } from "./theme";
import { Button, Card, Dot, PageTitle, Segmented } from "./ui";
import { CloudDisclosure } from "./CloudDisclosure";
import { PublikFirstRun } from "./PublikFirstRun";
import { frontNow, ownKeysChoice, ownKeysOpen, publikFront, setFrontLater, setOwnKeysChoice } from "./publikFront";
import { useFrontLater } from "./PublikFrontCard";
import {
  publikAcceptDisclosure,
  publikForgetKey,
  publikOpenLink,
  requestAccessibility,
  requestInputMonitoring,
  requestMicrophone,
  setApiKey,
  type AsrMode,
  type CleanupLevel,
  type CleanupMode,
  type PublikStatus,
  type Settings,
  type Status,
} from "./api";

const ASR_MODES: { value: AsrMode; label: string; hint: string }[] = [
  {
    value: "local",
    label: "Local",
    hint: "On-device Whisper (offline, needs a downloaded speech model — see Home).",
  },
  {
    value: "cloud",
    label: "Cloud",
    hint: "Fast cloud transcription via OpenAI or an OpenAI-compatible API like Groq.",
  },
];

// publik first: "publik API" leads the picker and the card above it says that
// WhimprFlow runs on publik API with no key; Local stays the saved default until
// the person connects (the cost + data-path notice is the consent).
const MODES: { value: CleanupMode; label: string; hint: string }[] = [
  {
    value: "publik",
    label: "publik API",
    hint: "Cloud cleanup on your publik balance — priced per use at 50% of the model's published list price; most people spend under $2 a month. Your transcript (never audio) goes through publik's servers to a shared model account; publik never trains on it and does not store it.",
  },
  { value: "local", label: "Local", hint: "On-device model (offline)" },
  { value: "raw", label: "Raw", hint: "Paste exactly what you said" },
  { value: "open_ai", label: "OpenAI", hint: "Cloud cleanup via OpenAI (or an OpenAI-compatible API like OpenRouter — set the base URL below)" },
  { value: "anthropic", label: "Anthropic", hint: "Cloud cleanup via Claude" },
];

const LEVELS: { value: CleanupLevel; label: string; hint: string }[] = [
  { value: "none", label: "None", hint: "Transcribe exactly what you said, including mistakes." },
  { value: "light", label: "Light", hint: "Clean up filler words and grammar. (Recommended)" },
  { value: "medium", label: "Medium", hint: "Edit for clarity and conciseness." },
  { value: "high", label: "High", hint: "Rewrite for brevity and polish." },
];

const PTT_KEYS: { value: string; label: string }[] = [
  { value: "ControlRight", label: "Right Ctrl" },
  { value: "ControlLeft", label: "Left Ctrl" },
  { value: "AltRight", label: "Right Alt" },
  { value: "AltLeft", label: "Left Alt" },
  { value: "ShiftRight", label: "Right Shift" },
  { value: "ShiftLeft", label: "Left Shift" },
  { value: "MetaRight", label: "Right Super/Windows" },
  { value: "MetaLeft", label: "Left Super/Windows" },
  { value: "CapsLock", label: "Caps Lock" },
  { value: "F1", label: "F1" },
  { value: "F2", label: "F2" },
  { value: "F3", label: "F3" },
  { value: "F4", label: "F4" },
  { value: "F5", label: "F5" },
  { value: "F6", label: "F6" },
  { value: "F7", label: "F7" },
  { value: "F8", label: "F8" },
  { value: "F9", label: "F9" },
  { value: "F10", label: "F10" },
  { value: "F11", label: "F11" },
  { value: "F12", label: "F12" },
];

function SectionTitle({ children, sub }: { children: React.ReactNode; sub?: string }) {
  return (
    <div style={{ marginBottom: 14 }}>
      <div style={{ fontSize: 15, fontWeight: 600, color: theme.textStrong }}>{children}</div>
      {sub && <div style={{ color: theme.textMuted, fontSize: 13, marginTop: 4 }}>{sub}</div>}
    </div>
  );
}

// A physical key from a KeyboardEvent.code, as a Tauri accelerator key name.
// Returns null for a bare modifier press (so the recorder keeps listening) and
// for keys we don't want to bind.
function keyNameFromCode(code: string): string | null {
  if (code === "Space") return "Space";
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F[0-9]{1,2}$/.test(code)) return code;
  return null;
}

// A KeyboardEvent → a Tauri accelerator string ("CmdOrCtrl+Shift+Space"), or
// null if it isn't a valid global shortcut yet (no non-modifier key, or no
// modifier — a bare key makes a terrible global hotkey).
function acceleratorFromEvent(e: KeyboardEvent): string | null {
  const key = keyNameFromCode(e.code);
  if (!key) return null;
  const parts: string[] = [];
  if (e.metaKey) parts.push("CmdOrCtrl");
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (parts.length === 0) return null;
  parts.push(key);
  return parts.join("+");
}

const ACCELERATOR_SYMBOLS: Record<string, string> = {
  CmdOrCtrl: "⌘",
  Cmd: "⌘",
  Command: "⌘",
  Super: "⌘",
  Ctrl: "⌃",
  Control: "⌃",
  Alt: "⌥",
  Option: "⌥",
  Shift: "⇧",
};

// Windows and Linux have no ⌘: "CmdOrCtrl" is the Ctrl key there, spelled
// out the way those platforms write chords ("Ctrl+Shift+Space"). The Windows
// Hub used to show ⌘ ⇧ Space for a shortcut the keyboard cannot produce.
const ACCELERATOR_WORDS: Record<string, string> = {
  CmdOrCtrl: "Ctrl",
  Cmd: "Win",
  Command: "Win",
  Super: "Win",
  Control: "Ctrl",
  Option: "Alt",
};

export function prettyAccelerator(accelerator: string, platform: string): string {
  if (!accelerator.trim()) return "Off";
  const parts = accelerator.split("+");
  if (platform === "macos" || platform === "") {
    return parts.map((part) => ACCELERATOR_SYMBOLS[part] ?? part).join(" ");
  }
  return parts.map((part) => ACCELERATOR_WORDS[part] ?? part).join("+");
}

// "Speak without having to hold down fn … a combination of buttons … with
// customization in settings" (Publik Test 2). Click to record a new shortcut;
// the next modifier+key you press becomes it. Esc while recording cancels.
function HandsFreeHotkeyRow({
  value,
  onChange,
  platform,
  keyLabel,
}: {
  value: string;
  onChange: (accelerator: string) => void;
  platform: string;
  keyLabel: string;
}) {
  const [recording, setRecording] = useState(false);

  useEffect(() => {
    if (!recording) return;
    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setRecording(false);
        return;
      }
      const accelerator = acceleratorFromEvent(e);
      if (accelerator) {
        onChange(accelerator);
        setRecording(false);
      }
      // Otherwise a bare modifier is still held — keep listening for the key.
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [recording, onChange]);

  return (
    <Card style={{ marginBottom: 16 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>
            Hands-free shortcut
          </div>
          <div style={{ color: theme.textMuted, fontSize: 13, marginTop: 4 }}>
            Press it once to start talking with no key held, again to stop. Holding {keyLabel}{" "}
            (push-to-talk){platform === "macos" ? ` and double-tapping ${keyLabel}` : ""} still{" "}
            {platform === "macos" ? "work" : "works"} too.
          </div>
        </div>
        <div style={{ display: "flex", alignItems: "center", gap: 8, flex: "0 0 auto" }}>
          <Button onClick={() => setRecording((on) => !on)}>
            {recording ? "Press keys…" : prettyAccelerator(value, platform)}
          </Button>
          {value.trim() !== "" && !recording && (
            <Button onClick={() => onChange("")}>Off</Button>
          )}
        </div>
      </div>
    </Card>
  );
}

function KeyField({
  label,
  configured,
  onSave,
}: {
  label: string;
  configured: boolean;
  onSave: (key: string) => Promise<void>;
}) {
  const [value, setValue] = useState("");
  const [status, setStatus] = useState<"idle" | "saving" | "saved" | "error">("idle");
  return (
    <div style={{ marginTop: 16 }}>
      <div style={{ fontSize: 13, marginBottom: 7, display: "flex", alignItems: "center", color: theme.textBody }}>
        <Dot ok={configured} />
        {label} {configured ? "— configured" : "— not set"}
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        <input
          type="password"
          value={value}
          placeholder={configured ? "Enter a new key to replace" : "Paste your API key"}
          onChange={(e) => {
            setValue(e.target.value);
            setStatus("idle");
          }}
          style={{
            flex: 1,
            background: theme.cardBgSubtle,
            border: `1px solid ${theme.border}`,
            borderRadius: 10,
            padding: "9px 12px",
            color: theme.textBody,
            fontFamily: font.mono,
            fontSize: 13,
            outline: "none",
          }}
        />
        <Button
          onClick={async () => {
            setStatus("saving");
            try {
              await onSave(value);
              setValue("");
              setStatus("saved");
            } catch (e) {
              console.error("save key failed", e);
              setStatus("error");
            }
          }}
        >
          Save
        </Button>
      </div>
      {status === "saved" && (
        <div style={{ fontSize: 12, color: theme.accentDeep, marginTop: 6 }}>Saved to keychain ✓</div>
      )}
      {status === "error" && (
        <div style={{ fontSize: 12, color: "#e5484d", marginTop: 6 }}>
          Couldn't save — the OS credential store may be unavailable. Check the app's console output.
        </div>
      )}
    </div>
  );
}

// The publik API card under the picker: the balance line from the gateway's
// headers (live via `whimpr://publik`), the R21 §4.1 states (ready / needs
// credit / disconnected / unreachable), and the "use my own key" branch.
function PublikCard({
  publik,
  onUseOwnKey,
  onReconnect,
  onForget,
}: {
  publik: PublikStatus;
  onUseOwnKey: () => void;
  onReconnect: () => void;
  onForget: () => void;
}) {
  const [confirmForget, setConfirmForget] = useState(false);
  const [whyOpen, setWhyOpen] = useState(false);
  const ready = publik.has_key && !publik.exhausted && !publik.disconnected && !publik.unreachable;
  const anonymous = publik.claim_state !== "claimed";
  const state = !publik.has_key
    ? "— not set up"
    : publik.disconnected
      ? "— disconnected"
      : publik.exhausted
        ? "— needs credit"
        : publik.unreachable
          ? "— unreachable"
          : "— ready";
  return (
    <div style={{ marginTop: 16 }}>
      <div style={{ fontSize: 13, display: "flex", alignItems: "center", flexWrap: "wrap", color: theme.textBody }}>
        <Dot ok={ready} />
        <b>publik API</b>&nbsp;{state}
        {publik.balance_label && <span style={{ marginLeft: 8, color: theme.textMuted }}>· {publik.balance_label}</span>}
        {publik.week_label && <span style={{ marginLeft: 8, color: theme.textMuted }}>· {publik.week_label}</span>}
        {publik.last_charge_label && <span style={{ marginLeft: 8, color: theme.textFaint }}>· {publik.last_charge_label}</span>}
      </div>
      {publik.has_key && publik.exhausted && (
        <div style={{ fontSize: 12.5, color: palette.error, marginTop: 6 }}>
          <b>{anonymous ? "publik API needs a balance." : "publik API needs a plan or a pack."}</b>{" "}
          {anonymous
            ? "Link this computer to your publik account for $0.05 of free use, once, pick a plan, or use your own key."
            : "Your publik balance is too low. Add a plan or a pack, or use your own key."}{" "}
          Dictation still works — text is pasted without cleanup.
        </div>
      )}
      {publik.disconnected && (
        <div style={{ fontSize: 12.5, color: palette.error, marginTop: 6 }}>
          <b>publik API is disconnected.</b> This computer was removed from your publik account.
        </div>
      )}
      {publik.has_key && !publik.disconnected && publik.unreachable && (
        <div style={{ fontSize: 12.5, color: theme.textMuted, marginTop: 6 }}>
          <b>publik API is unreachable right now.</b> Nothing is being charged. Try again in a minute, or use your own key.
        </div>
      )}
      <div style={{ display: "flex", gap: 8, marginTop: 10, flexWrap: "wrap" }}>
        {publik.disconnected || !publik.has_key ? (
          <Button size="sm" onClick={onReconnect} disabled={!publik.can_provision}>
            {publik.disconnected ? "Reconnect" : "Turn on publik API"}
          </Button>
        ) : publik.exhausted ? (
          // The 402's one link (top_up_url): the claim page while anonymous,
          // the add-credit page once claimed.
          <Button size="sm" variant="accent" onClick={() => void publikOpenLink("top_up")}>
            {anonymous ? "Link this computer & pick a plan" : "Add a plan or pack"}
          </Button>
        ) : (
          // CONTRACT §12.2: "Pick a plan" → claim_url while anonymous;
          // "Manage plan" → the dashboard once this computer is claimed.
          <Button size="sm" variant="accent" onClick={() => void publikOpenLink("plan")}>
            {publik.plan_cta.label}
          </Button>
        )}
        <Button size="sm" variant="ghost" onClick={() => void publikOpenLink("dashboard")}>
          Usage
        </Button>
        <Button size="sm" variant="ghost" onClick={onUseOwnKey}>
          Use my own key instead
        </Button>
        {publik.has_key &&
          (confirmForget ? (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setConfirmForget(false);
                onForget();
              }}
            >
              Really forget this key?
            </Button>
          ) : (
            <Button size="sm" variant="ghost" onClick={() => setConfirmForget(true)}>
              Forget this key
            </Button>
          ))}
      </div>
      {publik.has_key && publik.justification && (
        <div style={{ marginTop: 10, fontSize: 12.5 }}>
          <a
            href="#why-it-costs"
            onClick={(e) => {
              e.preventDefault();
              setWhyOpen((v) => !v);
            }}
            style={{ color: theme.accentDeep, fontWeight: 600 }}
          >
            {whyOpen ? "Why it costs money ▾" : "Why it costs money ▸"}
          </a>
          {whyOpen && <div style={{ color: theme.textMuted, marginTop: 4, lineHeight: 1.5 }}>{publik.justification}</div>}
        </div>
      )}
    </div>
  );
}

function PermRow({
  ok,
  label,
  detail,
  onClick,
}: {
  ok: boolean;
  label: string;
  detail: string;
  onClick: () => void;
}) {
  return (
    <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
      <div style={{ display: "flex", alignItems: "center", fontSize: 13 }}>
        <Dot ok={ok} />
        <span style={{ color: theme.textBody }}>
          <b>{label}</b> <span style={{ color: theme.textMuted }}>— {detail}</span>
        </span>
      </div>
      {ok ? (
        <span style={{ color: theme.accentDeep, fontSize: 13, fontWeight: 600 }}>Granted</span>
      ) : (
        <Button variant="ghost" size="sm" onClick={onClick}>
          Grant
        </Button>
      )}
    </div>
  );
}

export function SettingsPane({
  settings,
  onChange,
  status,
  refresh,
  publik,
  refreshPublik,
  setPublik,
  reloadSettings,
  openOwnKeys = 0,
  onOwnKeysShown,
}: {
  settings: Settings;
  onChange: (s: Settings) => void;
  status: Status;
  refresh: () => void;
  publik: PublikStatus;
  // Re-reads the status and (throttled, in Rust) GET /wallet.
  refreshPublik: () => void;
  setPublik: (p: PublikStatus) => void;
  // Re-reads Settings from Rust after it changed them (provisioning writes
  // the install id / base URL / model / claim link and selects the mode).
  reloadSettings: () => Promise<void>;
  // Bumped by "Use my own key instead" on the Home / setup card: open the own-key fields and scroll to them.
  openOwnKeys?: number;
  // Called once the own-key fields were opened for that request, so a later visit to Settings does not scroll again.
  onOwnKeysShown?: () => void;
}) {
  const [showDisclosure, setShowDisclosure] = useState(false);
  // The own-key fields start closed unless the person already uses their own key (publik first), or chose to see
  // them ("Use my own key instead", or the toggle): that choice is kept for the next visit.
  const [keysChoice, setKeysChoiceState] = useState<boolean | null>(ownKeysChoice);
  const setKeysChoice = (open: boolean) => {
    setOwnKeysChoice(open);
    setKeysChoiceState(open);
  };
  const keysOpen = keysChoice ?? ownKeysOpen(settings, status);
  // "Not now" here is the same stored flag as on Home and the setup screen.
  const later = useFrontLater();
  const front = publikFront(settings, publik, status);
  useEffect(() => {
    if (!openOwnKeys) return;
    // "Use my own key instead" (Home or the setup screen): the own-key fields, not the publik card on top of them.
    setFrontLater();
    setKeysChoice(true);
    setTimeout(() => document.getElementById("byo-keys")?.scrollIntoView({ behavior: "smooth", block: "start" }), 80);
    onOwnKeysShown?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- only a new request opens the fields
  }, [openOwnKeys]);
  const [busy, setBusy] = useState(false);
  const [publikError, setPublikError] = useState<string | null>(null);
  // The front card only while no engine works and publik is not the picked engine: with publik picked, the publik
  // card below covers every state ("Turn on publik API", "Reconnect"), so there is never a second publik block.
  const frontShown = front === "connect" && settings.cleanup_mode !== "publik" && !showDisclosure && !later;

  // Opening Settings is the GET /wallet moment (only when a key exists —
  // Rust decides; a device that never picked publik never phones home).
  useEffect(() => {
    refreshPublik();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const useOwnKey = () => {
    // Switch mode only. The publik key stays for when they come back; nothing
    // is deleted and the user's own key fields are untouched.
    setShowDisclosure(false);
    setFrontLater();
    setKeysChoice(true);
    onChange({ ...settings, cleanup_mode: "open_ai" });
    setTimeout(() => document.getElementById("byo-keys")?.scrollIntoView({ behavior: "smooth", block: "start" }), 50);
  };

  const accept = async () => {
    setBusy(true);
    setPublikError(null);
    try {
      // The ONE call that provisions: mint (if needed) → keychain → mode = publik.
      // Rust persisted the mode and the publik fields; re-read rather than
      // pushing this pane's pre-provisioning copy back over them.
      const p = await publikAcceptDisclosure();
      setPublik(p);
      await reloadSettings();
      setShowDisclosure(false);
    } catch (e) {
      setPublikError(typeof e === "string" ? e : e instanceof Error ? e.message : "Could not set up publik API.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div style={{ maxWidth: 720 }}>
      <PageTitle>Settings</PageTitle>

      <Card style={{ marginBottom: 16 }}>
        <SectionTitle sub="Where your dictation is cleaned up before it's typed.">Cleanup Engine</SectionTitle>
        {/* One publik block at a time: the front card when no engine works yet; when publik is the engine and this
            computer was removed from the account, the publik card below says "Reconnect" (not a second block). */}
        {frontShown && (
          <div style={{ marginBottom: 14 }}>
            <CloudDisclosure
              variant="front"
              now={frontNow(settings, publik, status)}
              onAccept={() => void accept()}
              onOwnKey={useOwnKey}
              onClose={setFrontLater}
              busy={busy}
              error={publikError}
              canProvision={publik.can_provision || publik.has_key}
            />
          </div>
        )}
        <Segmented
          options={MODES.map((m) => ({ value: m.value, label: m.label }))}
          value={settings.cleanup_mode}
          onChange={(v) => {
            // Picking publik the first time (or with no key) shows the
            // disclosure instead of switching — the mode changes only after
            // "Turn on publik API".
            if (v === "publik" && (publik.disclosure_needed || !publik.has_key)) {
              setPublikError(null);
              setShowDisclosure(true);
              return;
            }
            setShowDisclosure(false);
            onChange({ ...settings, cleanup_mode: v });
          }}
        />
        <div style={{ color: theme.textMuted, fontSize: 12.5, marginTop: 10 }}>
          {MODES.find((m) => m.value === settings.cleanup_mode)?.hint}
        </div>

        {/* The front card above already says why Local cannot run ("Now: ..."); this line is for after "Not now". */}
        {settings.cleanup_mode === "local" && !status.local_model_present && !showDisclosure && !frontShown && (
          <div style={{ fontSize: 12.5, color: theme.textMuted, marginTop: 10 }}>
            Local cleanup needs a model you haven't added yet (see docs/MODELS.md), so text is pasted as spoken.{" "}
            {publik.available && (
              <>
                <a
                  href="#publik"
                  onClick={(e) => {
                    e.preventDefault();
                    setPublikError(null);
                    setShowDisclosure(true);
                  }}
                  style={{ color: theme.accentDeep, fontWeight: 600 }}
                >
                  Turn on publik API →
                </a>{" "}
                or add your own key below.
              </>
            )}
          </div>
        )}

        {showDisclosure && (
          <CloudDisclosure
            onAccept={() => void accept()}
            onOwnKey={useOwnKey}
            onClose={() => setShowDisclosure(false)}
            busy={busy}
            error={publikError}
            canProvision={publik.can_provision || publik.has_key}
          />
        )}

        {publik.first_run && !showDisclosure && (
          // Right after provisioning (and on the next launch if it was never
          // settled): the balance, why it costs money, and the plan button.
          <PublikFirstRun card={publik.first_run} onSettled={setPublik} />
        )}

        {settings.cleanup_mode === "publik" && !showDisclosure && !publik.first_run && (
          <PublikCard
            publik={publik}
            onUseOwnKey={useOwnKey}
            onReconnect={() => {
              setPublikError(null);
              setShowDisclosure(true);
            }}
            onForget={() => {
              void publikForgetKey().then(setPublik);
            }}
          />
        )}

        <div id="byo-keys" style={{ marginTop: 22 }}>
          <button
            onClick={() => setKeysChoice(!keysOpen)}
            aria-expanded={keysOpen}
            style={{ all: "unset", cursor: "pointer", display: "block", width: "100%" }}
          >
            <SectionTitle sub="Bring your own key — your key, your account, your bill. Not needed with publik API.">
              {keysOpen ? "▾" : "▸"} Use my own key
            </SectionTitle>
          </button>
        </div>
        {keysOpen && (
        <>
        <KeyField
          label="OpenAI API key"
          configured={status.has_openai_key}
          onSave={async (k) => {
            await setApiKey("openai", k);
            setTimeout(refresh, 400);
          }}
        />
        <div style={{ marginTop: 12, display: "flex", gap: 8 }}>
          <div style={{ flex: 1 }}>
            <div style={{ fontSize: 12.5, color: theme.textMuted, marginBottom: 6 }}>
              Base URL (blank = OpenAI; e.g. https://openrouter.ai/api/v1 for OpenRouter)
            </div>
            <input
              type="text"
              value={settings.openai_base_url}
              placeholder="https://openrouter.ai/api/v1"
              onChange={(e) => onChange({ ...settings, openai_base_url: e.target.value })}
              style={{
                width: "100%",
                background: theme.cardBgSubtle,
                border: `1px solid ${theme.border}`,
                borderRadius: 10,
                padding: "9px 12px",
                color: theme.textBody,
                fontFamily: font.mono,
                fontSize: 13,
                outline: "none",
                boxSizing: "border-box",
              }}
            />
          </div>
          <div style={{ flex: 1 }}>
            <div style={{ fontSize: 12.5, color: theme.textMuted, marginBottom: 6 }}>
              Model (e.g. an OpenRouter model slug)
            </div>
            <input
              type="text"
              value={settings.openai_model}
              placeholder="meta-llama/llama-3.3-70b-instruct:free"
              onChange={(e) => onChange({ ...settings, openai_model: e.target.value })}
              style={{
                width: "100%",
                background: theme.cardBgSubtle,
                border: `1px solid ${theme.border}`,
                borderRadius: 10,
                padding: "9px 12px",
                color: theme.textBody,
                fontFamily: font.mono,
                fontSize: 13,
                outline: "none",
                boxSizing: "border-box",
              }}
            />
          </div>
        </div>
        <KeyField
          label="Anthropic API key"
          configured={status.has_anthropic_key}
          onSave={async (k) => {
            await setApiKey("anthropic", k);
            setTimeout(refresh, 400);
          }}
        />
        </>
        )}
      </Card>

      <Card style={{ marginBottom: 16 }}>
        <SectionTitle sub="Which engine turns your speech into text.">Speech-to-Text</SectionTitle>
        <Segmented
          options={ASR_MODES.map((m) => ({ value: m.value, label: m.label }))}
          value={settings.asr_mode}
          onChange={(v) => onChange({ ...settings, asr_mode: v })}
        />
        <div style={{ color: theme.textMuted, fontSize: 12.5, marginTop: 10 }}>
          {ASR_MODES.find((m) => m.value === settings.asr_mode)?.hint}
        </div>
        {settings.asr_mode === "cloud" && (
          <div style={{ marginTop: 12, display: "flex", gap: 8 }}>
            <div style={{ flex: 1 }}>
              <div style={{ fontSize: 12.5, color: theme.textMuted, marginBottom: 6 }}>
                Base URL (blank = OpenAI; e.g. https://api.groq.com/openai/v1 for Groq)
              </div>
              <input
                type="text"
                value={settings.asr_base_url}
                placeholder="https://api.groq.com/openai/v1"
                onChange={(e) => onChange({ ...settings, asr_base_url: e.target.value })}
                style={{
                  width: "100%",
                  background: theme.cardBgSubtle,
                  border: `1px solid ${theme.border}`,
                  borderRadius: 10,
                  padding: "9px 12px",
                  color: theme.textBody,
                  fontFamily: font.mono,
                  fontSize: 13,
                  outline: "none",
                  boxSizing: "border-box",
                }}
              />
            </div>
            <div style={{ flex: 1 }}>
              <div style={{ fontSize: 12.5, color: theme.textMuted, marginBottom: 6 }}>
                Model (e.g. whisper-large-v3-turbo for Groq, whisper-1 for OpenAI)
              </div>
              <input
                type="text"
                value={settings.asr_model}
                placeholder="whisper-large-v3-turbo"
                onChange={(e) => onChange({ ...settings, asr_model: e.target.value })}
                style={{
                  width: "100%",
                  background: theme.cardBgSubtle,
                  border: `1px solid ${theme.border}`,
                  borderRadius: 10,
                  padding: "9px 12px",
                  color: theme.textBody,
                  fontFamily: font.mono,
                  fontSize: 13,
                  outline: "none",
                  boxSizing: "border-box",
                }}
              />
            </div>
          </div>
        )}
        <div style={{ color: theme.textMuted, fontSize: 12.5, marginTop: 10 }}>
          Cloud speech-to-text reuses the OpenAI API key in "Use my own key" above.
        </div>
      </Card>

      <Card style={{ marginBottom: 16 }}>
        <SectionTitle>Auto Cleanup</SectionTitle>
        <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
          {LEVELS.map((l) => {
            const selected = settings.cleanup_level === l.value;
            return (
              <button
                key={l.value}
                onClick={() => onChange({ ...settings, cleanup_level: l.value })}
                style={{
                  textAlign: "left",
                  cursor: "pointer",
                  borderRadius: 12,
                  padding: "12px 14px",
                  fontFamily: font.ui,
                  background: selected ? theme.accentSoft : theme.cardBgSubtle,
                  border: `1px solid ${selected ? theme.accentSoftBorder : theme.border}`,
                  color: theme.textBody,
                }}
              >
                <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>{l.label}</div>
                <div style={{ fontSize: 12.5, color: theme.textMuted, marginTop: 2 }}>{l.hint}</div>
              </button>
            );
          })}
        </div>
      </Card>

      {/* macOS reads a fixed Fn key, so this card would be a no-op there. */}
      {status.platform !== "macos" && (
      <Card style={{ marginBottom: 16 }}>
        <SectionTitle sub="Hold this key to start dictating, release to finish.">Push-to-Talk Key</SectionTitle>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
          {PTT_KEYS.map((k) => {
            const selected = settings.push_to_talk_key === k.value;
            return (
              <button
                key={k.value}
                onClick={() => onChange({ ...settings, push_to_talk_key: k.value })}
                style={{
                  cursor: "pointer",
                  borderRadius: 8,
                  padding: "8px 14px",
                  fontFamily: font.ui,
                  fontSize: 13,
                  border: `1px solid ${selected ? theme.accentSoftBorder : theme.border}`,
                  background: selected ? theme.accentSoft : theme.cardBgSubtle,
                  color: selected ? theme.textStrong : theme.textBody,
                  fontWeight: selected ? 600 : 400,
                }}
              >
                {k.label}
              </button>
            );
          })}
        </div>
        <div style={{ color: theme.textMuted, fontSize: 12.5, marginTop: 10 }}>
          {status.platform === "windows"
            ? "Takes effect right away."
            : "Changes take effect after restarting the app."}
        </div>
      </Card>
      )}

      <Card style={{ marginBottom: 16 }}>
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
          <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>
            Play a sound when recording starts
          </div>
          <Segmented
            options={[
              { value: "on", label: "On" },
              { value: "off", label: "Off" },
            ]}
            value={settings.sound_on_start ? "on" : "off"}
            onChange={(v) => onChange({ ...settings, sound_on_start: v === "on" })}
          />
        </div>
      </Card>

      <HandsFreeHotkeyRow
        value={settings.hands_free_hotkey ?? ""}
        onChange={(accelerator) => onChange({ ...settings, hands_free_hotkey: accelerator })}
        platform={status.platform}
        keyLabel={status.push_to_talk_label}
      />

      {status.platform === "windows" ? (
        // Windows has no Accessibility / Input Monitoring grants, and the app
        // cannot read the microphone privacy switch, so three green rows here
        // would be a lie. Say what to do if the pill reports a blocked mic.
        <Card>
          <SectionTitle sub="Windows asks for no permission to watch the key or type your words.">
            Permissions
          </SectionTitle>
          <div style={{ fontSize: 13, color: theme.textBody, lineHeight: 1.55 }}>
            If dictation stops with <b>Microphone access blocked</b>, open Windows Settings → Privacy &amp;
            security → Microphone and turn on <b>Let desktop apps access your microphone</b>. If it says{" "}
            <b>No microphone found</b>, plug in or enable a microphone under Settings → System → Sound → Input.
          </div>
        </Card>
      ) : (
      <Card>
        <SectionTitle sub="Grant these to WhimprFlow — dots update automatically within a few seconds.">
          Permissions
        </SectionTitle>
        <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          <PermRow
            ok={status.accessibility}
            label="Accessibility"
            detail={
              status.accessibility
                ? `granted — ${status.push_to_talk_label} works everywhere + types your words`
                : `the key one: makes ${status.push_to_talk_label} work in EVERY app AND types your words`
            }
            onClick={() => {
              requestAccessibility();
              setTimeout(refresh, 800);
            }}
          />
          <PermRow
            ok={status.microphone}
            label="Microphone"
            // Same honesty as the setup screen: when macOS is judging this as
            // some other app, say so instead of pointing at a switch that
            // cannot move this dot.
            detail={
              status.microphone
                ? "granted"
                : (status.microphone_hint ?? "hears what you say")
            }
            onClick={() => {
              requestMicrophone();
              setTimeout(refresh, 1000);
            }}
          />
          <PermRow
            ok={status.input_monitoring}
            label="Input Monitoring"
            detail="optional — extra reliability for key detection"
            onClick={() => {
              requestInputMonitoring();
              setTimeout(refresh, 1000);
            }}
          />
        </div>
      </Card>
      )}
    </div>
  );
}
