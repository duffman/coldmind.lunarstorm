/**
 * `<DevMode>`: the launcher button, hotkey and the resizable bottom panel that hosts the Console and API tabs.
 * Rendered through a portal on `document.body`, with its own scoped styles, so app layout and CSS do not affect it.
 */
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import { ApiTab, type ApiLoad } from "./api-tab.tsx";
import { capture, type NetRecord } from "./capture.ts";
import { ConsoleTab } from "./console-tab.tsx";
import { newRequest, splitQuery, type Pair } from "./request.ts";
import { STYLES } from "./styles.ts";
import { TabBar, usePersisted } from "./ui.tsx";

/** Props of `<DevMode>`. */
export interface DevModeProps {
  /** Renders nothing and captures nothing when false. Pass e.g. `import.meta.env.DEV`. Default true. */
  enabled?: boolean;
  /** Toggle key, as `mod+…+key` with mods `ctrl`, `meta`, `alt`, `shift`. Default `` ctrl+` ``. */
  hotkey?: string;
  /** Initial API environment variables (e.g. `{ baseUrl: "http://localhost:3000" }`), used until the user edits the Env list. */
  env?: Record<string, string>;
  /** Show the floating launcher button. Default true; with false, only the hotkey opens the panel. */
  launcher?: boolean;
}

/** Panel height bounds in px. */
const MIN_HEIGHT = 160;

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
 * Turns a captured request into an editable one: query split into params, headers kept, body typed by content.
 * @param n the captured request
 * @return a new unsaved request
 */
export function requestFromNet(n: NetRecord): ReturnType<typeof newRequest> {
  const split = splitQuery(n.url);
  const method = (["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"].includes(n.method) ? n.method : "GET") as ReturnType<typeof newRequest>["method"];
  let bodyMode: "none" | "json" | "text" = "none";
  let body = n.requestBody ?? "";
  if (n.requestBody !== undefined) {
    try {
      body = JSON.stringify(JSON.parse(n.requestBody), null, 2);
      bodyMode = "json";
    } catch {
      bodyMode = "text";
    }
  }
  return newRequest({
    name: `${n.method} ${split.url.replace(/^https?:\/\/[^/]+/, "")}`,
    method, url: split.url, params: split.params, bodyMode, body,
    // Content-Type is re-added from the body mode; keeping it too would send it twice with different casing.
    headers: n.requestHeaders.filter(([k]) => !(bodyMode === "json" && k.toLowerCase() === "content-type")).map(([key, value]) => ({ key, value, enabled: true })),
  });
}

/**
 * The dev mode overlay. Mount once, anywhere in the tree: `<DevMode enabled={import.meta.env.DEV} />`.
 */
export function DevMode({ enabled = true, hotkey = "ctrl+`", env, launcher = true }: DevModeProps) {
  if (!enabled) return null;
  return <DevModePanel hotkey={hotkey} env={env} launcher={launcher} />;
}

/** The enabled dev mode; split from `DevMode` so hooks never run when disabled. */
function DevModePanel({ hotkey, env, launcher }: Required<Pick<DevModeProps, "hotkey" | "launcher">> & Pick<DevModeProps, "env">) {
  const [open, setOpen] = usePersisted("open", false);
  const [height, setHeight] = usePersisted("height", 360);
  const [tab, setTab] = usePersisted<"console" | "api">("tab", "console");
  const [load, setLoad] = useState<ApiLoad>();
  const [host] = useState(() => document.createElement("div"));
  const drag = useRef(false);
  const entries = useSyncExternalStore(capture.subscribe, capture.snapshot);
  const errors = useMemo(() => entries.filter((e) => e.type === "error" || (e.type === "net" && (e.net!.status === 0 || e.net!.status >= 500))).length, [entries]);
  const defaultEnv = useMemo<Pair[]>(() => Object.entries(env ?? {}).map(([key, value]) => ({ key, value, enabled: true })), [env]);

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

  useEffect(() => {
    const move = (e: PointerEvent) => {
      if (drag.current) setHeight(Math.max(MIN_HEIGHT, Math.min(window.innerHeight - 40, window.innerHeight - e.clientY)));
    };
    const up = () => {
      drag.current = false;
      document.body.style.userSelect = "";
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
  }, [setHeight]);

  const onLoaded = useCallback(() => setLoad(undefined), []);
  const openInApi = (n: NetRecord) => {
    setLoad((l) => ({ seq: (l?.seq ?? 0) + 1, req: requestFromNet(n) }));
    setTab("api");
  };

  return createPortal(
    <div className="lsdm">
      <style>{STYLES}</style>
      {launcher && !open && (
        <button className="lsdm-launcher" title={`Dev mode (${hotkey})`} onClick={() => setOpen(true)}>
          DEV{errors > 0 && <span className="lsdm-badge">{errors}</span>}
        </button>
      )}
      {open && (
        <div className="lsdm-panel" style={{ height: Math.min(height, window.innerHeight - 40) }} role="dialog" aria-label="Dev mode">
          <div
            className="lsdm-resize"
            onPointerDown={(e) => {
              e.preventDefault();
              drag.current = true;
              document.body.style.userSelect = "none";
            }}
          />
          <TabBar
            tabs={[
              ["console", <>Console{errors > 0 && <span className="lsdm-count lsdm-c-error">{errors}</span>}</>],
              ["api", "API"],
            ]}
            value={tab}
            onChange={setTab}
            right={<button className="lsdm-x lsdm-close" title={`Close (${hotkey})`} onClick={() => setOpen(false)}>×</button>}
          />
          {/* Both tabs stay mounted so a response, an in-flight request and scroll position survive switching. */}
          <div className="lsdm-content" hidden={tab !== "console"}><ConsoleTab onOpenInApi={openInApi} /></div>
          <div className="lsdm-content" hidden={tab !== "api"}><ApiTab load={load} onLoaded={onLoaded} defaultEnv={defaultEnv} /></div>
        </div>
      )}
    </div>,
    host,
  );
}
