/** @module @coldmind/devermind-shell/terminal — connection launcher and a full-width terminal docked at the bottom, rendered in the overlay's shadow root. */

import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";

import type { ShellError, ShellErrors } from "./error-store.ts";
import { APP_EVENT_PREFIX, type ServerFrame } from "./protocol.ts";
import type { ShellTransport } from "./transport.ts";

/** Props of {@link ShellTerminal}. */
export interface ShellTerminalProps {
  transport: ShellTransport;
  errors: ShellErrors;
  /** App channel the prompt sends on and prints from. Default `terminal` (event `app:terminal`). */
  channel?: string;
}

type LineKind = "status" | "info" | "in" | "out" | "error";
interface Line {
  readonly id: number;
  readonly time: number;
  readonly kind: LineKind;
  readonly text: string;
}
type Phase = "closed" | "open" | "closing";
type Rgb = readonly [number, number, number];

const GAP = 16;
const LAUNCHER = 44;
/** Gap between the launcher and the top edge of the open terminal. */
const RIDE = 12;
const MIN_H = 120;
/** Page height kept visible above the terminal. */
const KEEP_PAGE = 96;
const STEP = 16;
const OPEN_MS = 320;
const CLOSE_MS = 200;
const OPEN_EASE = "cubic-bezier(.2,.8,.2,1)";
const CLOSE_EASE = "cubic-bezier(.4,0,1,1)";
const MAX_LINES = 500;
const HEIGHT_KEY = "devermind.shell.terminal.height";
/** CSS variable on `<html>` holding the open terminal's height, for layouts sized to the viewport. */
const HEIGHT_VAR = "--devermind-terminal-height";
const GREEN: Rgb = [31, 157, 85];
const RED: Rgb = [229, 72, 77];

let nextLine = 1;
const makeLine = (kind: LineKind, text: string): Line => ({ id: nextLine++, time: Date.now(), kind, text });

/**
 * Round launcher in the bottom-right corner with a pulsing ripple: green while the dev server
 * socket is open, red otherwise. It toggles a full-width terminal docked at the bottom that slides
 * up and pushes the page: `body` gets bottom padding equal to the terminal height, and `<html>`
 * gets `--devermind-terminal-height` for layouts sized to the viewport. The launcher rides on the
 * terminal's top edge. Drag the top edge to resize (ArrowUp/ArrowDown when it has focus).
 *
 * The terminal prints socket status changes, HMR frames and shell errors, and the prompt sends
 * each line on the app channel (`app:terminal` by default); messages on that channel are printed.
 * `clear` empties the log; ArrowUp/ArrowDown walk the input history; Escape closes the panel.
 */
