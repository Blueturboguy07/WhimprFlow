import { useState } from "react";
import { font } from "../tokens/values";
import { theme } from "./theme";
import { Button, Card, Dot, PageTitle, Segmented } from "./ui";
import {
  requestAccessibility,
  requestInputMonitoring,
  requestMicrophone,
  setApiKey,
  type CleanupLevel,
  type CleanupMode,
  type Settings,
  type Status,
} from "./api";

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
    </div>
  );
}
