import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { CloudDisclosure } from "./CloudDisclosure";
import { PublikFirstRun } from "./PublikFirstRun";
import { theme } from "./theme";
import { Button, Card } from "./ui";
import { publikAcceptDisclosure, type PublikStatus, type Settings, type Status } from "./api";
import { PUBLIK_FRONT, frontLater, frontNow, publikFront, setFrontLater, subscribeFrontLater } from "./publikFront";

/** The one stored "Not now" flag (Home, setup screen and Settings read the same one). */
export function useFrontLater(): boolean {
  return useSyncExternalStore(subscribeFrontLater, frontLater);
}

// publik first on the first screen (Home, and the macOS setup screen): "WhimprFlow runs on publik API", no key
// needed, one "Connect publik" button. The button is the existing consent + `publik_accept_disclosure` (mint ->
// keychain -> cleanup mode = publik); right after it, the existing first-run card with "Link this computer & pick
// a plan" (claim_url on publikhq.com) shows here too. "Use my own key instead" opens the own-key fields in Settings
// and, like "Not now", keeps the card away (one stored flag).
// `compact` (the macOS setup screen, under the permissions): one line and the button first, so it fits the window;
// the button opens the full card (cost, where your words go, terms), whose button is the consent.
export function PublikFrontCard({
  settings,
  publik,
  status,
  setPublik,
  reloadSettings,
  onOwnKey,
  compact = false,
}: {
  settings: Settings;
  publik: PublikStatus;
  status: Status;
  setPublik: (p: PublikStatus) => void;
  reloadSettings: () => Promise<void>;
  onOwnKey: () => void;
  compact?: boolean;
}) {
  const later = useFrontLater();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(!compact);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (open && compact) box.current?.scrollIntoView({ behavior: "smooth", block: "nearest" });
  }, [open, compact]);
  const front = publikFront(settings, publik, status);

  if (front === "first_run" && publik.first_run) {
    return <PublikFirstRun card={publik.first_run} onSettled={setPublik} />;
  }
  if (front === "none" || (later && front === "connect")) return null;

  const ownKey = () => {
    setFrontLater();
    onOwnKey();
  };
  const accept = async () => {
    setBusy(true);
    setError(null);
    try {
      const p = await publikAcceptDisclosure();
      setPublik(p);
      await reloadSettings();
    } catch (e) {
      setError(typeof e === "string" ? e : e instanceof Error ? e.message : "Could not connect publik.");
    } finally {
      setBusy(false);
    }
  };

  if (!open) {
    return (
      <Card style={{ marginTop: 12, border: `1px solid ${theme.accentSoftBorder}`, background: theme.accentSoft }}>
        <div style={{ fontSize: 16, fontWeight: 700, color: theme.textStrong }}>{PUBLIK_FRONT.title}</div>
        <p style={{ color: theme.textBody, fontSize: 13.5, lineHeight: 1.5, margin: "4px 0 0" }}>{PUBLIK_FRONT.short}</p>
        <div style={{ display: "flex", gap: 8, marginTop: 10, flexWrap: "wrap", alignItems: "center" }}>
          <Button variant="accent" onClick={() => setOpen(true)}>
            {front === "reconnect" ? PUBLIK_FRONT.reconnect : PUBLIK_FRONT.button}
          </Button>
          <Button variant="ghost" size="sm" onClick={ownKey}>
            {PUBLIK_FRONT.ownKey}
          </Button>
        </div>
      </Card>
    );
  }

  return (
    <div ref={box}>
      <CloudDisclosure
        variant="front"
        reconnect={front === "reconnect"}
        now={frontNow(settings, publik, status)}
        onAccept={() => void accept()}
        onOwnKey={ownKey}
        onClose={setFrontLater}
        busy={busy}
        error={error}
        canProvision={publik.can_provision || publik.has_key}
      />
    </div>
  );
}
