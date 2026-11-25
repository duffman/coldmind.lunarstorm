/**
 * The dev panel's stylesheet, injected as a `<style>` inside the portal.
 *
 * Design: an Ubuntu/GNOME floating window. Aubergine title bar with a warm orange glow, frosted switcher, orange
 * close button; colour-coded content (levels, methods, statuses, JSON). Light and dark share the brand colours and
 * differ only in the neutral ladder (warm greys in light, aubergine-black in dark), switched by `data-theme` on
 * <html>. Rules kept from the non-ass-ugly-ui skill: no pure white, gradient fills, shadow depth, one top-left light.
 * Fonts: Ubuntu (chrome, titles), Roboto (text), Ubuntu Mono (code).
 */
export const STYLES = `
.lsdm, .lsdm * { box-sizing: border-box; }
.lsdm {
  /* brand, shared by both themes */
  --orange: oklch(66% .19 40); --orange2: oklch(75% .16 60); --aub: oklch(38% .14 335); --plum: oklch(27% .09 328);
  --teal: oklch(68% .12 200); --magenta: oklch(62% .2 350); --green: oklch(66% .16 150); --red: oklch(62% .2 25);
  --amber: oklch(78% .15 78); --blue: oklch(64% .15 255); --violet: oklch(62% .17 300);
  --title: radial-gradient(420px 140px at 0% 0%, oklch(66% .19 40 / .45), transparent 70%),
           linear-gradient(135deg, oklch(37% .13 342), oklch(28% .1 332) 55%, oklch(23% .07 322));
  /* light neutrals (warm) */
  --s0: oklch(93.5% .007 60); --s1: oklch(96.3% .006 60); --s2: oklch(98.1% .004 60); --s3: oklch(99.2% .003 60);
  --line: oklch(90% .008 60); --text: oklch(25% .02 330); --soft: oklch(45% .02 330); --mute: oklch(60% .015 330);
  --tint: .12; --key: oklch(48% .12 210); --str: oklch(52% .15 45); --num: oklch(50% .2 350);
  --sh-sm: 1px 2px 4px -2px oklch(35% .04 330 / .25);
  --sh-md: 8px 10px 24px -14px oklch(35% .05 330 / .32), 1px 2px 5px -3px oklch(35% .04 330 / .2);
  --sh-lg: 16px 22px 56px -20px oklch(25% .08 330 / .5), 2px 3px 10px -4px oklch(25% .05 330 / .25);
  --hi: -6px -6px 16px -10px oklch(100% 0 0 / .9);
  --in: inset 1px 1px 3px oklch(35% .04 330 / .18), inset -1px -1px 2px oklch(100% 0 0 / .8);
  --raised: linear-gradient(160deg, var(--s3), var(--s2));
  --ui: "Ubuntu", system-ui, sans-serif; --body: "Roboto", system-ui, sans-serif; --mono: "Ubuntu Mono", ui-monospace, Menlo, monospace;
  font: 13px/1.5 var(--body); color: var(--text); -webkit-font-smoothing: antialiased;
}
:root[data-theme="dark"] .lsdm {
  color-scheme: dark;
  /* dark neutrals: aubergine-black ladder, page darkest */
  --s0: oklch(17.5% .018 330); --s1: oklch(20.5% .02 330); --s2: oklch(23.5% .022 330); --s3: oklch(26.5% .024 330);
  --line: oklch(29% .025 330); --text: oklch(94% .01 330); --soft: oklch(78% .02 330); --mute: oklch(60% .025 330);
  --tint: .2; --key: oklch(80% .1 205); --str: oklch(80% .13 60); --num: oklch(76% .16 350);
  --sh-sm: 1px 2px 4px -2px oklch(0% 0 0 / .6);
  --sh-md: 8px 10px 24px -14px oklch(0% 0 0 / .7), 1px 2px 5px -3px oklch(0% 0 0 / .5);
  --sh-lg: 16px 22px 56px -18px oklch(0% 0 0 / .8), 2px 3px 10px -4px oklch(0% 0 0 / .55);
  --hi: -6px -6px 16px -10px oklch(100% 0 0 / .06), inset 1px 1px 0 oklch(100% 0 0 / .05);
  --in: inset 1px 1px 3px oklch(0% 0 0 / .5), inset -1px -1px 1px oklch(100% 0 0 / .05);
}
.lsdm button, .lsdm input, .lsdm select, .lsdm textarea { font: inherit; color: inherit; margin: 0; }
.lsdm button { cursor: pointer; background: none; border: 0; padding: 0; }
.lsdm input, .lsdm select, .lsdm textarea {
  color: var(--text); background: var(--s0); border: 0; border-radius: 9px; padding: 6px 10px; outline: none; min-width: 0; box-shadow: var(--in);
}
.lsdm input:focus, .lsdm select:focus, .lsdm textarea:focus { box-shadow: var(--in), 0 0 0 2px oklch(66% .19 40 / .55); }
.lsdm input::placeholder, .lsdm textarea::placeholder { color: var(--mute); }
.lsdm input[type=checkbox], .lsdm input[type=radio] { accent-color: var(--orange); padding: 0; box-shadow: none; }
.lsdm code, .lsdm pre, .lsdm-list, .lsdm-repl textarea, .lsdm-urlinput, .lsdm-cmdbox textarea, .lsdm-res-body, .lsdm-cmd-name, .lsdm-method, .lsdm-time {
  font-family: var(--mono); font-size: 13.5px;
}
.lsdm-tab, .lsdm-btn, .lsdm-chip, .lsdm-send, .lsdm-launcher, .lsdm-item, .lsdm-status, .lsdm-m, .lsdm-conn, .lsdm-proto, .lsdm-run-meta { font-family: var(--ui); }

/* launcher: an aubergine pill with the orange status dot */
.lsdm-launcher {
  position: fixed; right: 18px; bottom: 18px; z-index: 2147483646; padding: 9px 16px 9px 12px !important; border-radius: 999px !important;
  background: var(--title) !important; color: oklch(97% .01 330) !important; font-weight: 500; font-size: 13px;
  box-shadow: 8px 12px 28px -12px oklch(25% .1 330 / .7), inset 1px 1px 0 oklch(100% 0 0 / .18); display: flex; align-items: center; gap: 9px;
}
.lsdm-launcher::before { content: ""; width: 9px; height: 9px; border-radius: 50%; background: var(--orange2); box-shadow: 0 0 0 3px oklch(66% .19 40 / .35), 0 0 12px var(--orange); }
.lsdm-launcher:hover { filter: brightness(1.12); }
.lsdm-badge { color: #fff; background: linear-gradient(145deg, oklch(70% .19 30), var(--red)); border-radius: 999px; padding: 0 7px; font-size: 11px; font-weight: 700; }

/* the window */
/* docked: flush with the bottom and both sides, square corners; the shadow is cast upward onto the page */
.lsdm-panel {
  position: fixed; left: 0; right: 0; bottom: 0; z-index: 2147483647; border-radius: 0; overflow: hidden;
  display: flex; flex-direction: column; background: linear-gradient(170deg, var(--s1), var(--s0));
  box-shadow: 0 -10px 30px -12px oklch(20% .06 330 / .45);
}
/* react-resizable's top handle: a 10px strip over the title bar edge with a grip pill that lights orange on hover/drag */
.lsdm-resize { position: absolute; top: 0; left: 0; right: 0; height: 10px; cursor: ns-resize; z-index: 2; touch-action: none; }
.lsdm-resize::after {
  content: ""; position: absolute; top: 4px; left: 50%; width: 44px; height: 4px; margin-left: -22px; border-radius: 4px;
  background: oklch(100% 0 0 / .28); transition: background .15s, width .15s, margin .15s;
}
.lsdm-resize:hover::after, .lsdm-resize:active::after { width: 80px; margin-left: -40px; background: var(--orange2); }
.lsdm-content { flex: 1; min-height: 0; display: flex; gap: 12px; padding: 12px; }
.lsdm-content[hidden] { display: none; }

/* title bar: aubergine with orange glow, title left, frosted switcher centred, orange close right */
.lsdm-tabbar { display: flex; align-items: center; gap: 4px; flex-shrink: 0; }
.lsdm-panel > .lsdm-tabbar {
  position: relative; height: 50px; padding: 0 12px 0 18px; gap: 3px; color: oklch(97% .01 330);
  background: var(--title); box-shadow: inset 0 1px 0 oklch(100% 0 0 / .16), 0 6px 16px -10px oklch(20% .08 330 / .7);
}
.lsdm-panel > .lsdm-tabbar::before {
  content: "Dev mode"; position: absolute; left: 18px; font: 500 14.5px var(--ui); color: oklch(98% .01 330);
  padding-left: 20px; background: radial-gradient(circle at 5px 50%, var(--orange2) 4px, oklch(66% .19 40 / .35) 5px, transparent 8px);
}
.lsdm-panel > .lsdm-tabbar > .lsdm-grow { display: none; }
.lsdm-panel > .lsdm-tabbar > .lsdm-tab:first-of-type { margin-left: auto; }
.lsdm-panel > .lsdm-tabbar > .lsdm-min { margin-left: auto; }
/* minimized: only the title bar shows; the tab contents stay mounted */
.lsdm-minimized > .lsdm-content { display: none !important; }
.lsdm-panel > .lsdm-tabbar > .lsdm-tab {
  padding: 6px 18px !important; border-radius: 9px !important; color: oklch(90% .02 330 / .8) !important; font: 500 13px var(--ui);
  background: oklch(100% 0 0 / .06) !important; display: flex; gap: 7px; align-items: center;
}
.lsdm-panel > .lsdm-tabbar > .lsdm-tab:hover { color: #fff !important; background: oklch(100% 0 0 / .12) !important; }
.lsdm-panel > .lsdm-tabbar > .lsdm-tab.lsdm-on {
  color: #fff !important; background: linear-gradient(160deg, oklch(100% 0 0 / .26), oklch(100% 0 0 / .14)) !important;
  box-shadow: inset 1px 1px 0 oklch(100% 0 0 / .3), 2px 3px 8px -3px oklch(10% .05 330 / .6);
}
.lsdm-min {
  width: 26px; height: 26px; padding: 0 !important; border-radius: 50% !important; display: grid; place-items: center; font-size: 13px; color: #fff !important;
  background: linear-gradient(145deg, var(--orange2), var(--orange)) !important;
  box-shadow: inset 1px 1px 0 oklch(100% 0 0 / .35), 2px 3px 8px -3px oklch(30% .12 40 / .7);
}
.lsdm-min:hover { filter: brightness(1.1); }
.lsdm-glyph-min { width: 10px; height: 2px; border-radius: 1px; background: #fff; margin-top: 5px; }
.lsdm-glyph-restore { width: 7px; height: 7px; border: solid #fff; border-width: 2px 2px 0 0; transform: rotate(-45deg); margin-top: 4px; }
.lsdm-x { color: var(--mute) !important; font-size: 15px; line-height: 1; padding: 2px 6px !important; border-radius: 7px !important; }
.lsdm-x:hover { color: var(--red) !important; background: oklch(62% .2 25 / var(--tint)) !important; }

/* generic tabs, buttons, chips, counts */
.lsdm-tab { padding: 5px 14px !important; border-radius: 8px !important; color: var(--soft) !important; font-weight: 500; display: flex; gap: 7px; align-items: center; }
.lsdm-tab:hover { color: var(--text) !important; }
.lsdm-tab.lsdm-on { color: var(--text) !important; background: var(--raised) !important; box-shadow: var(--sh-sm), var(--hi); }
.lsdm-grow { flex: 1; }
.lsdm-btn, .lsdm-chip { padding: 5px 12px !important; border-radius: 8px !important; font-weight: 500; white-space: nowrap; }
.lsdm-btn { color: var(--text); background: var(--raised) !important; box-shadow: var(--sh-sm), var(--hi); }
.lsdm-btn:hover { box-shadow: var(--sh-md), var(--hi); color: var(--orange); }
.lsdm-chip { color: var(--soft) !important; display: inline-flex; gap: 6px; align-items: center; --c: var(--orange); }
.lsdm-chip:hover { color: var(--text) !important; }
.lsdm-chip.lsdm-on {
  color: #fff !important; background: linear-gradient(145deg, color-mix(in oklch, var(--c) 78%, white), var(--c)) !important;
  box-shadow: 3px 5px 12px -6px var(--c), inset 1px 1px 0 oklch(100% 0 0 / .3);
}
.lsdm-f-error { --c: var(--red); } .lsdm-f-warn { --c: oklch(70% .15 70); } .lsdm-f-info { --c: var(--blue); } .lsdm-f-net { --c: var(--teal); }
.lsdm-count { border-radius: 999px; padding: 0 6px; font-size: 10.5px; font-weight: 700; background: oklch(60% .02 330 / .18); color: inherit; }
.lsdm-chip .lsdm-c-error, .lsdm-tab .lsdm-c-error { background: linear-gradient(145deg, oklch(70% .19 30), var(--red)); color: #fff; }
.lsdm-chip .lsdm-c-warn { background: linear-gradient(145deg, oklch(85% .14 85), var(--amber)); color: oklch(30% .08 70); }
.lsdm-chip .lsdm-c-net { background: linear-gradient(145deg, oklch(78% .1 200), var(--teal)); color: oklch(22% .05 210); }
.lsdm-chip.lsdm-on .lsdm-count { background: oklch(100% 0 0 / .28); color: #fff; }
.lsdm-dim { color: var(--mute); }
.lsdm-empty { color: var(--mute); padding: 18px; text-align: center; }
.lsdm-hint { color: var(--soft); padding: 4px 2px 8px; }
.lsdm-hint code { color: var(--orange); background: oklch(66% .19 40 / var(--tint)); border-radius: 5px; padding: 0 4px; }
.lsdm-warn { color: oklch(62% .14 65); padding: 2px 0; font-size: 12px; }

/* lit main surfaces */
.lsdm-console, .lsdm-main {
  border-radius: 12px; min-width: 0;
  background: linear-gradient(135deg, var(--s3) 0%, var(--s2) 55%, var(--s1) 100%);
  box-shadow: -10px -10px 26px -18px oklch(100% 0 0 / .5), 12px 14px 32px -18px oklch(25% .06 330 / .35), inset 1px 1px 0 oklch(100% 0 0 / .05);
}

/* console */
.lsdm-console { display: flex; flex-direction: column; flex: 1; overflow: hidden; }
.lsdm-toolbar { display: flex; gap: 4px; align-items: center; padding: 10px 10px 8px; flex-wrap: wrap; box-shadow: 0 1px 0 var(--line); }
.lsdm-search { margin-left: auto; width: 220px; }
.lsdm-toolbar .lsdm-btn { margin-left: 6px; }
.lsdm-list { flex: 1; overflow: auto; padding: 6px 8px; }
.lsdm-row {
  --lv: transparent; display: flex; gap: 12px; align-items: baseline; padding: 6px 12px 6px 16px; border-radius: 8px; position: relative;
  box-shadow: inset 0 -1px 0 color-mix(in oklch, var(--line) 70%, transparent);
}
.lsdm-row::before { content: ""; position: absolute; left: 4px; top: 7px; bottom: 7px; width: 3px; border-radius: 3px; background: var(--lv); }
.lsdm-row:hover { background: oklch(60% .03 330 / .07); }
.lsdm-time { color: var(--mute); flex-shrink: 0; font-size: 12px !important; }
.lsdm-args { display: flex; flex-wrap: wrap; gap: 0 8px; min-width: 0; white-space: pre-wrap; word-break: break-word; }
.lsdm-mark { color: var(--orange); font-weight: 700; }
.lsdm-t-log { --lv: oklch(70% .02 330 / .5); }
.lsdm-t-info { --lv: var(--blue); } .lsdm-t-info .lsdm-str-top { color: var(--blue); }
.lsdm-t-debug { --lv: var(--mute); color: var(--mute); }
.lsdm-t-warn { --lv: var(--amber); background: linear-gradient(100deg, oklch(80% .15 85 / var(--tint)), transparent 80%); color: oklch(55% .13 65); }
.lsdm-t-error { --lv: var(--red); background: linear-gradient(100deg, oklch(62% .2 25 / var(--tint)), transparent 80%); color: oklch(55% .19 25); }
:root[data-theme="dark"] .lsdm-t-warn { color: oklch(86% .12 85); }
:root[data-theme="dark"] .lsdm-t-error { color: oklch(80% .13 25); }
.lsdm-t-net { --lv: var(--teal); }
.lsdm-t-input { --lv: var(--orange); } .lsdm-t-input .lsdm-code { color: var(--orange); white-space: pre-wrap; }
.lsdm-t-result { --lv: var(--magenta); }
.lsdm-url { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.lsdm-repl {
  display: flex; gap: 10px; align-items: flex-start; margin: 0 8px 8px; padding: 9px 14px; border-radius: 10px; background: var(--s0);
  box-shadow: var(--in);
}
.lsdm-repl:focus-within { box-shadow: var(--in), 0 0 0 2px oklch(66% .19 40 / .5); }
.lsdm-prompt { color: var(--orange); font-weight: 700; padding-top: 1px; }
.lsdm-repl textarea { flex: 1; resize: none; background: transparent !important; box-shadow: none !important; padding: 1px 0 !important; }

/* methods: filled colour chips (lists) and coloured text (console rows, method picker) */
.lsdm-m-GET { --mc: var(--green); } .lsdm-m-POST { --mc: var(--orange); } .lsdm-m-PUT { --mc: var(--blue); }
.lsdm-m-PATCH { --mc: var(--violet); } .lsdm-m-DELETE { --mc: var(--red); } .lsdm-m-HEAD, .lsdm-m-OPTIONS { --mc: var(--teal); }
.lsdm-m {
  font-weight: 700; font-size: 9.5px; letter-spacing: .04em; width: 50px; flex-shrink: 0; text-align: center; padding: 2px 0; border-radius: 6px;
  color: #fff; background: linear-gradient(145deg, color-mix(in oklch, var(--mc) 78%, white), var(--mc)); box-shadow: inset 1px 1px 0 oklch(100% 0 0 / .25);
}
.lsdm-row > .lsdm-method { font-weight: 700; width: 56px; flex-shrink: 0; font-size: 12px !important; color: var(--mc); }
.lsdm-status { font-weight: 700; padding: 1px 9px; border-radius: 99px; font-size: 11px; white-space: nowrap; color: #fff; }
.lsdm-s-ok { background: linear-gradient(145deg, oklch(74% .15 150), var(--green)); }
.lsdm-s-redir { background: linear-gradient(145deg, oklch(84% .14 85), var(--amber)); color: oklch(30% .08 70); }
.lsdm-s-fail { background: linear-gradient(145deg, oklch(70% .19 30), var(--red)); }

/* values / JSON */
.lsdm-str { color: var(--str); }
.lsdm-num { color: var(--num); }
.lsdm-nil { color: var(--mute); font-style: italic; }
.lsdm-fn { color: var(--violet); font-style: italic; }
.lsdm-err-val { color: var(--red); white-space: pre-wrap; }
.lsdm-toggle { cursor: pointer; }
.lsdm-toggle:hover { color: var(--orange); }
.lsdm-children { display: block; padding-left: 16px; margin-left: 4px; box-shadow: inset 2px 0 0 oklch(66% .19 40 / .25); }
.lsdm-child { display: block; }
.lsdm-key { color: var(--key); }

/* api: redis-protocol executor */
.lsdm-api { display: flex; flex: 1; min-width: 0; gap: 12px; position: relative; }
.lsdm-side {
  width: 290px; flex-shrink: 0; border-radius: 12px; display: flex; flex-direction: column; overflow: hidden;
  background: linear-gradient(170deg, var(--s2), var(--s1)); box-shadow: var(--sh-md), var(--hi);
}
.lsdm-side > .lsdm-tabbar { margin: 8px; padding: 3px; border-radius: 10px; background: var(--s0); box-shadow: var(--in); gap: 3px; }
.lsdm-side > .lsdm-tabbar .lsdm-tab { flex: 1; justify-content: center; padding: 5px 0 !important; }
.lsdm-side > .lsdm-tabbar .lsdm-tab.lsdm-on { color: var(--orange) !important; }
.lsdm-side-body { flex: 1; overflow: auto; padding: 0 8px 8px; }
.lsdm-side-actions { display: flex; gap: 6px; margin-bottom: 8px; }
.lsdm-side-actions .lsdm-btn { flex: 1; }
.lsdm-item { display: flex; gap: 9px; align-items: center; padding: 7px 8px; border-radius: 9px; cursor: pointer; color: var(--soft); margin-bottom: 2px; }
.lsdm-item:hover { background: oklch(60% .03 330 / .08); color: var(--text); }
.lsdm-item.lsdm-on {
  color: var(--text); font-weight: 500; background: linear-gradient(100deg, oklch(66% .19 40 / var(--tint)), oklch(66% .19 40 / .03));
  box-shadow: inset 3px 0 0 var(--orange), var(--sh-sm);
}
.lsdm-item-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.lsdm-main { flex: 1; display: flex; flex-direction: column; padding: 14px 16px; gap: 10px; overflow: hidden; }
.lsdm-urlbar { display: flex; align-items: center; gap: 10px; padding: 4px; border-radius: 13px; background: var(--s0); box-shadow: var(--in); }
.lsdm-urlinput { flex: 1; padding: 8px 6px !important; background: transparent !important; box-shadow: none !important; }
.lsdm-send {
  padding: 8px 26px !important; border-radius: 10px !important; color: #fff !important; font-weight: 500; font-size: 13.5px;
  background: linear-gradient(145deg, var(--orange2), var(--orange)) !important;
  box-shadow: 0 6px 18px -6px oklch(66% .19 40 / .75), inset 1px 1px 0 oklch(100% 0 0 / .35);
}
.lsdm-send:hover { filter: brightness(1.07); }
.lsdm-send:disabled { opacity: .5; cursor: default; filter: none; box-shadow: none; }
.lsdm-res {
  flex: 1; min-height: 0; display: flex; flex-direction: column; border-radius: 12px; overflow: hidden;
  background: var(--raised); box-shadow: var(--sh-md), var(--hi);
}
.lsdm-res > .lsdm-tabbar { padding: 8px 10px; flex-wrap: wrap; row-gap: 4px; box-shadow: 0 1px 0 var(--line); }
.lsdm-res > .lsdm-tabbar > .lsdm-status { font-size: 12px; padding: 2px 11px; margin-left: 6px; }
.lsdm-res > .lsdm-tabbar > .lsdm-dim, .lsdm-res > .lsdm-tabbar > .lsdm-chip { margin-left: 6px; }
.lsdm-res.lsdm-empty { display: flex; align-items: center; justify-content: center; }
.lsdm-res-body { flex: 1; overflow: auto; padding: 8px 16px 12px; line-height: 1.7; }
.lsdm-btn:disabled { opacity: .5; cursor: default; }
.lsdm-proto {
  font: 700 12px var(--ui); letter-spacing: .04em; padding: 8px 14px; border-radius: 10px; color: #fff;
  background: linear-gradient(145deg, oklch(70% .2 25), oklch(55% .21 25)); box-shadow: inset 1px 1px 0 oklch(100% 0 0 / .3), 2px 3px 8px -4px var(--red);
}
.lsdm-conn { font-weight: 700; font-size: 11px; padding: 2px 10px; border-radius: 99px; white-space: nowrap; color: #fff; display: inline-flex; gap: 6px; align-items: center; }
.lsdm-conn::before { content: ""; width: 7px; height: 7px; border-radius: 50%; background: currentColor; opacity: .9; }
.lsdm-st-open { background: linear-gradient(145deg, oklch(74% .15 150), var(--green)); }
.lsdm-st-connecting { background: linear-gradient(145deg, oklch(84% .14 85), var(--amber)); color: oklch(30% .08 70); }
.lsdm-st-error { background: linear-gradient(145deg, oklch(70% .19 30), var(--red)); }
.lsdm-st-idle, .lsdm-st-closed { background: oklch(60% .02 330 / .35); color: var(--text); }
.lsdm-urlbar > .lsdm-btn { margin-right: 4px; }
.lsdm-cmdbox {
  display: flex; gap: 10px; align-items: flex-start; padding: 8px 8px 8px 14px; border-radius: 13px; background: var(--s0); box-shadow: var(--in);
}
.lsdm-cmdbox:focus-within { box-shadow: var(--in), 0 0 0 2px oklch(66% .19 40 / .5); }
.lsdm-cmdbox .lsdm-prompt { padding-top: 7px; font-size: 15px; }
.lsdm-cmdbox textarea { flex: 1; resize: none; background: transparent !important; box-shadow: none !important; padding: 7px 0 !important; line-height: 1.5; font-size: 14px; }
.lsdm-cmd-name { font-weight: 700; font-size: 11px !important; color: var(--red); min-width: 54px; }
.lsdm-item .lsdm-x { opacity: 0; } .lsdm-item:hover .lsdm-x { opacity: 1; }
.lsdm-count.lsdm-c-push { background: linear-gradient(145deg, oklch(78% .1 200), var(--teal)); color: oklch(22% .05 210); }
.lsdm-res-body { line-height: 1.55; }
.lsdm-run { --lv: var(--orange); position: relative; padding: 8px 12px 10px 16px; border-radius: 10px; box-shadow: inset 0 -1px 0 color-mix(in oklch, var(--line) 70%, transparent); }
.lsdm-run::before { content: ""; position: absolute; left: 4px; top: 9px; bottom: 9px; width: 3px; border-radius: 3px; background: var(--lv); }
.lsdm-run-err { --lv: var(--red); background: linear-gradient(100deg, oklch(62% .2 25 / var(--tint)), transparent 70%); }
.lsdm-run-push { --lv: var(--teal); }
.lsdm-run-meta { display: flex; gap: 10px; color: var(--mute); font-size: 11px; float: right; }
.lsdm-chip-sm { color: var(--orange); background: oklch(66% .19 40 / var(--tint)); border-radius: 99px; padding: 0 8px; font-weight: 500; }
.lsdm-exchange + .lsdm-exchange { margin-top: 6px; }
/* only the command that failed is marked, not its whole pipeline */
.lsdm-exchange-err { margin-left: -8px; padding: 2px 8px; border-radius: 8px; background: linear-gradient(100deg, oklch(62% .2 25 / var(--tint)), transparent 80%); }
.lsdm-cmdline { color: var(--orange); font-weight: 700; white-space: pre-wrap; word-break: break-all; }
.lsdm-reply { white-space: pre; overflow-x: auto; padding-left: 14px; }
.lsdm-r-ok { color: var(--green); font-weight: 700; }
.lsdm-r-err { color: var(--red); white-space: pre-wrap; }
.lsdm-r-type, .lsdm-r-idx { color: var(--mute); }
.lsdm-r-num { color: var(--num); }
.lsdm-r-str { color: var(--str); }
.lsdm-r-key { color: var(--key); }
.lsdm-r-nil { color: var(--mute); font-style: italic; }
.lsdm-r-text { color: var(--text); }
/* API tree: categories with a chevron and count, calls as NAME args rows */
.lsdm-save { display: flex; gap: 6px; margin-bottom: 8px; }
.lsdm-save input { flex: 1; min-width: 0; }
/* Save row and filter stay put; only the tree scrolls (the History list keeps the side body's own scrolling) */
.lsdm-side-body:has(> .lsdm-tree-wrap) { display: flex; flex-direction: column; overflow: hidden; }
.lsdm-tree-wrap { display: flex; flex-direction: column; gap: 6px; flex: 1; min-height: 0; }
.lsdm-tree-wrap > .lsdm-tree { flex: 1; min-height: 0; overflow: auto; }
.lsdm-tree-filter { width: 100%; box-sizing: border-box; }
.lsdm-tree { outline: none; display: flex; flex-direction: column; gap: 1px; }
.lsdm-tree:focus-visible { box-shadow: 0 0 0 2px oklch(66% .19 40 / .45); border-radius: 9px; }
.lsdm-cat-row {
  display: flex; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 9px; cursor: pointer;
  font: 500 12.5px var(--ui); color: var(--text); user-select: none;
}
.lsdm-cat-row:hover { background: oklch(60% .03 330 / .08); }
.lsdm-chev { width: 6px; height: 6px; border: solid var(--mute); border-width: 0 2px 2px 0; transform: rotate(-45deg); transition: transform .15s; margin: 0 3px 0 1px; flex-shrink: 0; }
.lsdm-chev-open { transform: rotate(45deg); border-color: var(--orange); }
.lsdm-cat-label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.lsdm-cat-saved .lsdm-cat-label::after { content: " ★"; color: var(--orange); font-size: 10px; }
.lsdm-cat-count { font: 700 10px var(--ui); color: var(--soft); background: var(--s0); box-shadow: var(--in); border-radius: 99px; padding: 1px 7px; }
.lsdm-call {
  display: flex; align-items: center; gap: 8px; padding: 4px 8px 4px 26px; border-radius: 8px; cursor: pointer; color: var(--soft);
  position: relative;
}
.lsdm-call::before { content: ""; position: absolute; left: 13px; top: 0; bottom: 0; width: 1px; background: var(--line); }
.lsdm-call:hover { background: oklch(60% .03 330 / .08); color: var(--text); }
.lsdm-call .lsdm-item-name { font: 12px var(--mono); color: var(--mute); }
.lsdm-call .lsdm-x { opacity: 0; padding: 0 5px !important; } .lsdm-call:hover .lsdm-x { opacity: 1; }
.lsdm-call .lsdm-cmd-name { min-width: 64px; }
.lsdm-cat-row.lsdm-on, .lsdm-call.lsdm-on {
  color: var(--text); background: linear-gradient(100deg, oklch(66% .19 40 / var(--tint)), oklch(66% .19 40 / .03));
  box-shadow: inset 3px 0 0 var(--orange);
}
.lsdm-tree-note { color: var(--mute); font-size: 12px; padding: 8px 8px 4px; }
/* DevApp crash fallback: shown in the app's place (not in the dock), lit top-left like the panel */
.lsdm-crash { display: grid; place-items: center; min-height: 100%; padding: 32px 20px; }
.lsdm-crash-card {
  width: min(760px, 100%); border-radius: 14px; overflow: hidden; padding: 0 0 16px;
  background: linear-gradient(135deg, var(--s3), var(--s2) 50%, var(--s1)); box-shadow: var(--sh-lg), var(--hi);
}
.lsdm-crash-title {
  font: 600 15px/1 var(--ui); color: oklch(97% .01 330); padding: 16px 20px; background: var(--title);
  box-shadow: inset 0 -3px 0 var(--red);
}
.lsdm-crash-msg { margin: 16px 20px 8px; color: var(--red); white-space: pre-wrap; font-weight: 600; }
.lsdm-crash-stack {
  margin: 0 20px 14px; padding: 10px 12px; max-height: 240px; overflow: auto; color: var(--soft); font-size: 12.5px;
  background: var(--s0); border-radius: 9px; box-shadow: var(--in); white-space: pre;
}
.lsdm-crash-retry { margin-left: 20px; }
`;
