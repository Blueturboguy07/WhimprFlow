import { palette } from "../tokens/values";
import { theme } from "./theme";
import { Button, Card } from "./ui";
import { publikOpenLink } from "./api";
import { PUBLIK_FRONT } from "./publikFront";

// The one-time cost + data-path disclosure for publik API (R21 §4.3 copy,
// adapted for a Tier-2 app where Local stays the default and the user has
// just tapped "publik API" in the picker). Two disclosures, two paragraphs.
// Nothing is minted until "Turn on publik API" — that tap is the consent.
// A new computer starts at $0.00 (publik migration 0059, founder 2026-09-28):
// the one free thing is $0.05 of use when it is linked to a publik account,
// once per account, so this card promises nothing else.
// No hourly-cost figure by design (R25 S17): the rate and the monthly line.
//
// variant "front" (publik first): the same consent, shorter, on the first screen a new person sees (Home, and the
// setup screen on macOS): "WhimprFlow runs on publik API", no key needed, one "Connect publik" button.
export function CloudDisclosure({
  onAccept,
  onOwnKey,
  onClose,
  busy,
  error,
  canProvision,
  variant = "settings",
  reconnect = false,
}: {
  onAccept: () => void;
  onOwnKey: () => void;
  onClose: () => void;
  busy: boolean;
  error: string | null;
  canProvision: boolean;
  variant?: "settings" | "front";
  reconnect?: boolean;
}) {
  if (variant === "front") {
    return (
      <Card style={{ marginTop: 12, border: `1px solid ${theme.accentSoftBorder}`, background: theme.accentSoft }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <div style={{ fontSize: 17, fontWeight: 700, color: theme.textStrong }}>{PUBLIK_FRONT.title}</div>
          <span
            style={{
              fontSize: 10,
              fontWeight: 700,
              letterSpacing: 0.4,
              textTransform: "uppercase",
              color: theme.accentDeep,
              border: `1px solid ${theme.accentSoftBorder}`,
              borderRadius: 999,
              padding: "2px 7px",
            }}
          >
            No key needed
          </span>
        </div>
        <p style={{ color: theme.textBody, fontSize: 13.5, lineHeight: 1.5, margin: "8px 0 0" }}>{PUBLIK_FRONT.lead}</p>
        <p style={{ color: theme.textMuted, fontSize: 12.5, lineHeight: 1.5, margin: "8px 0 0" }}>
          <b>Cost.</b> {PUBLIK_FRONT.cost} <b>Where your words go.</b> {PUBLIK_FRONT.data}
        </p>
        {!canProvision && (
          <div style={{ fontSize: 12.5, color: palette.error, marginTop: 8 }}>
            publik API is not available in this build (no app token was compiled in). Use Local or your own key.
          </div>
        )}
        {error && <div style={{ fontSize: 12.5, color: palette.error, marginTop: 8 }}>{error}</div>}
        <div style={{ display: "flex", gap: 8, marginTop: 12, flexWrap: "wrap", alignItems: "center" }}>
          <Button variant="accent" onClick={onAccept} disabled={busy || !canProvision}>
            {busy ? "Connecting…" : reconnect ? PUBLIK_FRONT.reconnect : PUBLIK_FRONT.button}
          </Button>
          <Button variant="ghost" size="sm" onClick={onOwnKey} disabled={busy}>
            {PUBLIK_FRONT.ownKey}
          </Button>
          <Button variant="ghost" size="sm" onClick={onClose} disabled={busy}>
            {PUBLIK_FRONT.later}
          </Button>
        </div>
        <Terms />
      </Card>
    );
  }
  return (
    <Card style={{ marginTop: 12, border: `1px solid ${theme.accentSoftBorder}`, background: theme.accentSoft }}>
      <div style={{ fontSize: 15, fontWeight: 600, color: theme.textStrong }}>Turn on publik API cleanup?</div>
      <p style={{ color: theme.textBody, fontSize: 13, lineHeight: 1.5, margin: "8px 0 0" }}>
        WhimprFlow can clean up your dictation in the cloud on <b>publik API</b>, already set up — no key needed. Local
        stays your default until you turn this on.
      </p>
      <p style={{ color: theme.textBody, fontSize: 13, lineHeight: 1.5, margin: "8px 0 0" }}>
        <b>Cost.</b> Every request is priced per use at 50% of the model's published list price, from your publik balance.
        Your balance starts at $0.00. Linking this computer to your publik account gives $0.05 of free use, once; a plan,
        a pack or your own key takes it from there. Most people spend under $2 a month. You can see every charge in the
        app and at publikhq.com.
      </p>
      <p style={{ color: theme.textBody, fontSize: 13, lineHeight: 1.5, margin: "8px 0 0" }}>
        <b>Where your words go.</b> Your transcript — never audio — goes through publik's servers to a shared model
        account. publik never trains on it and does not store it. Speech-to-text stays on this computer. You can switch to
        your own key at any time in Settings.
      </p>
      {!canProvision && (
        <div style={{ fontSize: 12.5, color: palette.error, marginTop: 8 }}>
          publik API is not available in this build (no app token was compiled in). Use Local or your own key.
        </div>
      )}
      {error && <div style={{ fontSize: 12.5, color: palette.error, marginTop: 8 }}>{error}</div>}
      <div style={{ display: "flex", gap: 8, marginTop: 12, flexWrap: "wrap" }}>
        <Button onClick={onAccept} disabled={busy || !canProvision}>
          {busy ? "Setting up…" : "Turn on publik API"}
        </Button>
        <Button variant="ghost" onClick={onOwnKey} disabled={busy}>
          Use my own key instead
        </Button>
        <Button variant="ghost" onClick={onClose} disabled={busy}>
          Not now
        </Button>
      </div>
      <Terms />
    </Card>
  );
}

function Terms() {
  return (
    <div style={{ fontSize: 12, color: theme.textMuted, marginTop: 10 }}>
      By continuing you agree to the{" "}
      <a
        href="https://publikhq.com/terms#api"
        onClick={(e) => {
          e.preventDefault();
          void publikOpenLink("terms");
        }}
        style={{ color: theme.accentDeep }}
      >
        publik API terms
      </a>
      .
    </div>
  );
}
