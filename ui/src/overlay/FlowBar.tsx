import { useEffect, useRef, useState } from "react";
import { palette, pillFill, geometry, font } from "../tokens/values";

// Visual states, mirroring the Rust `BarState`.
export type BarState =
  | "idle"
  | "recording"
  | "locked"
  | "transcribing"
  | "done"
  | "cancelled"
  | "error";

type StateEvent = { state: BarState };
type WaveformEvent = { bars: number[] };
// Mirrors `diag::ErrorDto` in src-tauri/src/diag.rs.
type ErrorEvent = { headline: string; detail: string };

async function tauriListen<T>(event: string, cb: (payload: T) => void): Promise<() => void> {
  try {
    const { listen } = await import("@tauri-apps/api/event");
    return await listen<T>(event, (e) => cb(e.payload as T));
  } catch {
    return () => {};
  }
}

// ── Dragging ─────────────────────────────────────────────────────────────────
// The overlay is a borderless, unfocusable window, so it can only be moved by
// asking the OS to start a native drag. We can't observe mouseup afterwards
// (the OS owns the mouse during the drag), so the final resting position is
// read back on a short delay and persisted to settings.
async function beginDrag() {
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    await win.startDragging();

    // Poll briefly until the position stops changing, then save it once.
    let last = "";
    let stable = 0;
    for (let i = 0; i < 40; i++) {
      await new Promise((r) => setTimeout(r, 100));
      const p = await win.outerPosition();
      const key = `${p.x},${p.y}`;
      if (key === last) {
        if (++stable >= 2) break;
      } else {
        stable = 0;
        last = key;
      }
    }
    const p = await win.outerPosition();
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("set_pill_position", { x: p.x, y: p.y });
  } catch {
    /* browser preview — no window to drag */
  }
}

// A row of dot-like rounded bars driven by mic RMS — Wispr's dotted-waveform look:
// small dots when quiet, rising into a waveform when speaking.
function DottedWaveform({ bars }: { bars: number[] }) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const barsRef = useRef<number[]>(bars);
  barsRef.current = bars;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    let raf = 0;
    const N = 16;
    const draw = () => {
      const dpr = window.devicePixelRatio || 1;
      const w = canvas.clientWidth;
      const h = canvas.clientHeight;
      if (canvas.width !== w * dpr || canvas.height !== h * dpr) {
        canvas.width = w * dpr;
        canvas.height = h * dpr;
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      const dotW = 2.4;
      const gap = (w - N * dotW) / (N - 1);
      const t = performance.now();
      ctx.fillStyle = palette.waveBar;
      for (let i = 0; i < N; i++) {
        const real = barsRef.current[barsRef.current.length - 1 - (i % barsRef.current.length)];
        // Idle shimmer so the dotted line reads as "listening" even in near-silence.
        const shimmer = 0.12 + 0.06 * Math.abs(Math.sin(t / 260 + i * 0.7));
        const amp = Math.max(shimmer, real ?? 0);
        const bh = 3 + amp * 20; // 3px dot → up to ~23px bar
        const x = i * (dotW + gap);
        const y = (h - bh) / 2;
        ctx.beginPath();
        ctx.roundRect(x, y, dotW, bh, dotW / 2);
        ctx.fill();
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  return <canvas ref={canvasRef} style={{ width: "100%", height: 28 }} />;
}

async function pillCommand(cmd: "pill_cancel" | "pill_stop" | "pill_start") {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke(cmd);
  } catch {
    /* browser preview */
  }
}

/// Stop propagation so a press on a button never reaches the pill's drag handler.
function buttonHandlers(onActivate: () => void) {
  return {
    onMouseDown: (e: React.MouseEvent) => {
      e.stopPropagation();
      e.preventDefault();
    },
    onClick: (e: React.MouseEvent) => {
      e.stopPropagation();
      onActivate();
    },
  };
}

// ── Hover action cluster ─────────────────────────────────────────────────────
const LANG_CYCLE = ["en", "hi", "gu", "auto"] as const;
const LANG_NAMES: Record<string, string> = {
  en: "EN",
  hi: "हिं",
  gu: "ગુ",
  auto: "Auto",
};
function langLabel(code: string) {
  return LANG_NAMES[code] ?? code.toUpperCase();
}

const KEY_LABELS: Record<string, string> = {
  fn: "fn",
  right_command: "⌘",
  right_option: "⌥",
  right_control: "⌃",
};

function GlobeIcon() {
  return (
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8">
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3c2.5 2.7 2.5 15.3 0 18M12 3c-2.5 2.7-2.5 15.3 0 18" />
    </svg>
  );
}

function MicIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor">
      <rect x="9" y="2.5" width="6" height="11" rx="3" />
      <path
        d="M5.5 11a6.5 6.5 0 0 0 13 0"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.9"
        strokeLinecap="round"
      />
      <path d="M11.1 18h1.8v3.2h-1.8z" />
    </svg>
  );
}