export function ShellTerminal({ transport, errors, channel: channelName = "terminal" }: ShellTerminalProps) {
  const subscribeStatus = useCallback((listener: () => void) => transport.onStatus(listener), [transport]);
  const status = useSyncExternalStore(subscribeStatus, () => transport.status);
  const connected = status === "open";
  const rgb = connected ? GREEN : RED;
  const color = `rgb(${rgb.join(",")})`;

  const [phase, setPhase] = useState<Phase>("closed");
  const [lines, setLines] = useState<readonly Line[]>(() => [makeLine("status", `socket ${transport.status}`)]);
  const [height, setHeight] = useState(loadHeight);
  const [resizing, setResizing] = useState(false);
  const [hover, setHover] = useState(false);
  const [focusRing, setFocusRing] = useState<"launcher" | "edge" | null>(null);

  const panelId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const logRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const launcherRef = useRef<HTMLButtonElement>(null);
  const waveRef = useRef<HTMLSpanElement>(null);
  const glowRef = useRef<HTMLSpanElement>(null);
  const anim = useRef<Animation | null>(null);
  const pageAnim = useRef<Animation | null>(null);
  const launcherAnim = useRef<Animation | null>(null);
  const launcherWrapRef = useRef<HTMLDivElement>(null);
  /** The page's own bottom padding while it is pushed, restored on close. */
  const pushed = useRef<{ inline: string; base: string } | null>(null);
  const stick = useRef(true);
  const heightRef = useRef(height);
  heightRef.current = height;
  const drag = useRef<{ y: number; h: number } | null>(null);
  const history = useRef<string[]>([]);
  const historyAt = useRef(0);

  const channel = useMemo(() => transport.channel<unknown>(channelName), [transport, channelName]);

  const push = useCallback((kind: LineKind, text: string) => {
    setLines((prev) => {
      const next = [...prev, makeLine(kind, text)];
      return next.length > MAX_LINES ? next.slice(-MAX_LINES) : next;
    });
  }, []);

  // Feed the log: status, frames (minus this terminal's channel and error frames, which arrive via `errors`), channel output, errors.
  useEffect(() => {
    const appEvent = APP_EVENT_PREFIX + channelName;
    let seen = 0;
    const logErrors = () => {
      for (const error of errors.list()) {
        if (error.id <= seen) continue;
        seen = error.id;
        push("error", describeError(error));
      }
    };
    logErrors();
    const offStatus = transport.onStatus((next) => push("status", `socket ${next}`));
    const offFrame = transport.onFrame((frame) => {
      if (frame.type === "custom" && frame.event === appEvent) return;
      const text = describeFrame(frame);
      if (text) push("info", text);
    });
    const offChannel = channel.subscribe((data) => push("out", typeof data === "string" ? data : safeJson(data)));
    const offErrors = errors.subscribe(logErrors);
    return () => {
      offStatus();
      offFrame();
      offChannel();
      offErrors();
    };
  }, [transport, errors, channel, channelName, push]);

  // Ripple: a scaling wave plus a box-shadow pulse, restarted when the colour changes.
  useEffect(() => {
    if (reducedMotion()) return;
    const [r, g, b] = rgb;
    const wave = play(waveRef.current, [{ opacity: 1, transform: "scale(1)" }, { opacity: 0, transform: "scale(2.25)" }], {
      duration: 2500,
      easing: "ease-in-out",
      iterations: Infinity,
    });
    const glow = play(
      glowRef.current,
      [{ boxShadow: `0 0 0 0 rgba(${r},${g},${b},.25)` }, { boxShadow: `0 0 3px 30px rgba(${r},${g},${b},0)` }],
      { duration: 2000, easing: "ease-in-out", iterations: Infinity },
    );
    return () => {
      wave?.cancel();
      glow?.cancel();
    };
  }, [rgb]);

  // Open: slide up and push the page. Close: slide down, let the page back, then unmount.
  useLayoutEffect(() => {
    const el = panelRef.current;
    if (phase === "closed" || !el) return;
    anim.current?.cancel();
    pageAnim.current?.cancel();
    launcherAnim.current?.cancel();
    const h = heightRef.current;
    const motion = !reducedMotion();
    const slide: Keyframe[] = [{ transform: "translateY(100%)" }, { transform: "none" }];
    // The launcher animates only with open/close; resizes move it directly.
    const ride: Keyframe[] = [{ bottom: launcherBottom(false, h) }, { bottom: launcherBottom(true, h) }];
    if (phase === "open") {
      const { from, to } = pushPage(pushed, h);
      anim.current = motion ? play(el, slide, { duration: OPEN_MS, easing: OPEN_EASE }) : null;
      pageAnim.current = motion ? play(document.body, [{ paddingBottom: from }, { paddingBottom: to }], { duration: OPEN_MS, easing: OPEN_EASE }) : null;
      launcherAnim.current = motion ? play(launcherWrapRef.current, ride, { duration: OPEN_MS, easing: OPEN_EASE }) : null;
      inputRef.current?.focus({ preventScroll: true });
      if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
      return;
    }
    const pad = pushed.current ? pushedPadding(pushed.current.base, h) : null;
    const closing = motion ? play(el, [...slide].reverse(), { duration: CLOSE_MS, easing: CLOSE_EASE, fill: "forwards" }) : null;
    const page =
      motion && pad && pushed.current
        ? play(document.body, [{ paddingBottom: pad }, { paddingBottom: pushed.current.base }], { duration: CLOSE_MS, easing: CLOSE_EASE, fill: "forwards" })
        : null;
    anim.current = closing;
    pageAnim.current = page;
    launcherAnim.current = motion ? play(launcherWrapRef.current, [...ride].reverse(), { duration: CLOSE_MS, easing: CLOSE_EASE }) : null;
    const done = () => {
      page?.cancel();
      releasePage(pushed);
      setPhase((p) => (p === "closing" ? "closed" : p));
    };
    if (!closing) return done();
    closing.finished.then(done, () => {});
  }, [phase]);

  // Follow resizes while open; give the page back if the shell unmounts.
  useEffect(() => {
    if (phase === "open") pushPage(pushed, height);
  }, [height, phase]);
  useEffect(
    () => () => {
      pageAnim.current?.cancel();
      releasePage(pushed);
    },
    [],
  );

  // Keep the log pinned to the bottom unless the user scrolled up.
  useLayoutEffect(() => {
    const el = logRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [lines]);

  // Keep the terminal inside the viewport; remember its height.
  useEffect(() => {
    const onResize = () => setHeight((h) => clampHeight(h));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  useEffect(() => {
    try {
      localStorage.setItem(HEIGHT_KEY, String(height));
    } catch {}
  }, [height]);

  const toggle = () => setPhase((p) => (p === "open" ? "closing" : "open"));
  const close = () => {
    setPhase("closing");
    launcherRef.current?.focus({ preventScroll: true });
  };

  /** Capture the drag so resizing continues even when the pointer leaves the narrow edge. */
  const startResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = { y: event.clientY, h: height };
    setResizing(true);
  };
  const moveResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (d) setHeight(clampHeight(d.h + d.y - event.clientY));
  };
  const endResize = () => {
    if (!drag.current) return;
    drag.current = null;
    setResizing(false);
  };

  const onEdgeKey = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault();
    setHeight((h) => clampHeight(h + (event.key === "ArrowUp" ? STEP : -STEP)));
  };

  /**
   * Keeps history and local commands in the browser; other input is plain app-channel data.
   * This console never executes OS commands. A host may interpret the terminal channel separately.
   */
  const onInputKey = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    const input = event.currentTarget;
    if (event.key === "Escape") {
      event.preventDefault();
      close();
    } else if (event.key === "Enter") {
      const text = input.value.trim();
      input.value = "";
      if (!text) return;
      history.current = [...history.current.filter((h) => h !== text), text].slice(-100);
      historyAt.current = history.current.length;
      stick.current = true;
      if (text === "clear") return setLines([]);
      push("in", text);
      if (!channel.send(text)) push("error", "not sent: the socket is closed or its queue is full");
      else if (transport.status !== "open") push("info", "queued until the dev server is back");
    } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
      const list = history.current;
      if (!list.length) return;
      event.preventDefault();
      historyAt.current = Math.max(0, Math.min(list.length, historyAt.current + (event.key === "ArrowUp" ? -1 : 1)));
      input.value = list[historyAt.current] ?? "";
    }
  };

  const open = phase === "open";
  const ripple = `repeating-radial-gradient(rgba(${rgb},0) 0%, rgba(${rgb},.15) 50%, rgba(${rgb},.5) 75%, rgba(${rgb},.05) 100%)`;
  const ring = (which: "launcher" | "edge") => (event: { currentTarget: Element }) => setFocusRing(focusVisible(event.currentTarget) ? which : null);

  return (
    <>
      {phase !== "closed" && (
        <div
          ref={panelRef}
          id={panelId}
          role="dialog"
          aria-label="devermind terminal"
          style={{ ...styles.panel, height, userSelect: resizing ? "none" : undefined }}
        >
          <div
            role="separator"
            aria-label="Resize terminal"
            aria-orientation="horizontal"
            aria-valuenow={height}
            aria-valuemin={MIN_H}
            tabIndex={0}
            style={{ ...styles.edge, outline: focusRing === "edge" ? "2px solid #8ab4ff" : "none" }}
            onPointerDown={startResize}
            onPointerMove={moveResize}
            onPointerUp={endResize}
            onPointerCancel={endResize}
            onLostPointerCapture={endResize}
            onKeyDown={onEdgeKey}
            onFocus={ring("edge")}
            onBlur={() => setFocusRing(null)}
          >
            <span aria-hidden="true" style={{ ...styles.grip, background: resizing || focusRing === "edge" ? "#8ab4ff" : "#3a3a46" }} />
          </div>
          <div style={styles.bar}>
            <span style={{ ...styles.dot, background: color, boxShadow: `0 0 0 3px rgba(${rgb},.22)` }} />
            <span style={styles.brand}>devermind</span>
            <span style={styles.state}>{resizing ? `${height}px` : status}</span>
            <button type="button" style={styles.button} onClick={() => setLines([])}>
              Clear
            </button>
            <button type="button" style={styles.button} aria-label="Close terminal" onClick={close}>
              ✕
            </button>
          </div>
          <div
            ref={logRef}
            role="log"
            aria-live="polite"
            style={styles.log}
            onScroll={(event) => {
              const el = event.currentTarget;
              stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 8;
            }}
            onMouseUp={() => {
              const selection = window.getSelection();
              if (!selection || selection.isCollapsed) inputRef.current?.focus({ preventScroll: true });
            }}
          >
            {lines.map((line) => (
              <div key={line.id} style={{ ...styles.line, color: LINE_COLORS[line.kind] }}>
                <span style={styles.time}>{clock(line.time)}</span>
                {line.kind === "in" && <span style={styles.caret}>› </span>}
                {line.text}
              </div>
            ))}
          </div>
          <label style={styles.prompt}>
            <span aria-hidden="true" style={styles.caret}>
              ›
            </span>
            <input
              ref={inputRef}
              aria-label={`Send on app:${channelName}`}
              placeholder={connected ? `send on app:${channelName}` : "offline: input is queued until the server is back"}
              spellCheck={false}
              autoComplete="off"
              style={styles.input}
              onKeyDown={onInputKey}
            />
          </label>
        </div>
      )}
      <div ref={launcherWrapRef} style={{ ...styles.launcher, bottom: launcherBottom(open, height) }}>
        <span ref={waveRef} aria-hidden="true" style={{ ...styles.ring, background: ripple }} />
        <span ref={glowRef} aria-hidden="true" style={styles.ring} />
        <button
          ref={launcherRef}
          type="button"
          aria-label={open ? "Close terminal" : "Open terminal"}
          aria-expanded={open}
          aria-controls={phase !== "closed" ? panelId : undefined}
          title={`dev server: ${status}`}
          style={{
            ...styles.launcherButton,
            background: color,
            transform: `rotate(${open ? 90 : 0}deg) scale(${hover ? 1.08 : 1})`,
            outline: focusRing === "launcher" ? "2px solid #e6e6ea" : "none",
          }}
          onClick={toggle}
          onPointerEnter={() => setHover(true)}
          onPointerLeave={() => setHover(false)}
          onFocus={ring("launcher")}
          onBlur={() => setFocusRing(null)}
        >
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            {open ? <path d="M6 6l12 12M18 6L6 18" /> : <path d="M5 7l5 5-5 5M12.5 17H19" />}
          </svg>
        </button>
      </div>
    </>
  );
}

