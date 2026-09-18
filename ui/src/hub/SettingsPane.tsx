import { useEffect, useState } from "react";
import { font } from "../tokens/values";
import { theme } from "./theme";
import { Button, Card, Dot, PageTitle, Segmented } from "./ui";
import {
  LANGUAGES,
  listMicrophones,
  PTT_KEYS,
  requestAccessibility,
  requestInputMonitoring,
  requestMicrophone,
  resetPillPosition,
  setApiKey,
  type CleanupLevel,
  type CleanupMode,
  type PushToTalkKey,
  type Settings,
  type Status,
} from "./api";

const selectStyle: React.CSSProperties = {
  fontFamily: font.ui,
  fontSize: 13.5,
  color: theme.textBody,
  background: theme.cardBgSubtle,
  border: `1px solid ${theme.borderStrong}`,
  borderRadius: 9,
  padding: "7px 10px",
  minWidth: 210,
  cursor: "pointer",
};

/// A labelled settings row: title + explanatory hint on the left, control right.
function Row({
  title,
  hint,
  children,
}: {
  title: string;
  hint: string;
  children: React.ReactNode;
}) {
  return (
    <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 16 }}>
      <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>
        {title}
        <div style={{ fontSize: 12, fontWeight: 400, color: theme.textMuted, marginTop: 2 }}>
          {hint}
        </div>
      </div>
      {children}
    </div>
  );
}

const MODES: { value: CleanupMode; label: string; hint: string }[] = [
  { value: "raw", label: "Raw", hint: "Paste exactly what you said" },
  { value: "local", label: "Local", hint: "On-device model (offline)" },
  { value: "open_ai", label: "OpenAI", hint: "Cloud cleanup via OpenAI (or an OpenAI-compatible API like OpenRouter — set the base URL below)" },
  { value: "anthropic", label: "Anthropic", hint: "Cloud cleanup via Claude" },
];

const LEVELS: { value: CleanupLevel; label: string; hint: string }[] = [
  { value: "none", label: "None", hint: "Transcribe exactly what you said, including mistakes." },
  { value: "light", label: "Light", hint: "Clean up filler words and grammar. (Recommended)" },
  { value: "medium", label: "Medium", hint: "Edit for clarity and conciseness." },
  { value: "high", label: "High", hint: "Rewrite for brevity and polish." },
];

function SectionTitle({ children, sub }: { children: React.ReactNode; sub?: string }) {
  return (
    <div style={{ marginBottom: 14 }}>
      <div style={{ fontSize: 15, fontWeight: 600, color: theme.textStrong }}>{children}</div>
      {sub && <div style={{ color: theme.textMuted, fontSize: 13, marginTop: 4 }}>{sub}</div>}
    </div>
  );
}

