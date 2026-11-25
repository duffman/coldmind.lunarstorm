#!/usr/bin/env node
// Drives a page in headless system Chrome over the DevTools protocol: no npm
// dependencies (Node >= 22 has a global WebSocket), so it runs from a clean checkout.
//
//   node .claude/skills/run-lunarstorm/driver.mjs <step> [args] <step> [args] ...
//
// Steps, run in order; the first failing step exits 1 with its message:
//   open <url>                load the page and wait for `load`
//   type <selector> <text>    focus the element and insert text as real key input
//   click <selector|text=X>   mouse click at the centre of the element
//   wait <selector> <text>    poll (5 s) until the element's text or value contains <text>
//   text <selector>           print the element's text (or value)
//   eval <js>                 evaluate an expression in the page and print the result
//   shot <file.png>           screenshot of the viewport
//
// Env: CHROME (browser binary; default the macOS Google Chrome), DRIVER_SIZE (WxH, 1100x720).
//
// Decisions:
// - Real input (`Input.insertText`, `Input.dispatchMouseEvent`) rather than synthetic DOM
//   events, so the page's own handlers (`oninput`, `onclick`) fire as for a user.
// - The profile lives in <repo>/.scratch/chrome-driver, never in /tmp; it is reset per run.
// - Console messages and uncaught exceptions of the page are printed as `[page] ...`, so
//   a backend error in the Worker is visible in the driver's output.

import { spawn } from 'node:child_process';
import { mkdirSync, readFileSync, rmSync, existsSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const chrome = process.env.CHROME ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const [width, height] = (process.env.DRIVER_SIZE ?? '1100x720').split('x').map(Number);
const profile = resolve(repo, '.scratch/chrome-driver');

/** Sleeps for `ms` milliseconds. @param {number} ms @returns {Promise<void>} */
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Starts Chrome and returns its process and the page's WebSocket URL.
 * @returns {Promise<{browser: import('node:child_process').ChildProcess, ws: string}>}
 */
async function launch() {
  if (!existsSync(chrome)) throw new Error(`no browser at ${chrome}; set CHROME`);
  rmSync(profile, { recursive: true, force: true });
  mkdirSync(profile, { recursive: true });
  const browser = spawn(chrome, [
    '--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`,
    `--window-size=${width},${height}`, '--no-first-run', '--no-default-browser-check', 'about:blank',
  ], { stdio: 'ignore' });
  const portFile = resolve(profile, 'DevToolsActivePort');
  for (let i = 0; i < 100 && !existsSync(portFile); i++) await sleep(100);
  if (!existsSync(portFile)) throw new Error('Chrome did not open a DevTools port');
  const port = readFileSync(portFile, 'utf8').split('\n')[0];
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const page = targets.find((t) => t.type === 'page');
  return { browser, ws: page.webSocketDebuggerUrl };
}

/**
 * Opens a DevTools session on a page.
 * @param {string} url the page's WebSocket debugger URL
 * @returns {Promise<{send: (method: string, params?: object) => Promise<any>, on: (event: string, f: Function) => void, close: () => void}>}
 */
async function connect(url) {
  const socket = new WebSocket(url);
  await new Promise((ok, fail) => { socket.onopen = ok; socket.onerror = () => fail(new Error('DevTools socket failed')); });
  let next = 0;
  const pending = new Map();
  const listeners = new Map();
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (message.id !== undefined) {
      const { ok, fail } = pending.get(message.id);
      pending.delete(message.id);
      message.error ? fail(new Error(message.error.message)) : ok(message.result);
    } else for (const f of listeners.get(message.method) ?? []) f(message.params);
  };
  return {
    send: (method, params = {}) => new Promise((ok, fail) => {
      const id = ++next;
      pending.set(id, { ok, fail });
      socket.send(JSON.stringify({ id, method, params }));
    }),
    on: (event, f) => listeners.set(event, [...(listeners.get(event) ?? []), f]),
    close: () => socket.close(),
  };
}

/**
 * The JS expression that finds an element: a CSS selector, or `text=X` for the first
 * button/link/element whose own text is exactly X.
 * @param {string} target selector or `text=X`
 * @returns {string}
 */