function describeFrame(frame: ServerFrame): string | null {
  switch (frame.type) {
    case "connected":
      return "connected";
    case "update":
      return `hot update ${frame.updates.map((u) => u.path).join(", ")}`;
    case "full-reload":
      return `full reload${frame.path ? ` ${frame.path}` : ""}${frame.reason ? ` (${frame.reason})` : ""}`;
    case "prune":
      return `prune ${frame.paths.join(", ")}`;
    case "custom":
      return `event ${frame.event}${frame.data === undefined ? "" : ` ${safeJson(frame.data)}`}`;
    case "error":
      return null;
  }
}

function describeError(error: ShellError): string {
  return `${error.kind}: ${error.message}${error.location ? ` (${error.location})` : ""}`;
}

function safeJson(value: unknown): string {
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function clock(time: number): string {
  const d = new Date(time);
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, "0")).join(":");
}

function play(el: Element | null, frames: Keyframe[], options: KeyframeAnimationOptions): Animation | null {
  return el && typeof el.animate === "function" ? el.animate(frames, options) : null;
}

function reducedMotion(): boolean {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

function focusVisible(el: Element): boolean {
  try {
    return el.matches(":focus-visible");
  } catch {
    return true;
  }
}

function clampHeight(h: number): number {
  const max = Math.max(MIN_H, window.innerHeight - KEEP_PAGE);
  return Math.round(Math.min(max, Math.max(MIN_H, h)));
}

function loadHeight(): number {
  try {
    const saved = Number(localStorage.getItem(HEIGHT_KEY));
    if (saved > 0) return clampHeight(saved);
  } catch {}
  return clampHeight(Math.round(window.innerHeight * 0.36));
}

const launcherBottom = (open: boolean, h: number) => (open ? `${h + RIDE}px` : `calc(${GAP}px + env(safe-area-inset-bottom, 0px))`);

const pushedPadding = (base: string, h: number) => `calc(${base} + ${h}px)`;

/**
 * Snapshots the app's existing inline/computed padding once, then reserves terminal space.
 * Repeated resizes reuse that base instead of accumulating padding; releasePage restores it.
 * The CSS variable lets viewport-sized app layouts opt into the same reserved height.
 */
function pushPage(pushed: { current: { inline: string; base: string } | null }, h: number): { from: string; to: string } {
  const { body, documentElement } = document;
  if (!pushed.current) pushed.current = { inline: body.style.paddingBottom, base: getComputedStyle(body).paddingBottom || "0px" };
  const to = pushedPadding(pushed.current.base, h);
  body.style.paddingBottom = to;
  documentElement.style.setProperty(HEIGHT_VAR, `${h}px`);
  return { from: pushed.current.base, to };
}

/** Restores the page's own bottom padding. */
function releasePage(pushed: { current: { inline: string; base: string } | null }): void {
  if (!pushed.current) return;
  document.body.style.paddingBottom = pushed.current.inline;
  document.documentElement.style.removeProperty(HEIGHT_VAR);
  pushed.current = null;
}

const mono = "13px/1.6 ui-monospace,SFMono-Regular,Menlo,monospace";

const LINE_COLORS: Record<LineKind, string> = {
  status: "#8ab4ff",
  info: "#8d8d9c",
  in: "#e6e6ea",
  out: "#c9c9d4",
  error: "#ff8a8a",
};

const styles = {
  launcher: { position: "fixed", right: `calc(${GAP}px + env(safe-area-inset-right, 0px))`, width: LAUNCHER, height: LAUNCHER, zIndex: 2147483645 },
  ring: { position: "absolute", inset: 0, borderRadius: "50%", pointerEvents: "none" },
  launcherButton: {
    position: "relative",
    width: "100%",
    height: "100%",
    padding: 0,
    border: "none",
    borderRadius: "50%",
    color: "#fff",
    display: "grid",
    placeItems: "center",
    cursor: "pointer",
    outlineOffset: 3,
    boxShadow: "0 6px 18px rgba(0,0,0,.35)",
    transition: "transform 300ms ease, background-color 300ms ease",
  },
  panel: {
    position: "fixed",
    left: 0,
    right: 0,
    bottom: 0,
    zIndex: 2147483645,
    boxSizing: "border-box",
    display: "flex",
    flexDirection: "column",
    background: "#16161c",
    borderTop: "1px solid #33333f",
    boxShadow: "0 -12px 40px rgba(0,0,0,.35)",
    color: "#e6e6ea",
    font: mono,
    overflow: "hidden",
  },
  edge: {
    position: "absolute",
    top: 0,
    left: 0,
    right: 0,
    height: 7,
    cursor: "ns-resize",
    zIndex: 2,
    touchAction: "none",
    display: "grid",
    placeItems: "center",
    outlineOffset: -2,
  },
  grip: { width: 36, height: 3, borderRadius: 2 },
  bar: {
    display: "flex",
    alignItems: "center",
    gap: 10,
    padding: "10px max(12px, env(safe-area-inset-right, 0px)) 8px max(12px, env(safe-area-inset-left, 0px))",
    background: "#1d1d25",
    borderBottom: "1px solid #2a2a34",
    flex: "none",
  },
  dot: { width: 8, height: 8, borderRadius: "50%", flex: "none", transition: "background-color 300ms ease" },
  brand: { fontWeight: 700, letterSpacing: ".08em", color: "#c9c9d4", fontSize: 12 },
  state: { flex: 1, color: "#8d8d9c", fontSize: 12, fontVariantNumeric: "tabular-nums" },
  button: {
    font: mono,
    fontSize: 12,
    color: "#c9c9d4",
    background: "transparent",
    border: "1px solid #3a3a46",
    borderRadius: 6,
    padding: "2px 8px",
    cursor: "pointer",
  },
  log: { flex: 1, overflow: "auto", padding: "8px max(12px, env(safe-area-inset-right, 0px)) 8px max(12px, env(safe-area-inset-left, 0px))", background: "#0d0d12", fontSize: 12 },
  line: { whiteSpace: "pre-wrap", wordBreak: "break-word" },
  time: { color: "#5a5a6a", marginRight: 10, fontVariantNumeric: "tabular-nums" },
  caret: { color: "#8ab4ff" },
  prompt: {
    display: "flex",
    alignItems: "center",
    gap: 8,
    padding: "8px max(12px, env(safe-area-inset-right, 0px)) calc(8px + env(safe-area-inset-bottom, 0px)) max(12px, env(safe-area-inset-left, 0px))",
    background: "#111117",
    borderTop: "1px solid #26262f",
    flex: "none",
    cursor: "text",
  },
  input: {
    flex: 1,
    minWidth: 0,
    font: mono,
    fontSize: 12,
    color: "#e6e6ea",
    background: "transparent",
    border: "none",
    outline: "none",
    padding: 0,
  },
} satisfies Record<string, CSSProperties>;