function KeyField({
  label,
  configured,
  onSave,
}: {
  label: string;
  configured: boolean;
  onSave: (key: string) => void;
}) {
  const [value, setValue] = useState("");
  const [saved, setSaved] = useState(false);
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
            setSaved(false);
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
          onClick={() => {
            onSave(value);
            setValue("");
            setSaved(true);
          }}
        >
          Save
        </Button>
      </div>
      {saved && <div style={{ fontSize: 12, color: theme.accentDeep, marginTop: 6 }}>Saved to keychain ✓</div>}
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
}: {
  settings: Settings;
  onChange: (s: Settings) => void;
  status: Status;
  refresh: () => void;
}) {
  const [mics, setMics] = useState<string[]>([]);
  useEffect(() => {
    void listMicrophones().then(setMics);
  }, []);

  return (
    <div style={{ maxWidth: 720 }}>
      <PageTitle>Settings</PageTitle>

      <Card style={{ marginBottom: 16 }}>
        <SectionTitle sub="Where your dictation is cleaned up before it's typed.">Cleanup Engine</SectionTitle>
        <Segmented
          options={MODES.map((m) => ({ value: m.value, label: m.label }))}
          value={settings.cleanup_mode}
          onChange={(v) => onChange({ ...settings, cleanup_mode: v })}
        />
        <div style={{ color: theme.textMuted, fontSize: 12.5, marginTop: 10 }}>
          {MODES.find((m) => m.value === settings.cleanup_mode)?.hint}
        </div>

        <KeyField
          label="OpenAI API key"
          configured={status.has_openai_key}
          onSave={(k) => {
            setApiKey("openai", k);
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
          onSave={(k) => {
            setApiKey("anthropic", k);
            setTimeout(refresh, 400);
          }}
        />
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

      <Card style={{ marginBottom: 16 }}>
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
          <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>
            Dictation sounds
            <div style={{ fontSize: 12, fontWeight: 400, color: theme.textMuted, marginTop: 2 }}>
              A soft click when recording starts, a lighter one when the text lands, and a duller
              one if you cancel.
            </div>
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

      <Card style={{ marginBottom: 16 }}>
        <SectionTitle sub="How you start a dictation and what WhimprFlow listens to.">
          Dictation
        </SectionTitle>
        <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          <Row
            title="Push-to-talk key"
            hint="Hold this key to dictate. Only modifier keys can be used — they are the ones the key tap can see without intercepting your typing."
          >
            <select
              value={settings.push_to_talk_key}
              onChange={(e) =>
                onChange({ ...settings, push_to_talk_key: e.currentTarget.value as PushToTalkKey })
              }
              style={selectStyle}
            >
              {PTT_KEYS.map((k) => (
                <option key={k.value} value={k.value}>
                  {k.label}
                </option>
              ))}
            </select>
          </Row>

          <Row
            title="Language"
            hint="Needs a multilingual model. ggml-large-v3-turbo is multilingual; any model ending in .en is English-only and ignores this."
          >
            <select
              value={settings.language}
              onChange={(e) => onChange({ ...settings, language: e.currentTarget.value })}
              style={selectStyle}
            >
              {LANGUAGES.map((l) => (
                <option key={l.value} value={l.value}>
                  {l.label}
                </option>
              ))}
            </select>
          </Row>

          <Row title="Microphone" hint="Falls back to the system default if the chosen device is unplugged.">
            <select
              value={settings.microphone}
              onChange={(e) => onChange({ ...settings, microphone: e.currentTarget.value })}
              style={selectStyle}
            >
              <option value="">System default</option>
              {mics.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </Row>

          <Row
            title="Noisy environments"
            hint="Explores several transcriptions instead of committing to the first guess — better in a café or with people talking nearby, and slower on every dictation."
          >
            <Segmented
              options={[
                { value: "on", label: "On" },
                { value: "off", label: "Off" },
              ]}
              value={settings.noise_robust_asr ? "on" : "off"}
              onChange={(v) => onChange({ ...settings, noise_robust_asr: v === "on" })}
            />
          </Row>
        </div>

        <div
          style={{
            marginTop: 16,
            paddingTop: 14,
            borderTop: `1px solid ${theme.border}`,
            fontSize: 12.5,
            color: theme.textMuted,
            lineHeight: 1.6,
          }}
        >
          <b style={{ color: theme.textStrong }}>For crowded rooms, turn on macOS Voice Isolation.</b>{" "}
          It uses Apple's on-device model to strip background noise and other
          people's voices before WhimprFlow ever hears the audio, and it helps far
          more than anything above. Start a dictation, then open Control Center in
          the menu bar — a <b>Mic Mode</b> control appears while the microphone is
          in use — and choose <b>Voice Isolation</b>. macOS remembers it per app.
        </div>
      </Card>

      <Card style={{ marginBottom: 16 }}>
        <SectionTitle sub="How WhimprFlow behaves as a Mac app.">System</SectionTitle>
        <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          <Row title="Launch at login" hint="Starts WhimprFlow automatically when you log in.">
            <Segmented
              options={[
                { value: "on", label: "On" },
                { value: "off", label: "Off" },
              ]}
              value={settings.launch_at_login ? "on" : "off"}
              onChange={(v) => onChange({ ...settings, launch_at_login: v === "on" })}
            />
          </Row>
          <Row title="Show in Dock" hint="Off makes WhimprFlow a menu-bar-only app.">
            <Segmented
              options={[
                { value: "on", label: "On" },
                { value: "off", label: "Off" },
              ]}
              value={settings.show_in_dock ? "on" : "off"}
              onChange={(v) => onChange({ ...settings, show_in_dock: v === "on" })}
            />
          </Row>
        </div>
      </Card>

      <Card style={{ marginBottom: 16 }}>
        <SectionTitle sub="The Flow Bar is the small pill that shows idle / recording / cleaning up. Drag it anywhere; drop it back near the bottom centre to re-anchor.">
          Flow Bar
        </SectionTitle>
        <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
            <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>
              Show the Flow Bar at all times
              <div style={{ fontSize: 12, fontWeight: 400, color: theme.textMuted, marginTop: 2 }}>
                Off hides the pill until you start dictating.
              </div>
            </div>
            <Segmented
              options={[
                { value: "on", label: "On" },
                { value: "off", label: "Off" },
              ]}
              value={settings.show_pill_always ? "on" : "off"}
              onChange={(v) => onChange({ ...settings, show_pill_always: v === "on" })}
            />
          </div>

          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
            <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>
              Follow the active display
              <div style={{ fontSize: 12, fontWeight: 400, color: theme.textMuted, marginTop: 2 }}>
                Put the pill on the screen you're working on instead of the primary one.
              </div>
            </div>
            <Segmented
              options={[
                { value: "on", label: "On" },
                { value: "off", label: "Off" },
              ]}
              value={settings.pill_follows_active_display ? "on" : "off"}
              onChange={(v) =>
                onChange({ ...settings, pill_follows_active_display: v === "on" })
              }
            />
          </div>

          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
            <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>
              Gap above the Dock
              <div style={{ fontSize: 12, fontWeight: 400, color: theme.textMuted, marginTop: 2 }}>
                Measured from the top of the Dock, not the screen edge. Currently{" "}
                {Math.round(settings.pill_bottom_inset)} pt.
              </div>
            </div>
            <input
              type="range"
              min={16}
              max={220}
              step={4}
              value={settings.pill_bottom_inset}
              onChange={(e) =>
                onChange({ ...settings, pill_bottom_inset: Number(e.currentTarget.value) })
              }
              style={{ width: 180, accentColor: theme.accent }}
            />
          </div>

          {settings.pill_pos && (
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
              <div style={{ fontSize: 13, color: theme.textMuted }}>
                The pill is pinned where you dragged it, so the settings above are paused.
              </div>
              <button
                type="button"
                onClick={() => {
                  void resetPillPosition();
                  onChange({ ...settings, pill_pos: null });
                }}
                style={{
                  cursor: "pointer",
                  border: `1px solid ${theme.borderStrong}`,
                  background: "transparent",
                  color: theme.textBody,
                  borderRadius: 8,
                  fontSize: 12.5,
                  fontWeight: 600,
                  padding: "6px 12px",
                  whiteSpace: "nowrap",
                }}
              >
                Reset position
              </button>
            </div>
          )}
        </div>
      </Card>

      <Card>
        <SectionTitle sub="Grant these to WhimprFlow, then quit and reopen the app if a dot stays grey.">
          Permissions
        </SectionTitle>
        <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          <PermRow
            ok={status.accessibility}
            label="Accessibility"
            detail={
              status.accessibility
                ? "granted — Fn works everywhere + types your words"
                : "the key one: makes Fn work in EVERY app AND types your words"
            }
            onClick={() => {
              requestAccessibility();
              setTimeout(refresh, 800);
            }}
          />
          <PermRow
            ok={status.microphone}
            label="Microphone"
            detail={status.microphone ? "granted" : "hears what you say"}
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
    </div>
  );
}
