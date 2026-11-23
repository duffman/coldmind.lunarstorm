/** @module @coldmind/devermind-shell/overlay — error panel and connection badge, rendered in a shadow root. */

import { useEffect, useState, useSyncExternalStore, type CSSProperties } from "react";

import { BLOCKING_KINDS, type ShellError, type ShellErrors } from "./error-store.ts";
import type { ConnectionStatus } from "./transport.ts";

/** Props of {@link ShellOverlay}. */
export interface ShellOverlayProps {
  errors: ShellErrors;
  /** Subscribes to connection status changes; returns the unsubscribe function. */
  subscribeStatus: (listener: () => void) => () => void;
  getStatus: () => ConnectionStatus;
}

const LABELS: Record<ShellError["kind"], string> = {
  server: "Build error",
  load: "Failed to load the app",
  render: "Render error",
  runtime: "Uncaught error",
  hmr: "Hot update failed",
};

const STATUS_TEXT: Partial<Record<ConnectionStatus, string>> = {
  connecting: "connecting to dev server…",
  lost: "dev server lost, waiting for restart…",
};

/**
 * The shell's own UI: a panel listing errors (blocking errors open it, runtime errors show a
 * badge until opened) and a badge while the dev server is unreachable. Escape or the backdrop
 * hides the panel until the next error arrives.
 */
export function ShellOverlay({ errors, subscribeStatus, getStatus }: ShellOverlayProps) {
  const list = useSyncExternalStore(errors.subscribe, errors.list);
  const status = useSyncExternalStore(subscribeStatus, getStatus);
  const [hiddenAt, setHiddenAt] = useState(0);
  const newest = list.at(-1);
  const blocking = list.some((e) => BLOCKING_KINDS.has(e.kind));
  const [open, setOpen] = useState(false);
  const shown = (open || blocking) && !!newest && newest.id > hiddenAt;

  useEffect(() => {
    if (!shown) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") hide();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  function hide() {
    setOpen(false);
    setHiddenAt(newest?.id ?? 0);
  }

  return (
    <>
      {shown && (
        <div role="dialog" aria-label="Dev errors" style={styles.backdrop} onClick={(e) => e.target === e.currentTarget && hide()}>
          <div style={styles.card}>
            <div style={styles.bar}>
              <span style={styles.brand}>devermind</span>
              <span style={styles.count}>{list.length === 1 ? "1 error" : `${list.length} errors`}</span>
              <button type="button" style={styles.button} onClick={() => errors.clear()}>
                Clear
              </button>
              <button type="button" style={styles.button} aria-label="Close" onClick={hide}>
                ✕
              </button>
            </div>
            {[...list].reverse().map((error) => (
              <ErrorEntry key={error.id} error={error} onDismiss={() => errors.dismiss(error.id)} />
            ))}
            <div style={styles.hint}>Fix the file and save to retry · Esc or click outside to hide</div>
          </div>
        </div>
      )}
      {!shown && list.length > 0 && (
        <button type="button" style={{ ...styles.badge, ...styles.errorBadge }} onClick={() => (setOpen(true), setHiddenAt(0))}>
          {list.length === 1 ? "1 error" : `${list.length} errors`}
        </button>
      )}
      {STATUS_TEXT[status] && (
        <div role="status" style={{ ...styles.badge, ...styles.statusBadge }}>
          {STATUS_TEXT[status]}
        </div>
      )}
    </>
  );
}

function ErrorEntry({ error, onDismiss }: { error: ShellError; onDismiss: () => void }) {
  const detail = error.frame?.trim() || error.stack?.trim();
  return (
    <section style={styles.entry}>
      <div style={styles.entryHead}>
        <span style={styles.kind}>{LABELS[error.kind]}</span>
        <button type="button" style={styles.button} aria-label="Dismiss" onClick={onDismiss}>
          Dismiss
        </button>
      </div>
      <div style={styles.message}>{error.message}</div>
      {error.location && <div style={styles.location}>{error.location}</div>}
      {detail && <pre style={styles.pre}>{detail}</pre>}
      {error.componentStack && <pre style={{ ...styles.pre, ...styles.dim }}>{error.componentStack.trim()}</pre>}
    </section>
  );
}

const mono = "13px/1.6 ui-monospace,SFMono-Regular,Menlo,monospace";

const styles = {
  backdrop: {
    position: "fixed",
    inset: 0,
    zIndex: 2147483646,
    background: "rgba(10,10,14,.86)",
    display: "flex",
    alignItems: "flex-start",
    justifyContent: "center",
    padding: "6vh 4vw",
    overflow: "auto",
    font: mono,
  },
  card: {
    maxWidth: 920,
    width: "100%",
    background: "#16161c",
    border: "1px solid #33333f",
    borderRadius: 12,
    boxShadow: "0 20px 60px rgba(0,0,0,.5)",
    color: "#e6e6ea",
    overflow: "hidden",
  },
  bar: {
    display: "flex",
    alignItems: "center",
    gap: 10,
    padding: "12px 18px",
    background: "#2a1417",
    borderBottom: "1px solid #4a2028",
  },
  brand: { fontWeight: 700, letterSpacing: ".08em", color: "#ff6b6b" },
  count: { color: "#ff9a9a", flex: 1 },
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
  entry: { padding: "14px 18px", borderBottom: "1px solid #26262f" },
  entryHead: { display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 6 },
  kind: { color: "#ff9a9a", fontWeight: 600, fontSize: 12, textTransform: "uppercase", letterSpacing: ".06em" },
  message: { color: "#ff8a8a", fontWeight: 600, fontSize: 14, whiteSpace: "pre-wrap" },
  location: { color: "#8ab4ff", fontSize: 12, marginTop: 4 },
  pre: {
    margin: "10px 0 0",
    whiteSpace: "pre-wrap",
    background: "#0d0d12",
    border: "1px solid #26262f",
    borderRadius: 8,
    padding: "10px 12px",
    color: "#c9c9d4",
    maxHeight: "40vh",
    overflow: "auto",
    font: mono,
    fontSize: 12,
  },
  dim: { color: "#8d8d9c" },
  hint: { padding: "10px 18px", color: "#6f6f80", fontSize: 12 },
  badge: {
    position: "fixed",
    right: 72,
    // Under the terminal panel (2147483645), which shows the same status and errors.
    zIndex: 2147483644,
    font: mono,
    fontSize: 12,
    padding: "4px 10px",
    borderRadius: 999,
    border: "1px solid #33333f",
    boxShadow: "0 4px 16px rgba(0,0,0,.3)",
  },
  errorBadge: { bottom: 26, background: "#2a1417", color: "#ff9a9a", cursor: "pointer" },
  statusBadge: { bottom: 58, background: "#16161c", color: "#c9c9d4" },
} satisfies Record<string, CSSProperties>;
