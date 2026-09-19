import { theme } from "./theme";
import { Button, Card } from "./ui";
import { publikDismissFirstRun, publikOpenLink, type FirstRunCard, type PublikStatus } from "./api";

// The card the app shows the moment `POST /installs` has succeeded (CONTRACT
// §12.1) — not the disclosure sheet, which came before and had no balance on
// it. Three things, in this order, all read from Rust so the copy is the same
// everywhere: (a) the balance line from the mint response, (b) the one
// justification sentence, (c) "Link this computer & pick a plan" on the
// response's claim_url (publikhq.com only — Rust refuses anything else), with
// "Later" keeping the free starter and changing nothing.
//
// Never a silent starter (§12.4): the card stays owed — across relaunches —
// until one of its two buttons is tapped.
export function PublikFirstRun({ card, onSettled }: { card: FirstRunCard; onSettled: (p: PublikStatus) => void }) {
  return (
    <Card style={{ marginTop: 12, border: `1px solid ${theme.accentSoftBorder}`, background: theme.accentSoft }}>
      <div style={{ fontSize: 15, fontWeight: 600, color: theme.textStrong }}>publik API is on</div>
      <div style={{ fontSize: 14, fontWeight: 600, color: theme.accentDeep, marginTop: 8 }}>{card.balance_line}</div>
      <p style={{ color: theme.textBody, fontSize: 13, lineHeight: 1.5, margin: "8px 0 0" }}>{card.justification}</p>
      <div style={{ display: "flex", gap: 8, marginTop: 12, flexWrap: "wrap" }}>
        <Button
          variant="accent"
          onClick={() => {
            // Opens claim_url in the system browser and settles the card;
            // the settings card keeps "Pick a plan" until the computer is claimed.
            void publikOpenLink("first_run").then(publikDismissFirstRun).then(onSettled);
          }}
        >
          {card.cta_label}
        </Button>
        <Button variant="ghost" onClick={() => void publikDismissFirstRun().then(onSettled)}>
          {card.later_label}
        </Button>
      </div>
      <div style={{ fontSize: 12, color: theme.textMuted, marginTop: 10 }}>
        "{card.later_label}" keeps the free starter; you can pick a plan any time from this card in Settings.
      </div>
    </Card>
  );
}
