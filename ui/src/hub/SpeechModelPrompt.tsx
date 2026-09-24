import { font, palette } from "../tokens/values";
import { theme } from "./theme";
import { Button } from "./ui";
import type { AsrModel } from "./api";

// The popup that installs the speech model. It opens when the Hub finds no
// model loaded, and again whenever a dictation fails for that reason (Rust
// brings the Hub forward first). One button downloads ggml-base.en from
// huggingface.co, Rust checks its SHA-256, and loads it with no relaunch.
//
// v0.2.1 had only a banner that said to download a file by hand into a hidden
// folder and relaunch — a dead end for anyone who installed from the dmg.

export const MODEL_SIZE_LABEL = "148 MB";

export function mb(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}

export function SpeechModelPrompt({
  model,
  justInstalled,
  onDownload,
  onClose,
}: {
  model: AsrModel;
  // The download finished in this session — show the success state once.
  justInstalled: boolean;
  onDownload: () => void;
  onClose: () => void;
}) {
  const downloading = model.state === "downloading";
  const failed = model.state === "failed";
  const ready = model.state === "ready";
  const pct =
    model.state === "downloading" && model.total > 0
      ? Math.min(100, Math.round((model.received / model.total) * 100))
      : 0;

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="speech-model-title"
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 50,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        padding: 16,
        background: "rgba(17,20,25,0.38)",
        fontFamily: font.ui,
      }}
    >
      <div
        style={{
          width: "100%",
          maxWidth: 460,
          background: theme.cardBg,
          border: `1px solid ${theme.border}`,
          borderRadius: 18,
          padding: 28,
          boxShadow: "0 18px 48px rgba(17,20,25,0.18)",
        }}
      >
        <h2
          id="speech-model-title"
          style={{ fontFamily: font.serif, fontSize: 24, fontWeight: 700, color: theme.textStrong, margin: 0 }}
        >
          {ready && justInstalled ? "Speech model installed" : "Download the speech model"}
        </h2>

        {ready && justInstalled ? (
          <p style={{ color: theme.textBody, fontSize: 14, lineHeight: 1.55, margin: "12px 0 0" }}>
            WhimprFlow is ready. Hold your key and speak.
          </p>
        ) : (
          <>
            <p style={{ color: theme.textBody, fontSize: 14, lineHeight: 1.55, margin: "12px 0 0" }}>
              WhimprFlow turns your voice into text with a speech model that runs on this computer.
              This copy does not have one yet, so dictation cannot work.
            </p>
            <div style={{ color: theme.textMuted, fontSize: 12.5, marginTop: 10 }}>
              ggml-base.en · {MODEL_SIZE_LABEL} · from huggingface.co · one time
            </div>
          </>
        )}

        {downloading && (
          <div style={{ marginTop: 18 }}>
            <div
              role="progressbar"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={pct}
              style={{ height: 8, borderRadius: 9999, background: theme.track, overflow: "hidden" }}
            >
              <div
                style={{
                  width: `${pct}%`,
                  height: "100%",
                  borderRadius: 9999,
                  background: theme.accentDeep,
                  transition: "width 200ms linear",
                }}
              />
            </div>
            <div style={{ display: "flex", justifyContent: "space-between", marginTop: 8, fontSize: 12.5, color: theme.textMuted }}>
              <span>
                {mb(model.received)} of {mb(model.total)}
              </span>
              <span>{pct}%</span>
            </div>
            <div style={{ fontSize: 12.5, color: theme.textMuted, marginTop: 6 }}>
              You can close this window. The download continues.
            </div>
          </div>
        )}

        {failed && (
          <div
            style={{
              marginTop: 16,
              padding: "10px 12px",
              borderRadius: 10,
              background: "rgba(255,107,107,0.12)",
              border: "1px solid rgba(255,107,107,0.35)",
              fontSize: 13,
              color: palette.slate900,
            }}
          >
            {model.message}
          </div>
        )}

        <div style={{ display: "flex", gap: 8, marginTop: 22, flexWrap: "wrap" }}>
          {ready ? (
            <Button variant="accent" onClick={onClose}>
              Done
            </Button>
          ) : downloading ? (
            <Button variant="ghost" onClick={onClose}>
              Hide
            </Button>
          ) : (
            <>
              <Button variant="accent" onClick={onDownload} disabled={model.state === "loading"}>
                {failed ? "Try again" : `Download speech model (${MODEL_SIZE_LABEL})`}
              </Button>
              <Button variant="ghost" onClick={onClose}>
                Not now
              </Button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
