/**
 * The VCL stylesheet, installed once per document by `Application.run`. Tokens are `--vcl-*` custom
 * properties; dark follows `prefers-color-scheme`.
 */

/** Joins class names, skipping empty ones. */
export const cx = (...names: (string | false | undefined)[]): string => names.filter(Boolean).join(" ");

export const css = `
:root {
  --vcl-bg: #f4f4f2; --vcl-surface: #ffffff; --vcl-fg: #1d1d1b; --vcl-muted: #6b6b66;
  --vcl-border: #d9d9d4; --vcl-accent: #2f5fd0; --vcl-accent-fg: #ffffff; --vcl-danger: #b3261e;
  --vcl-focus: 0 0 0 2px color-mix(in srgb, var(--vcl-accent) 45%, transparent);
  --vcl-radius: 6px; --vcl-gap: 8px; --vcl-pad: 12px;
  --vcl-font: system-ui, -apple-system, "Segoe UI", sans-serif; --vcl-size: 14px;
  --vcl-shadow: 0 10px 30px rgb(0 0 0 / 0.18);
}
@media (prefers-color-scheme: dark) {
  :root {
    --vcl-bg: #18181a; --vcl-surface: #222225; --vcl-fg: #ececea; --vcl-muted: #9a9a96;
    --vcl-border: #3a3a3e; --vcl-accent: #7c9cf0; --vcl-accent-fg: #101012; --vcl-danger: #f2867d;
  }
}
html, body { margin: 0; height: 100%; background: var(--vcl-bg); }
#app { height: 100%; }
.vcl-screen { height: 100%; font: var(--vcl-size)/1.4 var(--vcl-font); color: var(--vcl-fg); }
.vcl-screen *, .vcl-screen *::before, .vcl-screen *::after { box-sizing: border-box; }

.vcl-form { display: flex; flex-direction: column; height: 100%; background: var(--vcl-surface); }
.vcl-form-caption { display: flex; align-items: center; justify-content: space-between; padding: 10px var(--vcl-pad);
  font-weight: 600; border-bottom: 1px solid var(--vcl-border); }
.vcl-form-close { border: 0; background: none; color: var(--vcl-muted); font-size: 18px; cursor: pointer; }
.vcl-form-client, .vcl-panel { display: flex; flex-direction: column; gap: var(--vcl-gap); min-width: 0; min-height: 0; }
.vcl-form-client { flex: 1; padding: var(--vcl-pad); }
.vcl-form-error { padding: var(--vcl-pad); color: var(--vcl-danger); }

.vcl-window, .vcl-modal { position: fixed; inset: 0; display: grid; place-items: center; pointer-events: none; }
.vcl-modal { background: rgb(0 0 0 / 0.35); pointer-events: auto; }
.vcl-window > .vcl-form, .vcl-modal > .vcl-form { height: auto; min-width: 340px; max-width: calc(100vw - 32px);
  border: 1px solid var(--vcl-border); border-radius: var(--vcl-radius); box-shadow: var(--vcl-shadow); pointer-events: auto; }

.vcl-dir-row { flex-direction: row; align-items: center; }
.vcl-align-top, .vcl-align-left { order: -1; flex: none; }
.vcl-align-bottom, .vcl-align-right { order: 1; flex: none; }
.vcl-align-client { flex: 1; }
.vcl-panel-caption { font-weight: 600; color: var(--vcl-muted); }
.vcl-grow { flex: 1; }
.vcl-end { justify-content: flex-end; }

.vcl-label { color: var(--vcl-fg); }
.vcl-label-muted { color: var(--vcl-muted); }
.vcl-label-danger { color: var(--vcl-danger); }

.vcl-button { font: inherit; padding: 6px 14px; border: 1px solid var(--vcl-border); border-radius: var(--vcl-radius);
  background: var(--vcl-surface); color: var(--vcl-fg); cursor: pointer; }
.vcl-button:hover:not(:disabled) { border-color: var(--vcl-muted); }
.vcl-button-primary { background: var(--vcl-accent); border-color: var(--vcl-accent); color: var(--vcl-accent-fg); }
.vcl-button:disabled { opacity: 0.5; cursor: default; }

.vcl-edit { font: inherit; padding: 6px 10px; border: 1px solid var(--vcl-border); border-radius: var(--vcl-radius);
  background: var(--vcl-bg); color: var(--vcl-fg); min-width: 0; }
.vcl-memo { resize: vertical; }
.vcl-checkbox { display: inline-flex; gap: 6px; align-items: center; }

.vcl-listbox { margin: 0; padding: 4px; list-style: none; overflow: auto; border: 1px solid var(--vcl-border);
  border-radius: var(--vcl-radius); background: var(--vcl-bg); }
.vcl-listbox-item { padding: 6px 8px; border-radius: 4px; cursor: default; }
.vcl-listbox-item[aria-selected="true"] { background: var(--vcl-accent); color: var(--vcl-accent-fg); }
.vcl-listbox-empty { padding: 6px 8px; color: var(--vcl-muted); }

.vcl-button:focus-visible, .vcl-edit:focus-visible, .vcl-listbox:focus-visible { outline: none; box-shadow: var(--vcl-focus); }

.vcl-errors { position: fixed; right: 16px; bottom: 16px; display: grid; gap: 8px; max-width: min(420px, calc(100vw - 32px)); }
.vcl-error { display: flex; gap: 12px; align-items: start; padding: 10px 12px; border-radius: var(--vcl-radius);
  background: var(--vcl-surface); border: 1px solid var(--vcl-danger); color: var(--vcl-fg); box-shadow: var(--vcl-shadow); }
.vcl-error > span { flex: 1; }
@media (prefers-reduced-motion: no-preference) { .vcl-button, .vcl-listbox-item { transition: background-color 120ms, border-color 120ms; } }
`;

/** Adds the stylesheet to `document` once. */
export function installStyles(document: Document): void {
  if (document.querySelector("style[data-vcl]")) return;
  const style = document.createElement("style");
  style.dataset.vcl = "";
  style.textContent = css;
  document.head.append(style);
}
