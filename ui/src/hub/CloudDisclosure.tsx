import { palette } from "../tokens/values";
import { theme } from "./theme";
import { Button, Card } from "./ui";
import { publikOpenLink } from "./api";

// The one-time cost + data-path disclosure for publik API (R21 §4.3 copy,
// adapted for a Tier-2 app where Local stays the default and the user has
// just tapped "publik API" in the picker). Two disclosures, two paragraphs.
// Nothing is minted until "Turn on publik API" — that tap is the consent.
// No hourly-cost figure by design (R25 S17): the rate and the monthly line.
export function CloudDisclosure({
  onAccept,
  onOwnKey,
  onClose,
  busy,
  error,
  canProvision,
}: {
  onAccept: () => void;
  onOwnKey: () => void;
  onClose: () => void;
  busy: boolean;
  error: string | null;
  canProvision: boolean;
}) {
  return (
    <Card style={{ marginTop: 12, border: `1px solid ${theme.accentSoftBorder}`, background: theme.accentSoft }}>
      <div style={{ fontSize: 15, fontWeight: 600, color: theme.textStrong }}>Turn on publik API cleanup?</div>
      <p style={{ color: theme.textBody, fontSize: 13, lineHeight: 1.5, margin: "8px 0 0" }}>
        WhimprFlow can clean up your dictation in the cloud on <b>publik API</b>, already set up — no account and no key
        needed. Local stays your default until you turn this on.
      </p>
      <p style={{ color: theme.textBody, fontSize: 13, lineHeight: 1.5, margin: "8px 0 0" }}>
        <b>Cost.</b> Every request is priced per use at 50% of the model's published list price, from your publik balance.
        Your first $0.25 is free. Most people spend under $2 a month. You can see every charge in the app and at
        publikhq.com.
      </p>
      <p style={{ color: theme.textBody, fontSize: 13, lineHeight: 1.5, margin: "8px 0 0" }}>
        <b>Where your words go.</b> Your transcript — never audio — goes through publik's servers to a shared model
        account. publik never trains on it and does not store it. Speech-to-text stays on this Mac. You can switch to
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
    </Card>
  );
}
