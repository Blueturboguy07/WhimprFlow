import { font } from "../tokens/values";
import { theme } from "./theme";
import { Icon, type IconName } from "./icons";

export type Page =
  | "home"
  | "insights"
  | "dictionary"
  | "snippets"
  | "style"
  | "transforms"
  | "scratchpad"
  | "settings"
  | "help";

type NavDef = { key: Page; label: string; icon: IconName };

const MAIN: NavDef[] = [
  { key: "home", label: "Home", icon: "home" },
  { key: "insights", label: "Insights", icon: "insights" },
  { key: "dictionary", label: "Dictionary", icon: "dictionary" },
  { key: "snippets", label: "Snippets", icon: "snippets" },
  { key: "style", label: "Style", icon: "style" },
  { key: "transforms", label: "Transforms", icon: "transforms" },
  { key: "scratchpad", label: "Scratchpad", icon: "scratchpad" },
];

const BOTTOM: NavDef[] = [
  { key: "settings", label: "Settings", icon: "settings" },
  { key: "help", label: "Help", icon: "help" },
];

function NavItem({ item, active, onClick }: { item: NavDef; active: boolean; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 11,
        width: "100%",
        textAlign: "left",
        border: "none",
        cursor: "pointer",
        padding: "9px 11px",
        borderRadius: 10,
        fontSize: 13.5,
        fontFamily: font.ui,
        fontWeight: active ? 600 : 500,
        color: active ? theme.accentDeep : theme.textBody,
        background: active ? theme.accentSoft : "transparent",
        transition: "background 120ms ease, color 120ms ease",
      }}
    >
      <Icon name={item.icon} size={18} style={{ color: active ? theme.accentDeep : theme.textMuted }} />
      {item.label}
    </button>
  );
}

// `badge`: the cleanup engine that works now (engineBadge), so it never says "Local" next to "runs on publik API".
export function Sidebar({ page, setPage, badge = "Local" }: { page: Page; setPage: (p: Page) => void; badge?: string | null }) {
  return (
    <aside
      style={{
        width: 230,
        flex: "0 0 230px",
        borderRight: `1px solid ${theme.border}`,
        background: theme.sidebarBg,
        display: "flex",
        flexDirection: "column",
        padding: "20px 14px 16px",
      }}
    >
      {/* Wordmark + engine badge */}
      <div style={{ display: "flex", alignItems: "center", gap: 9, padding: "0 8px 20px" }}>
        <span
          style={{
            fontFamily: font.serif,
            fontSize: 20,
            fontWeight: 600,
            letterSpacing: -0.3,
            color: theme.textStrong,
          }}
        >
          WhimprFlow
        </span>
        {badge && (
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
            {badge}
          </span>
        )}
      </div>

      <nav style={{ display: "flex", flexDirection: "column", gap: 3 }}>
        {MAIN.map((n) => (
          <NavItem key={n.key} item={n} active={page === n.key} onClick={() => setPage(n.key)} />
        ))}
      </nav>

      <div style={{ flex: 1 }} />

      <nav style={{ display: "flex", flexDirection: "column", gap: 3, paddingTop: 12, borderTop: `1px solid ${theme.border}` }}>
        {BOTTOM.map((n) => (
          <NavItem key={n.key} item={n} active={page === n.key} onClick={() => setPage(n.key)} />
        ))}
      </nav>
    </aside>
  );
}
