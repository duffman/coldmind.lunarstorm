/** Real Rust HTTP/WebSocket/file-watcher integration with the React shell.
 * Compiled updates are fetched and inspected, not executed as browser modules.
 */
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtemp, mkdir, writeFile, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { afterEach, beforeEach, expect, test } from "vitest";
import { act, useState } from "react";
import { startShell, useShellChannel, type ShellHandle } from "../../src/start-shell.tsx";
import { createHotContext, resetHotRuntime } from "../../src/hot-runtime.ts";
import { realWebSocket, realFetch } from "./setup.ts";

const cli = process.env.DEVERMIND_TEST_CLI;
if (!cli) throw new Error("DEVERMIND_TEST_CLI must name a freshly built devermind executable; run the product integration runner");
let shell: ShellHandle | undefined;
let host: Host | undefined;
const peers: WebSocket[] = [];

/** Bounded condition waits flush React work; no arbitrary readiness sleeps. */
async function until(predicate: () => boolean, description: string, timeout = 8000): Promise<void> {
  const deadline = Date.now() + timeout;
  await act(async () => {});
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error(`Timed out waiting for ${description}`);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 15)); });
  }
}

class Host {
  child?: ChildProcess;
  url = "";
  constructor(readonly root: string) {}
  /** Resolve readiness from CLI output and fail with captured diagnostics on exit/timeout. */
  async start(port = 0): Promise<void> {
    const child = spawn(cli!, ["dev", this.root, "--port", String(port), "--host", "127.0.0.1"], { stdio: ["ignore", "pipe", "pipe"] });
    this.child = child;
    await new Promise<void>((resolveReady, reject) => {
      let output = "";
      let settled = false;
      const timer = setTimeout(() => { child.kill("SIGKILL"); reject(new Error(`CLI startup timeout: ${output}`)); }, 20000);
      const finish = (error?: Error) => { if (settled) return; settled = true; clearTimeout(timer); error ? reject(error) : resolveReady(); };
      const onExit = (code: number | null) => finish(new Error(`CLI exited ${code}: ${output}`));
      const onError = (error: Error) => finish(error);
      child.on("exit", onExit); child.on("error", onError);
      child.stderr!.on("data", (chunk) => { output += chunk.toString(); });
      child.stdout!.on("data", (chunk) => {
        output += chunk.toString();
        const match = output.match(/devermind ready at (http:\/\/[^\s]+)/);
        if (match) { this.url = match[1]!; finish(); }
      });
    });
  }
  async stop(): Promise<void> {
    const child = this.child; this.child = undefined;
    if (!child || child.exitCode !== null || child.signalCode !== null) return;
    await new Promise<void>((done) => {
      const timer = setTimeout(() => child.kill("SIGKILL"), 3000);
      child.on("exit", () => { clearTimeout(timer); done(); }); child.kill("SIGINT");
    });
  }
}

beforeEach(async () => {
  resetHotRuntime(); document.body.innerHTML = ""; localStorage.clear();
  document.body.style.paddingBottom = "7px";
  const root = await mkdtemp(join(tmpdir(), "devermind-shell-wire-"));
  host = new Host(root);
  await mkdir(join(root, "src")); await mkdir(join(root, ".devermind"));
  await writeFile(join(root, "package.json"), '{"type":"module"}');
  await symlink(resolve(import.meta.dirname, "../../../../node_modules"), join(root, "node_modules"), "dir");
  await writeFile(join(root, ".devermind/config.toml"), "[server]\nport = 0\n");
  await writeFile(join(root, "index.html"), '<div id="root"></div><script type="module" src="/src/App.tsx"></script>');
  await writeFile(join(root, "src/App.tsx"), 'export function App() { return <p>initial</p>; }');
  await host.start();
});

afterEach(async () => {
  await act(async () => { shell?.stop(); }); shell = undefined;
  for (const socket of peers.splice(0)) socket.close();
  if (host) { await host.stop(); await rm(host.root, { recursive: true, force: true }); host = undefined; }
  resetHotRuntime();
});

async function peer(): Promise<WebSocket> {
  const socket = new realWebSocket(host!.url.replace(/^http/, "ws") + "__devermind_hmr");
  peers.push(socket); await until(() => socket.readyState === 1, "peer connection"); return socket;
}
const shadow = () => document.querySelector("[data-devermind-shell]")!.shadowRoot!;

async function mount(App: () => React.ReactNode, options: Partial<Parameters<typeof startShell>[0]> = {}) {
  const root = document.createElement("div"); document.body.appendChild(root);
  let connections = 0;
  const CountedSocket = class extends realWebSocket { constructor(url: string) { super(url); connections++; } };
  await act(async () => {
    shell = startShell({ root, load: async () => ({ default: App }), transport: {
      url: host!.url.replace(/^http/, "ws") + "__devermind_hmr", WebSocket: CountedSocket,
      fetch: realFetch, retryMs: 30, reload: () => {},
    }, ...options });
  });
  await until(() => shell!.transport.status === "open", "shell connection");
  return { root, connections: () => connections };
}

