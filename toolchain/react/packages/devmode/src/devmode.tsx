/**
 * `<DevMode>`: the launcher button, hotkey and the bottom-docked panel that hosts the Console and the Redis-protocol API tabs.
 * The panel spans the full window width and is resized from its top edge with react-resizable.
 * Rendered through a portal on `document.body`, with its own scoped styles, so app layout and CSS do not affect it.
 */
import { useEffect, useMemo, useState, useSyncExternalStore, type Ref } from "react";
import { createPortal } from "react-dom";
import { Resizable } from "react-resizable";
import { capture } from "./capture.ts";
import { ConsoleTab } from "./console-tab.tsx";
import { RedisTab } from "./redis-tab.tsx";
import { defaultEndpoint } from "./transport.ts";
import { STYLES } from "./styles.ts";
import { TabBar, usePersisted } from "./ui.tsx";

/** Props of `<DevMode>`. */
export interface DevModeProps {
  /** Renders nothing and captures nothing when false. Pass e.g. `import.meta.env.DEV`. Default true. */
  enabled?: boolean;
  /** Toggle key, as `mod+…+key` with mods `ctrl`, `meta`, `alt`, `shift`. Default `` ctrl+` ``. */
  hotkey?: string;
  /**
   * Endpoint of the app's Redis-protocol API, used until the user edits it: `ipc://resp` for the desktop host's WebView IPC,
   * or a `ws://`/`wss://` URL. Default: `ipc://resp` when `window.ipc` exists, else `/resp` on the page's own host.
   */
  redisUrl?: string;
  /** Show the floating launcher button. Default true; with false, only the hotkey opens the panel. */
  launcher?: boolean;
  /**
   * The host's API as `HostApi::spec` describes it. When given, the API tree lists its areas and does not ask the
   * server with `COMMAND DOCS`, so the API is described in one place. The bundler passes it to `DevApp`.
   */
  api?: unknown;
}

/** Smallest panel height in px. */
const MIN_HEIGHT = 160;
/** Gap in px kept above the panel at its largest, so the page stays reachable. */
const TOP_GAP = 40;
/** Height in px of the title bar, which is all a minimized panel shows (matches `.lsdm-panel > .lsdm-tabbar`). */
const TITLE_HEIGHT = 50;

/**
 * Tests a key event against a hotkey string.
 * @param e the event
 * @param hotkey e.g. `ctrl+shift+d`
 * @return true when every named modifier is down, no other is, and the key matches
 */
export function matchesHotkey(e: KeyboardEvent, hotkey: string): boolean {
  const parts = hotkey.toLowerCase().split("+");
  const key = parts.pop() ?? "";
  const mods = new Set(parts);
  return e.key.toLowerCase() === key && e.ctrlKey === mods.has("ctrl") && e.metaKey === mods.has("meta") && e.altKey === mods.has("alt") && e.shiftKey === mods.has("shift");
}

/**
 * The dev mode overlay. Mount once, anywhere in the tree: `<DevMode enabled={import.meta.env.DEV} />`.
 */
export function DevMode({ enabled = true, hotkey = "ctrl+`", redisUrl, launcher = true, api }: DevModeProps) {
  if (!enabled) return null;
  return <DevModePanel hotkey={hotkey} redisUrl={redisUrl ?? defaultEndpoint()} launcher={launcher} api={api} />;
}

/** The enabled dev mode; split from `DevMode` so hooks never run when disabled. */
function DevModePanel({ hotkey, redisUrl, launcher, api }: Required<Pick<DevModeProps, "hotkey" | "launcher" | "redisUrl">> & Pick<DevModeProps, "api">) {
  const [open, setOpen] = usePersisted("open", false);
  // Decision: default to 60% of the window; a fixed 360 px squeezed the API response pane on typical screens.
  const [height, setHeight] = usePersisted("height", () => Math.max(MIN_HEIGHT, Math.round(window.innerHeight * 0.6)));
  const [tab, setTab] = usePersisted<"console" | "api">("tab", "console");
  // Decision: minimize collapses the docked panel to its title bar instead of hiding it; the hotkey still hides it entirely.
  const [minimized, setMinimized] = usePersisted("minimized", false);
  const [host] = useState(() => document.createElement("div"));
  const entries = useSyncExternalStore(capture.subscribe, capture.snapshot);
  const errors = useMemo(() => entries.filter((e) => e.type === "error" || (e.type === "net" && (e.net!.status === 0 || e.net!.status >= 500))).length, [entries]);

  // Decision: never uninstalled here. The app may have called installCapture() first, and logs should keep
  // accumulating while the panel is closed or remounted (StrictMode).
  useEffect(() => capture.install(), []);

  useEffect(() => {
    host.className = "lsdm-host";
    document.body.appendChild(host);
    return () => host.remove();
  }, [host]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (matchesHotkey(e, hotkey)) {
        e.preventDefault();
        setOpen((o) => !o);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [hotkey, setOpen]);

  // Decision: clamp at render, not in state, so a smaller window shrinks the panel without losing the stored height.
  const shown = minimized ? TITLE_HEIGHT : Math.max(MIN_HEIGHT, Math.min(height, window.innerHeight - TOP_GAP));

  // Docked, not floating: reserve the panel's height at the bottom of the page so no app content sits under it.
  useEffect(() => {
    if (!open) return;
    document.body.style.paddingBottom = `${shown}px`;
    return () => {
      document.body.style.paddingBottom = "";
    };
  }, [open, shown]);

  return createPortal(
    <div className="lsdm">
      <style>{STYLES}</style>
      {launcher && !open && (
        <button className="lsdm-launcher" title={`Dev mode (${hotkey})`} onClick={() => setOpen(true)}>
          Dev mode{errors > 0 && <span className="lsdm-badge">{errors}</span>}
        </button>
      )}
      {open && (
        <Resizable
          width={0}
          height={shown}
          axis="y"
          resizeHandles={minimized ? [] : ["n"]}
          minConstraints={[0, MIN_HEIGHT]}
          maxConstraints={[Infinity, window.innerHeight - TOP_GAP]}
          handle={(_axis, ref) => <div ref={ref as Ref<HTMLDivElement>} className="lsdm-resize" title="Drag to resize" />}
          onResize={(_e, { size }) => setHeight(size.height)}
        >
        <div className={`lsdm-panel${minimized ? " lsdm-minimized" : ""}`} style={{ height: shown }} role="dialog" aria-label="Dev mode">
          <TabBar
            tabs={[
              ["console", <>Console{errors > 0 && <span className="lsdm-count lsdm-c-error">{errors}</span>}</>],
              ["api", "API"],
            ]}
            value={tab}
            onChange={(t) => {
              setTab(t);
              setMinimized(false);
            }}
            right={
              <button className="lsdm-min" title={minimized ? "Restore" : "Minimize"} aria-label={minimized ? "Restore" : "Minimize"} onClick={() => setMinimized((m) => !m)}>
                <span className={minimized ? "lsdm-glyph-restore" : "lsdm-glyph-min"} />
              </button>
            }
          />
          {/* Both tabs stay mounted so a response, an in-flight request and scroll position survive switching. */}
          <div className="lsdm-content" hidden={tab !== "console"}><ConsoleTab /></div>
          <div className="lsdm-content" hidden={tab !== "api"}><RedisTab defaultUrl={redisUrl} active={tab === "api" && !minimized} api={api} /></div>
        </div>
        </Resizable>
      )}
    </div>,
    host,
  );
}
