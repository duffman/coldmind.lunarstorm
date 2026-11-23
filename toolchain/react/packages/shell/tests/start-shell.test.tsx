import { afterEach, beforeEach, expect, test, vi, type MockInstance } from "vitest";
import { act, useState } from "react";

import { createHotContext, resetHotRuntime } from "@coldmind/devtools-shell/hot";
import { startShell, useShellChannel, useShellStatus, type AppModule, type ShellHandle } from "@coldmind/devtools-shell";
import { FakeSocket, fakeWebSocket, tick } from "./fake-socket.ts";

let shell: ShellHandle | null = null;
let reloads = 0;
let consoleError: MockInstance<typeof console.error>;

beforeEach(() => {
  FakeSocket.reset();
  reloads = 0;
  document.body.innerHTML = "";
  consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(async () => {
  await act(async () => shell?.stop());
  shell = null;
  resetHotRuntime();
  consoleError.mockRestore();
});

async function start(load: () => Promise<AppModule>, modules: Record<string, unknown> = {}) {
  const root = document.createElement("div");
  document.body.appendChild(root);
  await act(async () => {
    shell = startShell({
      root,
      load,
      transport: {
        url: "ws://localhost:5173/__devtools_hmr",
        WebSocket: fakeWebSocket,
        fetch: () => new Promise(() => {}),
        reload: () => reloads++,
      },
      reload: () => reloads++,
      importModule: async (url) => modules[url.split("?")[0]!],
    });
  });
  const socket = FakeSocket.last();
  await act(async () => socket.open());
  return { root, socket };
}

const overlayText = () => document.querySelector("[data-devtools-shell]")?.shadowRoot?.textContent ?? "";
const frame = async (socket: FakeSocket, value: unknown) => {
  await act(async () => {
    socket.frame(value);
    await tick();
  });
};

test("renders the app and opens exactly one socket", async () => {
  const { root } = await start(async () => ({ default: () => <p>hello</p> }));
  expect(root.textContent).toBe("hello");
  expect(FakeSocket.all.length).toBe(1);
  expect(overlayText()).toBe("");
});

test("app channels share the shell socket", async () => {
  function Chat() {
    const [log, setLog] = useState<string[]>([]);
    const send = useShellChannel<string>("chat", (text) => setLog((l) => [...l, text]));
    const status = useShellStatus();
    return (
      <button type="button" onClick={() => send("ping")}>
        {status}:{log.join(",")}
      </button>
    );
  }
  const { root, socket } = await start(async () => ({ default: Chat }));
  await frame(socket, { type: "custom", event: "app:chat", data: "a" });
  await frame(socket, { type: "custom", event: "app:other", data: "x" });
  expect(root.textContent).toBe("open:a");
  await act(async () => root.querySelector("button")!.click());
  expect(socket.sent).toEqual([{ type: "custom", event: "app:chat", data: "ping" }]);
});

test("a render error shows the overlay and a hot update re-mounts the app", async () => {
  let broken = true;
  function App() {
    if (broken) throw new Error("render exploded");
    return <p>fixed</p>;
  }
  // The refresh boundary the server would inject for /src/App.tsx.
  createHotContext("/src/App.tsx").accept(() => {
    broken = false;
  });
  const { root, socket } = await start(async () => ({ default: App }), { "/src/App.tsx": {} });
  expect(root.textContent).toBe("");
  expect(overlayText()).toContain("Render error");
  expect(overlayText()).toContain("render exploded");

  await frame(socket, { type: "update", updates: [{ type: "js-update", path: "/src/App.tsx", timestamp: 2 }] });
  expect(root.textContent).toBe("fixed");
  expect(shell!.errors.list()).toEqual([]);
  expect(overlayText()).toBe("");
  expect(reloads).toBe(0);
});

test("a server error frame shows the build error until the next update", async () => {
  createHotContext("/src/App.tsx").accept();
  const { socket } = await start(async () => ({ default: () => <p>ok</p> }), { "/src/App.tsx": {} });
  await frame(socket, { type: "error", err: { message: "Unexpected token", frame: "1 | export const = ;", loc: { file: "/src/x.ts", line: 1 } } });
  expect(overlayText()).toContain("Build error");
  expect(overlayText()).toContain("/src/x.ts:1");
  await frame(socket, { type: "update", updates: [{ type: "js-update", path: "/src/App.tsx", timestamp: 3 }] });
  expect(overlayText()).toBe("");
});

test("an app that failed to load reloads on the next update", async () => {
  const { root, socket } = await start(() => Promise.reject(new Error("GET /src/Broken.tsx 500")));
  expect(root.textContent).toBe("");
  expect(overlayText()).toContain("Failed to load the app");
  await frame(socket, { type: "update", updates: [{ type: "js-update", path: "/src/App.tsx", timestamp: 4 }] });
  expect(reloads).toBe(1);
});

test("an entry without a default component is a load error", async () => {
  await start(async () => ({}) as AppModule);
  expect(overlayText()).toContain("no default export component");
});

test("uncaught errors are listed without replacing the app", async () => {
  const { root } = await start(async () => ({ default: () => <p>app</p> }));
  await act(async () => {
    window.dispatchEvent(new ErrorEvent("error", { error: new Error("late failure") }));
  });
  expect(root.textContent).toBe("app");
  expect(overlayText()).toBe("1 error");
  await act(async () => (document.querySelector("[data-devtools-shell]")!.shadowRoot!.querySelector("button") as HTMLButtonElement).click());
  expect(overlayText()).toContain("late failure");
});

test("a lost connection shows the restart badge", async () => {
  const { socket } = await start(async () => ({ default: () => <p>app</p> }));
  await act(async () => socket.close());
  expect(overlayText()).toContain("waiting for restart");
});

test("stop unmounts the app and the overlay and closes the socket", async () => {
  const { root, socket } = await start(async () => ({ default: () => <p>app</p> }));
  await act(async () => shell!.stop());
  shell = null;
  expect(root.textContent).toBe("");
  expect(document.querySelector("[data-devtools-shell]")).toBeNull();
  expect(socket.closed).toBe(true);
});


test("legacy source exports share the canonical shell and hot runtime", async () => {
  const legacyShell = await import("@coldmind/devtools-shell/src/shell.tsx");
  const legacyHot = await import("@coldmind/devtools-shell/src/shell.hot.ts");
  expect(legacyShell.startShell).toBe(startShell);
  expect(legacyHot.createHotContext).toBe(createHotContext);
});
