/**
 * Runs the product's cross-runtime contract checks without silently skipping a host.
 *
 * The CLI is the `lunarstorm` binary (platform/cli/app), whose `serve`, `bundle`, `preview` and
 * `transpile` commands come from devtools-cli. CLI artifacts come from Cargo's build messages, not an assumed global target path.
 * Each phase has a deadline and its own process group; interruption or failure reaps
 * that phase's descendants. Installed project Tailwind dependencies are a prerequisite,
 * never downloaded implicitly or replaced with a mock.
 */
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";

const root = resolve(import.meta.dir, "..");
const cargoHome = join(homedir(), ".cargo", "bin");
const environment = { ...process.env, PATH: [existsSync(join(cargoHome, "cargo")) ? cargoHome : "", process.env.PATH].filter(Boolean).join(delimiter) };
const reportPath = resolve(process.env.DEVTOOLS_TEST_REPORT ?? join(root, "test-results/integration.json"));
const timeout = Number(process.env.DEVTOOLS_TEST_TIMEOUT_MS ?? 1_200_000);
if (!Number.isFinite(timeout) || timeout <= 0) throw new Error("DEVTOOLS_TEST_TIMEOUT_MS must be a positive number");
const phases: Array<{ name: string; passed: boolean; elapsedMs: number }> = [];
const startedAt = new Date().toISOString();
let activePid: number | undefined;
let cli: string | undefined;
let failure: string | undefined;
let completed = false;

/** Signals only the process group created for the currently owned test phase. */
function terminate(signal: NodeJS.Signals): void {
  if (activePid === undefined) return;
  try { process.kill(process.platform === "win32" ? activePid : -activePid, signal); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error; }
}

/** Saves partial results too, so a failed or interrupted run cannot look successful. */
function saveReport(): void {
  mkdirSync(dirname(reportPath), { recursive: true });
  writeFileSync(reportPath, JSON.stringify({ startedAt, finishedAt: new Date().toISOString(), status: failure ? "failed" : completed ? "passed" : "running", passed: completed && !failure, cli, phases, failure }, null, 2) + "\n");
}

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    failure = `Interrupted by ${signal}`;
    terminate("SIGKILL");
    saveReport();
    process.exit(signal === "SIGINT" ? 130 : 143);
  });
}

/** Streams test output while retaining diagnostics and checking the exit status. */
async function run(name: string, command: string[], env: Record<string, string | undefined> = {}, capture = false, minimumRustTests = 0): Promise<string> {
  console.log(`\n=== ${name} ===`);
  const started = performance.now();
  const child = Bun.spawn(command, { cwd: root, env: { ...environment, ...env }, stdin: "ignore", stdout: "pipe", stderr: "inherit", detached: process.platform !== "win32" });
  activePid = child.pid;
  const output = (async () => {
    const decoder = new TextDecoder();
    let text = "";
    for await (const chunk of child.stdout) {
      text += decoder.decode(chunk, { stream: true });
      if (!capture) process.stdout.write(chunk);
    }
    return text + decoder.decode();
  })();
  let expired = false;
  let force: ReturnType<typeof setTimeout> | undefined;
  const timer = setTimeout(() => {
    expired = true; terminate("SIGTERM");
    force = setTimeout(() => terminate("SIGKILL"), 2000);
  }, timeout);
  try {
    const code = await child.exited;
    if (expired) terminate("SIGKILL");
    const text = await output;
    const executed = [...text.matchAll(/test result: ok\. (\d+) passed/g)].reduce((total, match) => total + Number(match[1]), 0);
    const passed = code === 0 && !expired && executed >= minimumRustTests;
    phases.push({ name, passed, elapsedMs: Math.round(performance.now() - started) });
    saveReport();
    if (!passed) throw new Error(`${name} ${expired ? "timed out" : `exited ${code} (${executed} Rust tests; expected at least ${minimumRustTests})`}\n${text.slice(-4000)}`);
    return text;
  } finally {
    // Reap owned descendants even when the phase parent has already failed or exited.
    terminate("SIGKILL");
    clearTimeout(timer); if (force) clearTimeout(force); activePid = undefined;
  }
}

/** Requires real Tailwind/oxide packages; the explicit override is never silently ignored. */
function tailwindModules(): string {
  const explicit = process.env.DEVTOOLS_TAILWIND_NODE_MODULES;
  // Fix: Tailwind is now a devDependency of this package, so its own node_modules is
  // the default. The former fallbacks (`<repo>/node_modules` and
  // `<repo>/projects/winmind/app/node_modules`, from the pre-monorepo layout) never held
  // Tailwind here, so `pnpm test` always failed this prerequisite.
  const candidates = explicit ? [resolve(explicit)] : [join(root, "node_modules")];
  const modules = candidates.find((path) => ["@tailwindcss/node", "@tailwindcss/oxide", "tailwindcss"].every((name) => existsSync(join(path, name, "package.json"))));
  if (!modules) throw new Error("Real Tailwind integration requires installed @tailwindcss/node, @tailwindcss/oxide and tailwindcss. Set DEVTOOLS_TAILWIND_NODE_MODULES to the project's node_modules directory; no tests were silently skipped.");
  return modules;
}

saveReport();
try {
  if (process.platform === "win32") throw new Error("The current real Tailwind fixture requires Unix symlinks; this full integration runner does not claim Windows coverage.");
  const modules = tailwindModules();
  await run("Node integration prerequisites", ["node", "-e", "if (typeof fetch !== 'function' || typeof WebSocket !== 'function') throw Error('Node with native fetch and WebSocket is required (Node 22+).')"]);
  const artifacts = await run("Fresh unified CLI build", ["cargo", "build", "--locked", "-p", "lunarstorm", "--message-format=json"], {}, true);
  for (const line of artifacts.split("\n")) {
    if (!line.startsWith("{")) continue;
    const artifact = JSON.parse(line);
    if (artifact.reason === "compiler-artifact" && artifact.target?.name === "lunar" && artifact.target.kind.includes("bin") && artifact.executable) cli = artifact.executable;
  }
  if (!cli || !existsSync(cli)) throw new Error("Cargo did not report the unified CLI executable");
  await run("Rust workspace contracts", ["cargo", "test", "--locked", "--workspace"], {}, false, 1);
  await run("Actual CLI dev, build-to-preview and failure contracts", ["cargo", "test", "--locked", "-p", "lunarstorm", "--test", "frontend_commands"], {}, false, 9);
  await run("Standalone transpiler and CLI usability contracts", ["cargo", "test", "--locked", "-p", "lunarstorm", "--test", "frontend_transpile"], {}, false, 28);
  await run("Merged Tauri HTTP, IPC and permission contracts", ["cargo", "test", "--locked", "-p", "tauri-plugin-devtools", "--all-features"], {}, false, 14);
  await run("Real Tailwind CSS, asset and watcher integration", ["cargo", "test", "--locked", "-p", "devtools_server", "--test", "styles", "--", "--ignored"], { DEVTOOLS_TAILWIND_NODE_MODULES: modules }, false, 2);
  await run("Fresh native bindings and Bun/React adapter contracts", ["bun", "run", "test:adapters"]);
  await run("Rust server to React shell over actual HTTP/WebSocket", ["bun", "run", "--cwd", "packages/shell", "test:integration"], { DEVTOOLS_TEST_CLI: cli });
  completed = true;
  console.log(`\nAll ${phases.length} integration phases passed. Report: ${reportPath}`);
} catch (error) {
  failure = error instanceof Error ? error.message : String(error);
  console.error(`\nIntegration verification FAILED: ${failure}`);
  process.exitCode = 1;
} finally { saveReport(); }