function finder(target) {
  if (target.startsWith('text=')) {
    const text = JSON.stringify(target.slice(5));
    return `[...document.querySelectorAll('button,a,li,label,div,span')].find((e) => e.textContent.trim() === ${text})`;
  }
  return `document.querySelector(${JSON.stringify(target)})`;
}

/** Runs the steps given on the command line. */
async function main() {
  const steps = process.argv.slice(2);
  if (steps.length === 0) {
    process.stderr.write('usage: driver.mjs open <url> [type <sel> <text>] [click <sel>] [wait <sel> <text>] [text <sel>] [eval <js>] [shot <file>]\n');
    process.exit(2);
  }
  const { browser, ws } = await launch();
  const cdp = await connect(ws);
  let failed = false;
  try {
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
    cdp.on('Runtime.consoleAPICalled', (p) => console.log(`[page] ${p.type}: ${p.args.map((a) => a.value ?? a.description).join(' ')}`));
    cdp.on('Runtime.exceptionThrown', (p) => console.log(`[page] exception: ${p.exceptionDetails.exception?.description ?? p.exceptionDetails.text}`));
    /** Evaluates `expression` in the page. @param {string} expression @returns {Promise<any>} */
    const evaluate = async (expression) => {
      const { result, exceptionDetails } = await cdp.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
      if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
      return result.value;
    };
    for (let i = 0; i < steps.length;) {
      const step = steps[i++];
      if (step === 'open') {
        const url = steps[i++];
        const loaded = new Promise((r) => cdp.on('Page.loadEventFired', r));
        await cdp.send('Page.navigate', { url });
        await loaded;
        await sleep(300);
        console.log(`open ${url}`);
      } else if (step === 'type') {
        const [target, text] = [steps[i++], steps[i++]];
        const found = await evaluate(`(() => { const e = ${finder(target)}; if (!e) return false; e.focus(); return true; })()`);
        if (!found) throw new Error(`type: no element ${target}`);
        await cdp.send('Input.insertText', { text });
        console.log(`type ${target} ${JSON.stringify(text)}`);
      } else if (step === 'click') {
        const target = steps[i++];
        const box = await evaluate(`(() => { const e = ${finder(target)}; if (!e) return null; e.scrollIntoView({block: 'center'}); const r = e.getBoundingClientRect(); return {x: r.x + r.width / 2, y: r.y + r.height / 2}; })()`);
        if (!box) throw new Error(`click: no element ${target}`);
        for (const type of ['mousePressed', 'mouseReleased']) {
          await cdp.send('Input.dispatchMouseEvent', { type, x: box.x, y: box.y, button: 'left', clickCount: 1 });
        }
        console.log(`click ${target}`);
      } else if (step === 'wait' || step === 'text') {
        const target = steps[i++];
        const want = step === 'wait' ? steps[i++] : '';
        const read = `(() => { const e = ${finder(target)}; return e ? (e.value || e.textContent) : null; })()`;
        let seen = null;
        for (let t = 0; t < 50; t++) {
          seen = await evaluate(read);
          if (seen !== null && seen.includes(want)) break;
          await sleep(100);
        }
        if (seen === null || !seen.includes(want)) throw new Error(`${step}: ${target} is ${JSON.stringify(seen)}, wanted ${JSON.stringify(want)}`);
        console.log(`${step} ${target} = ${JSON.stringify(seen)}`);
      } else if (step === 'eval') {
        console.log(`eval = ${JSON.stringify(await evaluate(steps[i++]))}`);
      } else if (step === 'shot') {
        const file = resolve(steps[i++]);
        const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' });
        mkdirSync(dirname(file), { recursive: true });
        writeFileSync(file, Buffer.from(data, 'base64'));
        console.log(`shot ${file}`);
      } else throw new Error(`unknown step ${step}`);
    }
  } catch (error) {
    console.error(`driver: ${error.message}`);
    failed = true;
  } finally {
    cdp.close();
    browser.kill();
  }
  process.exit(failed ? 1 : 0);
}

await main();
