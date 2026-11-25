/**
 * The panel's stylesheet, injected as a `<style>` inside the portal.
 * Decision: plain CSS under the `.lsdm` prefix instead of a CSS file, so the package needs no bundler CSS support
 * and the explicit rules below override app-wide `button`/`input` styles.
 */
export const STYLES = `
.lsdm, .lsdm * { box-sizing: border-box; }
.lsdm {
  --bg: #16181d; --bg2: #1d2027; --bg3: #262a33; --line: #30343e; --fg: #d7dae0; --dim: #8a909c;
  --acc: #5b9cff; --ok: #4ec98a; --warn: #e5b74a; --err: #ef6b6b; --str: #e3a86b; --num: #a98bff; --key: #7fc6e8;
  font: 12px/1.45 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif; color: var(--fg);
}
.lsdm button, .lsdm input, .lsdm select, .lsdm textarea { font: inherit; color: inherit; margin: 0; }
.lsdm input, .lsdm select, .lsdm textarea {
  background: var(--bg); border: 1px solid var(--line); border-radius: 4px; padding: 4px 6px; outline: none; min-width: 0;
}
.lsdm input:focus, .lsdm select:focus, .lsdm textarea:focus { border-color: var(--acc); }
.lsdm input[type=checkbox], .lsdm input[type=radio] { accent-color: var(--acc); padding: 0; }
.lsdm button { cursor: pointer; background: none; border: 0; padding: 0; }
.lsdm code, .lsdm pre, .lsdm-list, .lsdm-repl textarea, .lsdm-urlinput, .lsdm-bodyedit, .lsdm-json, .lsdm-kv, .lsdm-pairs input {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 12px;
}
.lsdm-launcher {
  position: fixed; right: 16px; bottom: 16px; z-index: 2147483646; padding: 6px 10px !important; border-radius: 999px !important;
  background: var(--bg) !important; color: var(--fg); border: 1px solid var(--line) !important; font-weight: 700; letter-spacing: .06em;
  box-shadow: 0 4px 16px rgba(0,0,0,.35); display: flex; align-items: center; gap: 6px;
}
.lsdm-launcher:hover { border-color: var(--acc) !important; }
.lsdm-badge { background: var(--err); color: #fff; border-radius: 999px; padding: 0 6px; font-size: 11px; }
.lsdm-panel {
  position: fixed; left: 0; right: 0; bottom: 0; z-index: 2147483647; background: var(--bg); border-top: 1px solid var(--line);
  display: flex; flex-direction: column; box-shadow: 0 -6px 24px rgba(0,0,0,.35);
}
.lsdm-resize { position: absolute; top: -4px; left: 0; right: 0; height: 8px; cursor: ns-resize; }
.lsdm-content { flex: 1; min-height: 0; display: flex; }
.lsdm-content[hidden] { display: none; }
.lsdm-tabbar { display: flex; align-items: center; gap: 2px; padding: 0 6px; border-bottom: 1px solid var(--line); background: var(--bg2); min-height: 30px; flex-shrink: 0; }
.lsdm-tabbar > .lsdm-status, .lsdm-tabbar > .lsdm-dim, .lsdm-tabbar > .lsdm-chip { margin-left: 8px; }
.lsdm-tab { padding: 6px 10px !important; color: var(--dim) !important; border-bottom: 2px solid transparent !important; display: flex; gap: 6px; align-items: center; }
.lsdm-tab:hover { color: var(--fg) !important; }
.lsdm-tab.lsdm-on { color: var(--fg) !important; border-bottom-color: var(--acc) !important; }
.lsdm-grow { flex: 1; }
.lsdm-x { color: var(--dim) !important; font-size: 15px; line-height: 1; padding: 0 4px !important; }
.lsdm-x:hover { color: var(--err) !important; }
.lsdm-close { font-size: 18px; }
.lsdm-btn, .lsdm-chip { border: 1px solid var(--line) !important; border-radius: 4px !important; padding: 3px 8px !important; background: var(--bg3) !important; white-space: nowrap; }
.lsdm-btn:hover, .lsdm-chip:hover { border-color: var(--acc) !important; }
.lsdm-chip { background: transparent !important; display: inline-flex; gap: 5px; align-items: center; }
.lsdm-chip.lsdm-on { background: var(--bg3) !important; border-color: var(--acc) !important; }
.lsdm-count { border-radius: 999px; padding: 0 5px; font-size: 10px; background: var(--bg3); }
.lsdm-c-error { background: var(--err); color: #fff; }
.lsdm-c-warn { background: var(--warn); color: #000; }
.lsdm-dim { color: var(--dim); }
.lsdm-empty { color: var(--dim); padding: 16px; text-align: center; }
.lsdm-hint { color: var(--dim); padding: 6px 2px; }
.lsdm-warn { color: var(--warn); padding: 4px 0; }

/* console */
.lsdm-console { display: flex; flex-direction: column; flex: 1; min-width: 0; }
.lsdm-toolbar { display: flex; gap: 6px; align-items: center; padding: 5px 8px; border-bottom: 1px solid var(--line); flex-wrap: wrap; }
.lsdm-search { margin-left: auto; width: 200px; }
.lsdm-list { flex: 1; overflow: auto; }
.lsdm-row { display: flex; gap: 8px; align-items: baseline; padding: 3px 8px; border-bottom: 1px solid #22252c; }
.lsdm-time { color: var(--dim); flex-shrink: 0; font-size: 11px; }
.lsdm-args { display: flex; flex-wrap: wrap; gap: 0 8px; min-width: 0; white-space: pre-wrap; word-break: break-word; }
.lsdm-mark { color: var(--dim); }
.lsdm-t-warn { background: rgba(229,183,74,.08); color: var(--warn); }
.lsdm-t-error { background: rgba(239,107,107,.09); color: var(--err); }
.lsdm-t-debug { color: var(--dim); }
.lsdm-t-info .lsdm-str-top { color: var(--key); }
.lsdm-t-input .lsdm-code { color: var(--acc); white-space: pre-wrap; }
.lsdm-method { font-weight: 700; width: 56px; flex-shrink: 0; }
.lsdm-url { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.lsdm-status { font-weight: 700; padding: 0 5px; border-radius: 3px; font-size: 11px; white-space: nowrap; }
.lsdm-s-ok { color: var(--ok); background: rgba(78,201,138,.12); }
.lsdm-s-redir { color: var(--warn); background: rgba(229,183,74,.12); }
.lsdm-s-fail { color: var(--err); background: rgba(239,107,107,.12); }
.lsdm-link { color: var(--acc) !important; white-space: nowrap; }
.lsdm-link:hover { text-decoration: underline; }
.lsdm-repl { display: flex; gap: 6px; align-items: flex-start; border-top: 1px solid var(--line); padding: 5px 8px; }
.lsdm-prompt { color: var(--acc); font-weight: 700; padding-top: 4px; }
.lsdm-repl textarea { flex: 1; resize: none; border: 0 !important; background: transparent !important; padding: 4px 0 !important; }

/* values */
.lsdm-str { color: var(--str); }
.lsdm-num { color: var(--num); }
.lsdm-nil { color: var(--dim); }
.lsdm-fn { color: var(--key); font-style: italic; }
.lsdm-err-val { color: var(--err); white-space: pre-wrap; }
.lsdm-toggle { cursor: pointer; color: var(--fg); }
.lsdm-toggle:hover { color: #fff; }
.lsdm-children { display: block; padding-left: 16px; border-left: 1px solid var(--line); margin-left: 3px; }
.lsdm-child { display: block; }
.lsdm-key { color: var(--key); }

/* api */
.lsdm-api { display: flex; flex: 1; min-width: 0; position: relative; }
.lsdm-side { width: 240px; flex-shrink: 0; border-right: 1px solid var(--line); display: flex; flex-direction: column; background: var(--bg2); }
.lsdm-side-body { flex: 1; overflow: auto; padding: 6px; }
.lsdm-side-actions { display: flex; gap: 6px; margin-bottom: 6px; }
.lsdm-item { display: flex; gap: 6px; align-items: center; padding: 4px 6px; border-radius: 4px; cursor: pointer; }
.lsdm-item:hover { background: var(--bg3); }
.lsdm-item.lsdm-on { background: var(--bg3); outline: 1px solid var(--line); }
.lsdm-item-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.lsdm-m { font-weight: 700; font-size: 10px; width: 46px; flex-shrink: 0; }
.lsdm-m-GET { color: var(--ok); } .lsdm-m-POST { color: var(--warn); } .lsdm-m-PUT { color: var(--acc); }
.lsdm-m-PATCH { color: var(--num); } .lsdm-m-DELETE { color: var(--err); } .lsdm-m-HEAD, .lsdm-m-OPTIONS { color: var(--key); }
.lsdm-main { flex: 1; min-width: 0; display: flex; flex-direction: column; }
.lsdm-namebar, .lsdm-urlbar { display: flex; gap: 6px; padding: 6px 8px 0; align-items: center; }
.lsdm-name { flex: 1; border-color: transparent !important; background: transparent !important; font-weight: 600; font-size: 13px; }
.lsdm-name:hover, .lsdm-name:focus { border-color: var(--line) !important; }
.lsdm-urlbar select { font-weight: 700; width: 96px; }
.lsdm-urlinput { flex: 1; padding: 6px 8px !important; }
.lsdm-send { background: var(--acc) !important; color: #fff !important; border-radius: 4px !important; padding: 6px 18px !important; font-weight: 600; }
.lsdm-send:disabled { opacity: .45; cursor: default; }
.lsdm-cancel { background: var(--err) !important; }
.lsdm-main > .lsdm-warn { padding: 4px 8px 0; }
.lsdm-main > .lsdm-tabbar { margin-top: 6px; background: transparent; }
.lsdm-req-body { padding: 6px 8px; max-height: 40%; min-height: 70px; overflow: auto; border-bottom: 1px solid var(--line); flex-shrink: 0; }
.lsdm-row-flex { display: flex; gap: 12px; align-items: center; margin-bottom: 6px; }
.lsdm-radio { display: inline-flex; gap: 4px; align-items: center; cursor: pointer; }
.lsdm-bodyedit { width: 100%; min-height: 110px; resize: vertical; }
.lsdm-auth { display: flex; gap: 6px; flex-wrap: wrap; }
.lsdm-auth input { width: 260px; }
.lsdm-pairs { width: 100%; border-collapse: collapse; }
.lsdm-pairs td { padding: 1px 2px; }
.lsdm-pairs input:not([type]) { width: 100%; }
.lsdm-pairs .lsdm-check { width: 22px; text-align: center; }
.lsdm-off input:not([type=checkbox]) { opacity: .45; }
.lsdm-res { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.lsdm-res-body { flex: 1; overflow: auto; padding: 6px 8px; }
.lsdm-res-error { color: var(--err); padding: 12px 8px; }
.lsdm-pre { margin: 0; white-space: pre-wrap; word-break: break-word; }
.lsdm-json { white-space: pre-wrap; }
.lsdm-kv { border-collapse: collapse; }
.lsdm-kv td { padding: 2px 12px 2px 0; vertical-align: top; word-break: break-all; }
.lsdm-kv td:first-child { color: var(--key); white-space: nowrap; }
.lsdm-modal { position: absolute; inset: 0; background: rgba(0,0,0,.5); display: flex; align-items: center; justify-content: center; }
.lsdm-dialog { background: var(--bg2); border: 1px solid var(--line); border-radius: 6px; padding: 12px; width: min(640px, 90%); display: flex; flex-direction: column; gap: 8px; }
`;
