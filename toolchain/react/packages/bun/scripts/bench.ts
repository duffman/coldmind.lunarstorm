/**
 * Benchmark: Bun.build time with and without devermind, plus dev-server edit-to-served latency.
 *
 *   bun packages/tools/devermind/packages/bun/scripts/bench.ts [--modules 2000] [--runs 5] [--skip-serve]
 *
 * The fixture is generated once per module count into `bench/fixture-<modules>/` and reused.
 * Results print as a table and are written to `bench/results-<modules>.json`.
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import type { BunPlugin } from "bun";
import { devermind } from "@coldmind/devermind";

const { values } = parseArgs({
  options: {
    modules: { type: "string", default: "2000" },
    runs: { type: "string", default: "5" },
    "skip-serve": { type: "boolean", default: false },
  },
});
const modules = Number(values.modules);
const runs = Number(values.runs);
const benchRoot = resolve(import.meta.dir, "../bench");
const fixture = join(benchRoot, `fixture-${modules}`);
const define = { __MODE__: JSON.stringify("bench") };

/**
 * Refreshes bootstrap files on every invocation because their native-plugin import contains
 * the current checkout path. Cached module sources remain reusable after moving the product;
 * a completion marker only skips regenerating those path-independent benchmark modules.
 */
function ensureFixture(): void {
  mkdirSync(fixture, { recursive: true });
  writeFileSync(join(fixture, "index.html"), '<!doctype html><html><body><script type="module" src="./src/index.ts"></script></body></html>\n');
  writeFileSync(join(fixture, "server.ts"), [
    'import index from "./index.html";',
    "const server = Bun.serve({ port: 0, development: { hmr: true }, routes: { '/': index } });",
    "console.log(`url=${server.url}`);",
    "",
  ].join("\n"));
  writeFileSync(join(fixture, "bench.plugin.ts"), [
    `import { devermind } from ${JSON.stringify(resolve(import.meta.dir, "../src/bun-plugin.ts"))};`,
    `export default devermind({ define: ${JSON.stringify(define)}, inject: true });`,
    "",
  ].join("\n"));
  writeFileSync(join(fixture, "bunfig.toml"), '[serve.static]\nplugins = ["./bench.plugin.ts"]\n');
  const marker = join(fixture, ".complete");
  if (existsSync(marker)) return;
  const started = performance.now();
  mkdirSync(join(fixture, "src"), { recursive: true });
  writeFileSync(join(fixture, "src/dep.ts"), "export class Dep {\n  readonly id = 1;\n}\n");
  const exports: string[] = [];
  for (let index = 0; index < modules; index += 1) {
    writeFileSync(join(fixture, `src/m${index}.ts`), [
      'import { Dep } from "./dep.ts";',
      "const mark = <T>(target: T, _context: ClassDecoratorContext): T => target;",
      `/** Module ${index}: a decorated class, a define, and some ordinary code. */`,
      "@mark",
      `export class Service${index} {`,
      "  constructor(readonly dep: Dep, readonly label?: Dep) {}",
      `  value(): string { return \`\${__MODE__}:${index}:\${this.dep.id}\`; }`,
      "}",
      `export const table${index} = Array.from({ length: 16 }, (_, i) => ({ i, label: "row" + i }));`,
      "",
    ].join("\n"));
    exports.push(`export * from "./m${index}.ts";`);
  }
  writeFileSync(join(fixture, "src/index.ts"), `${exports.join("\n")}\n`);
  writeFileSync(marker, `${modules}\n`);
  console.log(`fixture: ${modules} modules written to ${fixture} in ${Math.round(performance.now() - started)} ms`);
}

/** Same work as the define stage, as a JS `onLoad` plugin (text replace, no scope analysis). */
const jsReplace: BunPlugin = {
  name: "js-replace",
  setup(build) {
    build.onLoad({ filter: /\.ts$/ }, async ({ path }) => ({
      contents: (await Bun.file(path).text()).replaceAll("__MODE__", define.__MODE__),
      loader: "ts",
    }));
  },
};

interface Variant {
  name: string;
  plugins: () => BunPlugin[];
  define?: Record<string, string>;
}