function NoteIcon() {
  return (
    <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8">
      <path d="M5 3.5h14v12l-4.5 4.5H5z" strokeLinejoin="round" />
      <path d="M19 15.5h-4.5V20" strokeLinejoin="round" />
    </svg>
  );
}

function RoundButton({
  children,
  onActivate,
  onEnter,
  title,
  primary = false,
  active = false,
}: {
  children: React.ReactNode;
  onActivate: () => void;
  onEnter: () => void;
  title: string;
  primary?: boolean;
  active?: boolean;
}) {
  const size = primary ? 46 : 40;
  return (
    <div
      title={title}
      onMouseEnter={onEnter}
      // Swallow mousedown so the pill's press-and-hold drag never starts here.
      onMouseDown={(e) => {
        e.stopPropagation();
        e.preventDefault();
      }}
      onClick={(e) => {
        e.stopPropagation();
        onActivate();
      }}
      style={{
        width: size,
        height: size,
        borderRadius: 9999,
        background: active ? palette.accent500 : pillFill.base,
        border: `1px solid rgba(255,255,255,${active ? 0.25 : 0.1})`,
        boxShadow: pillFill.shadow,
        color: active ? "#08201E" : palette.pillText,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        cursor: "pointer",
        flex: "0 0 auto",
      }}
    >
      {children}
    </div>
  );
}

function CancelButton() {
  return (
    <div
      title="Cancel (Esc)"
      {...buttonHandlers(() => void pillCommand("pill_cancel"))}
      style={{
        cursor: "pointer",
        flex: "0 0 auto",
        width: 26,
        height: 26,
        borderRadius: 9999,
        background: "rgba(255,255,255,0.16)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: "#fff",
        fontSize: 15,
        lineHeight: 1,
      }}
    >
      ✕
    </div>
  );
}

function StopButton() {
  return (
    <div
      title="Stop and insert"
      {...buttonHandlers(() => void pillCommand("pill_stop"))}
      style={{
        cursor: "pointer",
        flex: "0 0 auto",
        width: 26,
        height: 26,
        borderRadius: 9999,
        background: "#FF5A52",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      }}
    >
      <div style={{ width: 9, height: 9, borderRadius: 2, background: "#fff" }} />
    </div>
  );
}

