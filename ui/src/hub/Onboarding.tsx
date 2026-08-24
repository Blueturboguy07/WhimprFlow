import { useEffect } from "react";
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

// Shown when the running build is ad-hoc signed and Accessibility reads as
// missing. That pair is almost never what it looks like: macOS attaches a
// permission grant to the *designated requirement* of whoever asked for it,
// and an ad-hoc signature's requirement is a plain hash of the binary. Rebuild
// the app and that hash changes, so the grant the user gave stops matching —
// while System Settings goes on listing WhimprFlow with its switch on, because
// that list is drawn from the bundle name and path rather than the signature.
//
// The result is a user who has genuinely granted the permission, can see it
// granted, and is told by this screen that they have not. Toggling it again
// does nothing. Saying so plainly is the only way out, so this explains the
// real cause instead of repeating the demand.
function UnsignedBuildNotice() {
  return (
    <div
      style={{
        display: "flex",
        gap: 12,
        padding: "12px 14px",
        marginBottom: 20,
        borderRadius: 10,
        background: "rgba(255,176,32,0.10)",
        border: "1px solid rgba(255,176,32,0.40)",
        lineHeight: 1.5,
      }}
    >
      <span style={{ fontSize: 15, flex: "0 0 auto" }}>⚠</span>
      <div style={{ fontSize: 12.5, color: theme.textBody }}>
        <b>This copy of WhimprFlow is not signed.</b> If System Settings already shows WhimprFlow
        switched on under Accessibility, macOS is not ignoring you — an unsigned build gets a new
        identity every time it is compiled, so the permission you granted is still attached to the
        previous build and can never match this one.
        <div style={{ marginTop: 6, color: theme.textMuted }}>
          To fix it for good, install a signed build (<code>scripts/build-macos.sh</code>, then{" "}
          <code>scripts/install-macos.sh</code>). To carry on with this one, remove the stale
          WhimprFlow entry in System Settings → Privacy &amp; Security → Accessibility with the{" "}
          <b>–</b> button, then add this app again — and expect to repeat that after every rebuild.
        </div>
      </div>
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
  // Poll live so the state flips the moment macOS applies each grant.
  useEffect(() => {
    const id = setInterval(refresh, 1200);
    return () => clearInterval(id);
  }, [refresh]);

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

        {!status.stable_identity && !acc && <UnsignedBuildNotice />}

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
          detail="Lets WhimprFlow hear what you say."
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

        <p style={{ fontSize: 12, color: theme.textFaint, lineHeight: 1.5, marginTop: 16 }}>
          If a permission stays grey after you flip it on in System Settings, toggle WhimprFlow off
          and back on in that pane — the state here will update within a second.
        </p>
      </div>
    </div>
  );
}
