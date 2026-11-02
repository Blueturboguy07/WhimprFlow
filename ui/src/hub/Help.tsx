import { font } from "../tokens/values";
import { theme } from "./theme";
import { Card, PageTitle } from "./ui";
import { platformWords, type Status } from "./api";

type Tip = { emoji: string; title: string; body: string };

// The tips name THIS platform's key and computer. Bug fix: the Windows Hub
// said "Fn by default" and "your Mac" (0.2.2, GitHub Actions run 41374831510);
// the Windows key is Right Ctrl, and Fn is not a key its hook can see.
export function helpTips(status: Pick<Status, "platform" | "push_to_talk_label">): Tip[] {
  const words = platformWords(status.platform);
  const key = status.push_to_talk_label;
  return [
    {
      emoji: "🎙️",
      title: "Hold to dictate",
      body: `Press and hold ${key}, speak naturally, then release. WhimprFlow transcribes on-device — nothing leaves your ${words.computer} unless you choose a cloud cleanup engine.`,
    },
    {
      emoji: "✨",
      title: "Cleanup happens where your cursor is",
      body: "Release the key and your cleaned-up text is typed straight into whatever app has focus — email, chat, notes, code. Choose how aggressive the cleanup is under Settings → Auto Cleanup.",
    },
    {
      emoji: "📖",
      title: "Teach it your vocabulary",
      body: 'Open Dictionary and add names, jargon, or acronyms it keeps mishearing. Add the correct spelling plus any "also heard as" variants and WhimprFlow will fix them automatically.',
    },
    {
      emoji: "🔑",
      title: "Pick a cleanup engine",
      body: `Under Settings → Cleanup Engine, run fully offline (Local), paste exactly what you said (Raw), or add an OpenAI / Anthropic key for cloud cleanup. publik API is the built-in cloud option — already set up, priced per use at 50% of the model's published list price, with a cost and data-path notice shown before it is turned on. Keys are stored in your ${words.keychain}.`,
    },
  ];
}

export function Help({ status }: { status: Status }) {
  const tips = helpTips(status);
  return (
    <div style={{ maxWidth: 720 }}>
      <PageTitle sub="A few tips to get the most out of WhimprFlow.">Help</PageTitle>
      <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
        {tips.map((t) => (
          <Card key={t.title}>
            <div style={{ display: "flex", gap: 14 }}>
              <div style={{ fontSize: 22, lineHeight: 1.2 }}>{t.emoji}</div>
              <div>
                <div
                  style={{
                    fontFamily: font.ui,
                    fontSize: 15,
                    fontWeight: 600,
                    color: theme.textStrong,
                    marginBottom: 4,
                  }}
                >
                  {t.title}
                </div>
                <div style={{ fontSize: 13.5, lineHeight: 1.55, color: theme.textMuted }}>{t.body}</div>
              </div>
            </div>
          </Card>
        ))}
      </div>
    </div>
  );
}