export function FlowBar() {
  const [state, setState] = useState<BarState>("idle");
  const [bars, setBars] = useState<number[]>([]);
  // Set by `whimpr://error`, shown while state === "error". Falls back to a
  // generic line if the error-state event somehow arrives without one.
  const [errorText, setErrorText] = useState<ErrorEvent | null>(null);
  // Pending click-vs-drag decision; see the pill's onMouseDown.
  const dragTimer = useRef<number | null>(null);
  const [hover, setHover] = useState(false);
  // Which button the pointer is over, so the tooltip can name that action.
  const [tip, setTip] = useState<{ label: string; hint?: string } | null>(null);
  const [language, setLanguage] = useState("en");
  const [keyLabel, setKeyLabel] = useState("fn");
  const [scratch, setScratch] = useState(false);
  const [follows, setFollows] = useState(true);

  // Read the bits of settings the cluster displays. Re-read on hover so a change
  // made in the Hub shows up here without a restart.
  useEffect(() => {
    if (!hover) return;
    void (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const s = await invoke<{
          language: string;
          push_to_talk_key: string;
          pill_follows_active_display: boolean;
        }>("get_settings");
        setLanguage(s.language);
        setKeyLabel(KEY_LABELS[s.push_to_talk_key] ?? "fn");
        setFollows(s.pill_follows_active_display);
        setScratch(await invoke<boolean>("get_scratchpad_capture"));
      } catch {
        /* browser preview */
      }
    })();
  }, [hover]);

  async function cycleLanguage() {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const s = await invoke<Record<string, unknown>>("get_settings");
      const cur = String(s.language ?? "en");
      const i = LANG_CYCLE.indexOf(cur as (typeof LANG_CYCLE)[number]);
      const next = LANG_CYCLE[(i + 1) % LANG_CYCLE.length];
      await invoke("set_settings", { settings: { ...s, language: next } });
      setLanguage(next);
      setTip({ label: "Language", hint: langLabel(next) });
    } catch {
      /* browser preview */
    }
  }

  async function toggleScratch() {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const next = !scratch;
      await invoke("set_scratchpad_capture", { on: next });
      setScratch(next);
      setTip({ label: "Scratchpad", hint: next ? "On" : "Off" });
    } catch {
      /* browser preview */
    }
  }

  useEffect(() => {
    let un1: (() => void) | undefined;
    let un2: (() => void) | undefined;
    let un3: (() => void) | undefined;
    tauriListen<StateEvent>("whimpr://flowbar/state", (p) => setState(p.state)).then((u) => (un1 = u));
    tauriListen<WaveformEvent>("whimpr://audio/waveform", (p) => setBars(p.bars)).then((u) => (un2 = u));
    tauriListen<ErrorEvent>("whimpr://error", (p) => setErrorText(p)).then((u) => (un3 = u));
    return () => {
      un1?.();
      un2?.();
      un3?.();
    };
  }, []);

  const recording = state === "recording" || state === "locked";
  const isIdle = state === "idle";
  const processing = state === "transcribing";
  const isError = state === "error";
  const statusText =
    state === "transcribing"
      ? "Cleaning up…"
      : isError
        ? errorText?.headline ?? "Something's off"
        : state === "cancelled"
          ? "Discarded"
          : "Done";

  // Pill dimensions per state. The idle nub is deliberately tiny so it doesn't
  // nag, but that also made it undiscoverable — so hovering expands it into a
  // labelled affordance that says what a click will do. Error gets extra width
  // so the specific headline isn't clipped back to "Something's off".
  const dims = isIdle
    ? hover
      ? { w: 158, h: 38 }
      : { w: 76, h: 16 }
    : recording
      ? { w: 250, h: 44 }
      : isError
        ? { w: 280, h: 36 }
        : { w: 180, h: 36 };

  const showActions = isIdle && hover;

  // The window is sized to the content, not the other way round. Anything left
  // over would be an invisible rectangle above the Dock that eats clicks meant
  // for the app underneath — which is exactly what it used to be.
  const win = showActions
    ? { w: 320, h: 132 } // capsule + gap + button row
    : isIdle
      ? { w: 100, h: 26 }
      : { w: dims.w + 24, h: dims.h + 14 };

  useEffect(() => {
    void (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("set_overlay_size", { width: win.w, height: win.h });
      } catch {
        /* browser preview */
      }
    })();
  }, [win.w, win.h]);

  return (
    // Bottom-aligned so the resting nub keeps its position and the hover UI grows
    // upwards, rather than the whole cluster jumping when it expands.
    <div
      style={{
        position: "fixed",
        inset: 0,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "flex-end",
        paddingBottom: 4,
        fontFamily: font.ui,
        userSelect: "none",
        // No hover handler here: this fills the whole window, so listening on it
        // meant the cluster opened whenever the pointer was anywhere inside the
        // window rather than actually over the pill.
        pointerEvents: "none",
      }}
    >
      <div
        // The hover target is this cluster — the pill plus, once open, the
        // button row — so it tracks the visible thing rather than the window.
        onMouseEnter={() => setHover(true)}
        onMouseLeave={() => {
          setHover(false);
          setTip(null);
        }}
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          pointerEvents: "auto",
        }}
      >
      <div
        aria-label={`WhimprFlow ${state}${isError && errorText ? `: ${errorText.detail}` : ""}`}
        title={
          isError
            ? errorText?.detail
            : isIdle
              ? "Click to dictate · hold to move"
              : "Hold to move"
        }
        // A quick click and a drag both begin with mousedown, and the native
        // drag takes over the mouse the moment it starts — so the two are told
        // apart by time: release within 180ms is a click, keep holding and it
        // becomes a drag.
        onMouseDown={(e) => {
          if (e.button !== 0) return;
          e.preventDefault();
          if (dragTimer.current !== null) window.clearTimeout(dragTimer.current);
          dragTimer.current = window.setTimeout(() => {
            dragTimer.current = null;
            // Dragging is pointless while the pill is following the cursor —
            // the watcher would pull it straight back. Turn "Follow the active
            // display" off in Settings first.
            if (!follows) void beginDrag();
          }, 180);
        }}
        onMouseUp={() => {
          if (dragTimer.current === null) return; // the drag already started
          window.clearTimeout(dragTimer.current);
          dragTimer.current = null;
          // Clicking the idle nub starts a hands-free dictation; clicking it
          // mid-dictation finishes and inserts.
          void pillCommand(isIdle ? "pill_start" : "pill_stop");
        }}
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: recording ? "space-between" : "center",
          gap: 10,
          height: dims.h,
          width: dims.w,
          padding: recording ? "0 8px" : 0,
          background: pillFill.base,
          border: `1px solid rgba(255,255,255,0.10)`,
          borderRadius: 9999,
          boxShadow: pillFill.shadow,
          color: palette.pillText,
          transition: `width ${geometry.morphMs}ms ${motionEase}, height ${geometry.morphMs}ms ${motionEase}`,
          overflow: "hidden",
          fontSize: 13,
          cursor: "pointer",
        }}
      >
        {isIdle ? (
          // The capsule itself carries the label on hover — it is not replaced.
          showActions ? (
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 9,
                padding: "0 15px",
                whiteSpace: "nowrap",
                fontSize: 14,
                fontWeight: 500,
              }}
            >
              <span>{tip?.label ?? "Dictate"}</span>
              <b style={{ fontWeight: 700 }}>{tip?.hint ?? keyLabel}</b>
            </div>
          ) : null
        ) : recording ? (
          <>
            <CancelButton />
            <div style={{ flex: 1, minWidth: 0 }}>
              <DottedWaveform bars={bars} />
            </div>
            <StopButton />
          </>
        ) : processing ? (
          <span style={{ color: palette.pillTextMuted }}>{statusText}</span>
        ) : (
          <span style={{ color: palette.pillTextMuted }}>{statusText}</span>
        )}
      </div>

      {/* Added below the capsule on hover — the capsule stays put. */}
      {showActions && (
        <div style={{ display: "flex", alignItems: "center", gap: 11, marginTop: 9 }}>
          <RoundButton
            title="Language"
            onEnter={() => setTip({ label: "Language", hint: langLabel(language) })}
            onActivate={() => void cycleLanguage()}
          >
            <GlobeIcon />
          </RoundButton>
          <RoundButton
            primary
            title="Dictate"
            onEnter={() => setTip(null)}
            onActivate={() => void pillCommand("pill_start")}
          >
            <MicIcon />
          </RoundButton>
          <RoundButton
            title="Scratchpad"
            active={scratch}
            onEnter={() => setTip({ label: "Scratchpad", hint: scratch ? "On" : "Off" })}
            onActivate={() => void toggleScratch()}
          >
            <NoteIcon />
          </RoundButton>
        </div>
      )}
      </div>
    </div>
  );
}

const motionEase = "cubic-bezier(0.05,0.6,0.4,0.95)";
