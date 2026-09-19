import { useCallback, useEffect, useRef, useState } from "react";
import { font, palette } from "../tokens/values";
import { theme } from "./theme";
import { Onboarding } from "./Onboarding";
import { Sidebar, type Page } from "./Sidebar";
import { Home } from "./Home";
import { Insights } from "./Insights";
import { DictionaryPane } from "./DictionaryPane";
import { SettingsPane } from "./SettingsPane";
import { Help } from "./Help";
import { ComingSoon } from "./ComingSoon";
import type { IconName } from "./icons";
import {
  getSettings,
  setSettings,
  getStatus,
  getLastError,
  getPublikStatus,
  onPermissions,
  onPublik,
  publikDismissNotice,
  publikOpenLink,
  publikRefreshWallet,
  requestAccessibility,
  type PublikNotice,
  type PublikStatus,
  type Settings,
  type Status,
  type LastError,
  DEFAULT_SETTINGS,
  UNKNOWN_PUBLIK,
  UNKNOWN_STATUS,
} from "./api";

// A slim, dismissible warning strip shown above the Hub content whenever
// something is stopping dictation from reaching the cursor — either a
// permission that lapsed after the onboarding gate was already passed (e.g.
// a rebuild invalidated a stale macOS Accessibility grant), or the last loud
// diagnostic reported by the dictation pipeline (`diag::report` in
// src-tauri). Without this, a permission revoked (or a hotkey tap that died)
// mid-session was previously invisible outside the terminal — see the
// "text is not writing where the cursor is" bug reports.
// The publik API banner (CONTRACT §12.3): a 402, or the free starter running
// low. Non-blocking — dictation goes on, raw text is pasted — with the message
// from the response and exactly one link (`top_up_url`), resolved in Rust.
function PublikBanner({ notice, onDismiss }: { notice: PublikNotice; onDismiss: () => void }) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 14,
        padding: "10px 20px",
        background: theme.accentSoft,
        borderBottom: `1px solid ${theme.accentSoftBorder}`,
        fontFamily: font.ui,
      }}
    >
      <div style={{ flex: 1, minWidth: 0 }}>
        <span style={{ fontSize: 13, fontWeight: 700, color: palette.slate900 }}>{notice.headline}</span>
        <span style={{ fontSize: 13, color: theme.textMuted, marginLeft: 8 }}>{notice.message}</span>
      </div>
      <button
        onClick={() => void publikOpenLink("notice")}
        style={{
          flex: "0 0 auto",
          cursor: "pointer",
          border: "none",
          borderRadius: 8,
          padding: "6px 12px",
          fontSize: 12.5,
          fontWeight: 600,
          fontFamily: font.ui,
          background: theme.accentDeep,
          color: "#fff",
        }}
      >
        {notice.link_label}
      </button>
      <button
        onClick={onDismiss}
        aria-label="Dismiss"
        style={{ flex: "0 0 auto", cursor: "pointer", border: "none", background: "transparent", color: theme.textMuted, fontSize: 16 }}
      >
        ×
      </button>
    </div>
  );
}

function ErrorBanner({
  headline,
  detail,
  actionLabel,
  onAction,
  onDismiss,
}: {
  headline: string;
  detail: string;
  actionLabel?: string;
  onAction?: () => void;
  onDismiss: () => void;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 14,
        padding: "10px 20px",
        background: "rgba(255,107,107,0.12)",
        borderBottom: `1px solid rgba(255,107,107,0.35)`,
        fontFamily: font.ui,
      }}
    >
      <span style={{ fontSize: 15, flex: "0 0 auto" }}>⚠</span>
      <div style={{ flex: 1, minWidth: 0 }}>
        <span style={{ fontSize: 13, fontWeight: 700, color: palette.slate900 }}>{headline}</span>
        <span style={{ fontSize: 13, color: theme.textMuted, marginLeft: 8 }}>{detail}</span>
      </div>
      {actionLabel && onAction && (
        <button
          onClick={onAction}
          style={{
            flex: "0 0 auto",
            cursor: "pointer",
            border: "none",
            borderRadius: 8,
            padding: "6px 12px",
            fontSize: 12.5,
            fontWeight: 600,
            fontFamily: font.ui,
            color: "#fff",
            background: palette.error,
          }}
        >
          {actionLabel}
        </button>
      )}
      <button
        onClick={onDismiss}
        aria-label="Dismiss"
        style={{
          flex: "0 0 auto",
          cursor: "pointer",
          border: "none",
          background: "transparent",
          color: theme.textFaint,
          fontSize: 14,
          padding: 4,
        }}
      >
        ✕
      </button>
    </div>
  );
}