test("real server relays bidirectional app messages on the shell's single socket", async () => {
  function Chat() {
    const [text, setText] = useState(""); const send = useShellChannel<string>("chat", setText);
    return <button onClick={() => send("from-shell")}>{text || "waiting"}</button>;
  }
  const { root, connections } = await mount(Chat);
  const socket = await peer(); const messages: string[] = [];
  socket.addEventListener("message", (event) => messages.push(String(event.data)));
  await act(async () => { socket.send(JSON.stringify({ type: "custom", event: "app:chat", data: "from-peer" })); });
  await until(() => root.textContent === "from-peer", "channel message rendered");
  await act(async () => { root.querySelector("button")!.click(); });
  await until(() => messages.some((message) => message.includes("from-shell")), "shell message received by peer");
  expect(connections()).toBe(1);
  const page = await (await realFetch(host!.url)).text();
  expect(page).toContain("refresh-preamble.js"); expect(page).not.toContain('src="/@devermind/client.js"');
  const client = await (await realFetch(host!.url + "@devermind/client.js")).text();
  expect(client).toContain("hot-runtime.ts");
  const runtime = client.match(/export \* from "([^"]+)"/);
  expect(runtime, client).not.toBeNull();
  expect((await realFetch(new URL(runtime![1]!, host!.url))).status).toBe(200);
});

test("real file updates preserve React state and compile failures recover through the overlay", async () => {
  let label = "initial"; const imported: string[] = [];
  function Counter() { const [count, setCount] = useState(0); return <button onClick={() => setCount(count + 1)}>{label}:{count}</button>; }
  createHotContext("/src/App.tsx").accept();
  const original = await realFetch(host!.url + "src/App.tsx"); expect(original.status).toBe(200);
  const { root } = await mount(Counter, { importModule: async (url) => {
    const response = await realFetch(new URL(url, host!.url)); const source = await response.text();
    if (!response.ok) throw new Error(source);
    imported.push(source); label = source.includes("recovered") ? "recovered" : "updated"; return {};
  } });
  await act(async () => { root.querySelector("button")!.click(); });
  await writeFile(join(host!.root, "src/App.tsx"), 'export function App() { return <p>updated</p>; }');
  await until(() => root.textContent === "updated:1", "real HMR with state preserved");
  expect(imported.some((source) => source.includes("updated"))).toBe(true);
  await writeFile(join(host!.root, "src/App.tsx"), 'export function App( {');
  await until(() => shell!.errors.list().some((error) => error.kind === "server"), "compiler error over WebSocket");
  expect(shadow().textContent).toContain("Build error");
  await writeFile(join(host!.root, "src/App.tsx"), 'export function App() { return <p>recovered</p>; }');
  await until(() => root.textContent === "recovered:1" && shell!.errors.list().length === 0, "compile error recovery");
});

test("terminal uses the real app channel and restores layout on close and teardown", async () => {
  await mount(() => <p>app</p>); const socket = await peer();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(String(event.data));
    if (message.type === "custom" && message.event === "app:terminal") socket.send(JSON.stringify({ ...message, data: `reply:${message.data}` }));
  });
  await act(async () => { (shadow().querySelector('[aria-label="Open terminal"]') as HTMLButtonElement).click(); });
  expect(document.body.style.paddingBottom).not.toBe("7px");
  const input = shadow().querySelector("input")!;
  await act(async () => { input.value = "help"; input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); });
  await until(() => shadow().textContent!.includes("reply:help"), "terminal reply over real socket");
  await act(async () => { input.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true })); });
  expect(input.value).toBe("help");
  const edge = shadow().querySelector('[aria-label="Resize terminal"]')!;
  const previous = Number(edge.getAttribute("aria-valuenow"));
  await act(async () => { edge.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true })); });
  expect(Number(edge.getAttribute("aria-valuenow"))).toBeGreaterThan(previous);
  await act(async () => { input.value = "clear"; input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); });
  expect(shadow().querySelector('[role="log"]')!.textContent).toBe("");
  await act(async () => { input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); });
  expect(document.body.style.paddingBottom).toBe("7px");
  await act(async () => { (shadow().querySelector('[aria-label="Open terminal"]') as HTMLButtonElement).click(); });
  expect(document.body.style.paddingBottom).not.toBe("7px");
  await act(async () => { shell!.stop(); }); shell = undefined;
  expect(document.body.style.paddingBottom).toBe("7px");
  expect(document.documentElement.style.getPropertyValue("--devermind-terminal-height")).toBe("");
});

test("server loss reaches lost status and a real restart triggers the reload callback", async () => {
  let reloads = 0;
  await mount(() => <p>app</p>, { transport: {
    url: host!.url.replace(/^http/, "ws") + "__devermind_hmr", WebSocket: realWebSocket,
    fetch: realFetch, retryMs: 30, reload: () => { reloads++; },
  } });
  const port = Number(new URL(host!.url).port);
  await host!.stop(); await until(() => shell!.transport.status === "lost", "server loss");
  expect(reloads).toBe(0);
  await host!.start(port); await until(() => reloads === 1, "server restart reload");
});