const variants: Variant[] = [
  { name: "bun (define option, no plugin)", plugins: () => [], define },
  { name: "devermind define", plugins: () => [devermind({ define })] },
  { name: "devermind define + inject", plugins: () => [devermind({ define, inject: true })] },
  { name: "js onLoad replace", plugins: () => [jsReplace] },
];

function summarize(samples: number[]): { median: number; min: number; max: number } {
  const sorted = [...samples].sort((a, b) => a - b);
  const round = (value: number) => Math.round(value * 10) / 10;
  return { median: round(sorted[Math.floor(sorted.length / 2)]!), min: round(sorted[0]!), max: round(sorted.at(-1)!) };
}

/** Measures equivalent builds after warming each variant; fresh plugin state keeps counters per run. */
async function benchBuilds() {
  const rows = [];
  for (const variant of variants) {
    const samples: number[] = [];
    for (let run = 0; run <= runs; run += 1) {
      const started = performance.now();
      const result = await Bun.build({
        entrypoints: [join(fixture, "src/index.ts")],
        plugins: variant.plugins(),
        ...(variant.define ? { define: variant.define } : {}),
      });
      const elapsed = performance.now() - started;
      if (!result.success) throw new AggregateError(result.logs, `${variant.name} failed`);
      if (run > 0) samples.push(elapsed); // run 0 warms the filesystem cache
    }
    rows.push({ variant: variant.name, ...summarize(samples) });
  }
  return rows;
}

/** Reads the child dev server announcement before issuing requests; no fixed port is assumed. */
async function readUrl(stream: ReadableStream<Uint8Array>): Promise<string> {
  const decoder = new TextDecoder();
  let output = "";
  for await (const chunk of stream) {
    output += decoder.decode(chunk, { stream: true });
    const match = output.match(/url=(\S+)/);
    if (match) return match[1]!;
  }
  throw new Error(`dev server exited:\n${output}`);
}

/**
 * Starts a fixture dev server, edits its dependency and polls for served replacement content.
 * The measurement includes watch/rebuild/serve latency, not just compiler execution time.
 */
async function benchServe() {
  const server = Bun.spawn([process.execPath, "server.ts"], { cwd: fixture, stdout: "pipe", stderr: "inherit" });
  const leaf = join(fixture, "src/dep.ts");
  const original = readFileSync(leaf, "utf8");
  try {
    const url = await readUrl(server.stdout);
    const scriptUrl = async () => {
      const html = await (await fetch(url)).text();
      return new URL(html.match(/<script[^>]+src="([^"]+)"/)![1]!, url);
    };
    const coldStarted = performance.now();
    await (await fetch(await scriptUrl())).text();
    const cold = performance.now() - coldStarted;
    const samples: number[] = [];
    for (let run = 1; run <= runs; run += 1) {
      // Bundles drop comments: the edit renames a field so the marker survives into served code.
      const marker = `edit_${run}_${Date.now()}`;
      const started = performance.now();
      writeFileSync(leaf, `export class Dep {\n  readonly ${marker} = 1;\n  readonly id = 1;\n}\n`);
      for (;;) {
        const code = await (await fetch(await scriptUrl())).text();
        if (code.includes(marker)) break;
        if (performance.now() - started > 10_000) throw new Error("edit not served within 10 s");
        await Bun.sleep(5);
      }
      samples.push(performance.now() - started);
    }
    return { coldFirstRequest: Math.round(cold), editToServed: summarize(samples) };
  } finally {
    writeFileSync(leaf, original);
    server.kill();
    await server.exited;
  }
}

ensureFixture();
console.log(`Bun ${Bun.version}, ${process.platform}-${process.arch}, ${modules} modules, ${runs} runs (+1 warm-up)`);
const builds = await benchBuilds();
console.table(builds);
const serve = values["skip-serve"] ? undefined : await benchServe();
if (serve) console.log("dev server (devermind define + inject):", serve);
const report = { bun: Bun.version, platform: `${process.platform}-${process.arch}`, modules, runs, builds, serve };
writeFileSync(join(benchRoot, `results-${modules}.json`), `${JSON.stringify(report, null, 2)}\n`);
console.log(`results: ${join(benchRoot, `results-${modules}.json`)}`);