// Placeholder screens that are routed but not yet built.
const SOON: Partial<Record<Page, { icon: IconName; title: string; desc: string }>> = {
  snippets: {
    icon: "snippets",
    title: "Snippets",
    desc: "Save reusable phrases and expand them by voice — signatures, addresses, boilerplate.",
  },
  style: {
    icon: "style",
    title: "Style",
    desc: "Tune WhimprFlow's tone and formatting so cleaned-up text always sounds like you.",
  },
  transforms: {
    icon: "transforms",
    title: "Transforms",
    desc: "Turn a quick spoken thought into an email, a summary, or a to-do with one command.",
  },
  scratchpad: {
    icon: "scratchpad",
    title: "Scratchpad",
    desc: "A quiet place to dictate long-form and shape it before it lands anywhere else.",
  },
};

export function App() {
  const [page, setPage] = useState<Page>("home");
  const [settings, setLocalSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [entered, setEntered] = useState(() => {
    try { return localStorage.getItem("whimpr_onboarding_done") === "1"; } catch { return false; }
  });
  const [status, setStatus] = useState<Status>(UNKNOWN_STATUS);
  const [lastError, setLastError] = useState<LastError | null>(null);
  const [errorDismissed, setErrorDismissed] = useState(false);
  const [publik, setPublik] = useState<PublikStatus>(UNKNOWN_PUBLIK);

  const markEntered = () => {
    try { localStorage.setItem("whimpr_onboarding_done", "1"); } catch { /* ignore */ }
    setEntered(true);
  };

  // Stable across renders. It used to be rebuilt on every render, which tore
  // down and restarted any interval keyed on it — a poll that resets its own
  // clock on every tick is a poll that can be starved.
  const refresh = useCallback(
    () =>
      getStatus().then((s) => {
        setStatus(s);
        // Auto-enter if both required permissions are already granted so the user
        // never sees the Onboarding gate on a re-open after a successful setup.
        if (s.accessibility && s.microphone) markEntered();
      }),
    [],
  );
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;

  useEffect(() => {
    getSettings().then(setLocalSettings);
    refresh();
    getLastError().then(setLastError);
    // Cheap, no network: whether publik is available in this build and
    // whether a key already exists. The wallet is fetched only from Settings.
    getPublikStatus().then(setPublik);
  }, []);

  // Rust owns the publik fields of Settings (install id, base URL, model,
  // claim link) and writes them behind the Hub's back — on provisioning, on a
  // wallet refresh, on a silent re-mint. Every `onChange` from the Hub sends
  // the whole Settings object back, so the local copy must be re-read after
  // any of those, or a later toggle would overwrite them with stale values.
  const reloadSettings = useCallback(() => getSettings().then(setLocalSettings), []);

  // The publik balance line moves the moment a cleanup settles (the gateway
  // stamps the charge on every answer) and on a 402 / revoked key.
  useEffect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    void onPublik((p) => {
      setPublik(p);
      void reloadSettings();
    }).then((u) => (gone ? u() : (stop = u)));
    return () => {
      gone = true;
      stop?.();
    };
  }, [reloadSettings]);

  // Status + (throttled in Rust) GET /wallet — the Settings pane's open moment.
  const refreshPublik = useCallback(() => {
    void publikRefreshWallet().then((p) => {
      setPublik(p);
      void reloadSettings();
    });
  }, [reloadSettings]);

  // The permission heartbeat lives in Rust now (`permissions::watch`) and is
  // pushed here the instant macOS changes its mind. That matters because the
  // reader grants the microphone from *System Settings*, with this window
  // behind it or closed to the tray — and a webview that isn't rendering runs
  // no timers at all. (Measured on 0.1.1: hide the Hub and its status calls
  // stop 4.4s later and never resume, while a Rust thread keeps ticking every
  // half second.) Waiting on our own setInterval was half of why "it didn't
  // recognize that I had given it microphone permissions" happened.
  useEffect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    void onPermissions((p) => {
      setStatus((prev) => {
        const next = { ...prev, ...p };
        if (next.accessibility && next.microphone) markEntered();
        return next;
      });
    }).then((u) => (gone ? u() : (stop = u)));
    return () => {
      gone = true;
      stop?.();
    };
  }, []);

  // Coming back to the window is the other moment the reader expects the truth:
  // they just flipped a switch in System Settings and tabbed back here.
  useEffect(() => {
    const sync = () => void refreshRef.current();
    window.addEventListener("focus", sync);
    document.addEventListener("visibilitychange", sync);
    return () => {
      window.removeEventListener("focus", sync);
      document.removeEventListener("visibilitychange", sync);
    };
  }, []);

  // Backstop poll for the whole session, not just during onboarding — a
  // permission can lapse after `entered` is already true (a rebuild with a new
  // ad-hoc signature invalidates a prior macOS Accessibility grant; see the
  // "stale TCC entry" case in hotkey.rs), and that used to go completely
  // unnoticed until the user dug through logs.
  useEffect(() => {
    const id = setInterval(() => void refreshRef.current(), 5000);
    return () => clearInterval(id);
  }, []);

  // Live-update the moment the dictation pipeline reports a failure
  // (`diag::report` in src-tauri), instead of waiting for the next poll.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen<LastError>("whimpr://error", (e) => {
          setLastError(e.payload);
          setErrorDismissed(false);
        }),
      )
      .then((u) => (unlisten = u))
      .catch(() => {});
    return () => unlisten?.();
  }, []);

  const update = (s: Settings) => {
    setLocalSettings(s);
    void setSettings(s);
  };

  // Gate the app behind the setup wizard until the required permissions are granted.
  if (!(status.accessibility && status.microphone) && !entered) {
    return <Onboarding status={status} refresh={refresh} onEnter={markEntered} />;
  }

  const soon = SOON[page];

  // Two independent reasons for the post-onboarding banner: Accessibility
  // lapsed after entry (checked live against `status`, not just the one-time
  // onboarding gate), or the pipeline reported some other failure (hotkey tap
  // dead, paste failed, empty transcript, …).
  const accessibilityLapsed = entered && !status.accessibility;
  const banner = errorDismissed
    ? null
    : accessibilityLapsed
      ? {
          headline: "Accessibility permission needed",
          detail: "WhimprFlow can no longer type into other apps — grant it again to keep dictating.",
          actionLabel: "Grant Accessibility",
          onAction: () => requestAccessibility(),
        }
      : lastError
        ? { headline: lastError.headline, detail: lastError.detail }
        : null;

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100vh",
        fontFamily: font.ui,
        color: theme.textBody,
        background: theme.pageBg,
      }}
    >
      {banner && (
        <ErrorBanner
          headline={banner.headline}
          detail={banner.detail}
          actionLabel={banner.actionLabel}
          onAction={banner.onAction}
          onDismiss={() => setErrorDismissed(true)}
        />
      )}
      {publik.notice && <PublikBanner notice={publik.notice} onDismiss={() => void publikDismissNotice().then(setPublik)} />}
      <div style={{ display: "flex", flex: 1, minHeight: 0 }}>
        <Sidebar page={page} setPage={setPage} />
        <main style={{ flex: 1, minWidth: 0, overflowY: "auto" }}>
          <div style={{ padding: "36px 44px", margin: "0 auto", maxWidth: 1120 }}>
            {page === "home" && <Home />}
            {page === "insights" && <Insights />}
            {page === "dictionary" && <DictionaryPane />}
            {page === "settings" && (
              <SettingsPane
                settings={settings}
                onChange={update}
                status={status}
                refresh={refresh}
                publik={publik}
                refreshPublik={refreshPublik}
                setPublik={setPublik}
                reloadSettings={reloadSettings}
              />
            )}
            {page === "help" && <Help />}
            {soon && <ComingSoon icon={soon.icon} title={soon.title} desc={soon.desc} />}
          </div>
        </main>
      </div>
    </div>
  );
}
