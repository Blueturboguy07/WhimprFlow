import { useEffect, useRef } from "react";
import { font, palette } from "../tokens/values";
import { theme } from "./theme";
import {
  requestAccessibility,
  requestMicrophone,
  requestInputMonitoring,
  type Status,
} from "./api";

// A blocking permission gate: the app can't be used until Accessibility and
// Microphone are granted. The three permissions are presented in order (each
// unlocks the next), and their state flips live as macOS applies them.

function Step({
  n,
  title,
  detail,
  done,
  active,
  locked,
  required,
  onGrant,
}: {
  n: number;
  title: string;
  detail: string;
  done: boolean;
  active: boolean;
  locked: boolean;
  required: boolean;
  onGrant: () => void;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 16,
        padding: "16px 18px",
        borderRadius: 14,
        marginBottom: 12,
        background: active ? theme.accentSoft : theme.cardBg,
        border: `1px solid ${active ? theme.accentSoftBorder : theme.border}`,
        boxShadow: theme.shadowSoft,
        opacity: locked ? 0.5 : 1,
      }}
    >
      <div
        style={{
          flex: "0 0 auto",
          width: 30,
          height: 30,
          borderRadius: 9999,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          fontWeight: 700,
          fontSize: 14,
          color: done ? "#fff" : theme.textMuted,
          background: done ? theme.accentDeep : theme.track,
        }}
      >
        {done ? "✓" : n}
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: 15, fontWeight: 600, color: theme.textStrong }}>
          {title}{" "}
          <span style={{ fontSize: 12, color: theme.textFaint, fontWeight: 400 }}>
            {required ? "· required" : "· optional"}
          </span>
        </div>
        <div style={{ fontSize: 13, color: theme.textMuted, marginTop: 2 }}>{detail}</div>
      </div>
      {done ? (
        <span style={{ color: theme.accentDeep, fontSize: 13, fontWeight: 600 }}>Granted</span>
      ) : (
        <button
          onClick={onGrant}
          disabled={locked}
          style={{
            cursor: locked ? "default" : "pointer",
            border: "none",
            borderRadius: 10,
            padding: "9px 16px",
            fontSize: 13,
            fontWeight: 600,
            fontFamily: font.ui,
            color: "#fff",
            background: locked ? theme.textFaint : palette.slate900,
            whiteSpace: "nowrap",
          }}
        >
          Grant
        </button>
      )}
    </div>
  );
}

export function Onboarding({
  status,
  refresh,
  onEnter,
}: {
  status: Status;
  refresh: () => void;
  onEnter: () => void;
}) {
  // A backstop poll. The state actually flips on the heartbeat Rust pushes at
  // us (`permissions::watch` → `whimpr://permissions`), because this webview
  // stops running timers within seconds of its window going away — and the
  // reader grants the microphone from System Settings, i.e. with this window
  // behind it or closed to the tray.
  //
  // Held in a ref so the interval survives re-renders instead of restarting its
  // clock on each one; keyed on `[refresh]` it was rebuilt every render, and a
  // poll that keeps resetting its own timer can be starved.
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  useEffect(() => {
    const id = setInterval(() => refreshRef.current(), 1200);
    return () => clearInterval(id);
  }, []);

  const acc = status.accessibility;
  const mic = status.microphone;
  const inp = status.input_monitoring;
  const canEnter = acc && mic;

  return (
    <div
      style={{
        height: "100vh",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        background: theme.pageBg,
        color: theme.textBody,
        fontFamily: font.ui,
        padding: 24,
      }}
    >
      <div style={{ width: 560, maxWidth: "100%" }}>
        <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 6 }}>
          <div style={{ fontFamily: font.serif, fontSize: 30, fontWeight: 600, color: theme.textStrong }}>
            Set up WhimprFlow
          </div>
          <span
            style={{
              fontSize: 10,
              fontWeight: 700,
              letterSpacing: 0.4,
              textTransform: "uppercase",
              color: theme.accentDeep,
              background: theme.accentSoft,
              border: `1px solid ${theme.accentSoftBorder}`,
              borderRadius: 999,
              padding: "2px 7px",
            }}
          >
            Local
          </span>
        </div>
        <p style={{ color: theme.textMuted, lineHeight: 1.5, margin: "0 0 24px" }}>
          Grant these to <b>WhimprFlow</b>, in order. Each turns green here the moment macOS applies
          it — no relaunch needed.
        </p>

        <Step
          n={1}
          title="Accessibility"
          detail="Detects the Fn key in every app and types your words. This is the one that makes the Fn key work everywhere."
          done={acc}
          active={!acc}
          locked={false}
          required
          onGrant={() => requestAccessibility()}
        />
        <Step
          n={2}
          title="Microphone"
          // macOS can be answering about a different app entirely (whatever
          // launched us), in which case granting WhimprFlow here can never turn
          // this row green — so say whose switch actually counts rather than
          // repeating "not granted" at someone doing everything right.
          detail={status.microphone_hint ?? "Lets WhimprFlow hear what you say."}
          done={mic}
          active={acc && !mic}
          locked={!acc}
          required
          onGrant={() => requestMicrophone()}
        />
        <Step
          n={3}
          title="Input Monitoring"
          detail="Extra reliability for key detection. Optional — you can enter without it."
          done={inp}
          active={acc && mic && !inp}
          locked={!(acc && mic)}
          required={false}
          onGrant={() => requestInputMonitoring()}
        />

        <button
          onClick={onEnter}
          disabled={!canEnter}
          style={{
            marginTop: 12,
            width: "100%",
            cursor: canEnter ? "pointer" : "default",
            border: "none",
            borderRadius: 12,
            padding: "13px",
            fontSize: 15,
            fontWeight: 700,
            fontFamily: font.ui,
            color: "#fff",
            background: canEnter ? theme.accentDeep : theme.textFaint,
          }}
        >
          {canEnter ? "Enter WhimprFlow →" : "Grant Accessibility + Microphone to continue"}
        </button>

        {!canEnter && (
          <button
            onClick={onEnter}
            style={{
              marginTop: 10,
              width: "100%",
              cursor: "pointer",
              border: "none",
              background: "none",
              padding: "6px",
              fontSize: 12.5,
              fontFamily: font.ui,
              color: theme.textMuted,
              textDecoration: "underline",
            }}
          >
            Skip for now — Settings, Dictionary and history don't need these
          </button>
        )}

        <p style={{ fontSize: 12, color: theme.textFaint, lineHeight: 1.5, marginTop: 16 }}>
          If a permission stays grey after you flip it on in System Settings, toggle WhimprFlow off
          and back on in that pane — the state here will update within a second.
        </p>
      </div>
    </div>
  );
}
