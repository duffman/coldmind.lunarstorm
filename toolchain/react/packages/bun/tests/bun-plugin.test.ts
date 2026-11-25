import { afterAll, describe, expect, test } from "vitest";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { devtools, loadNative, nativeFileName, type NativeState } from "@coldmind/devtools";

const fixtures = join(import.meta.dir, "fixtures");
const scratch = mkdtempSync(join(tmpdir(), "devtools-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

const zero = { files: 0, changed: 0, parsed: 0, replaced: 0, defined: 0, injected: 0 };

/** Builds `entry` for Bun into the scratch directory and imports the bundle. */
async function buildAndImport(entry: string, plugin: ReturnType<typeof devtools>, name: string) {
  const result = await Bun.build({ entrypoints: [entry], outdir: join(scratch, name), target: "bun", plugins: [plugin] });
  expect(result.success).toBe(true);
  return import(result.outputs[0]!.path);
}

describe("native library", () => {
  test("names the library per platform", () => {
    expect(nativeFileName("darwin", "arm64")).toBe("devtools.darwin-arm64.node");
    expect(nativeFileName()).toBe(`devtools.${process.platform}-${process.arch}.node`);
  });

  test("rejects invalid configuration", () => {
    expect(() => devtools({ replace: [{ from: "", to: "x" }] })).toThrow("must not be empty");
    expect(() => devtools({ define: { "a-b": "1" } })).toThrow("dotted identifier path");
    expect(() => devtools({ define: { a: "1 +" } })).toThrow("not a JS expression");
  });

  test("rejects values that are not its own state", () => {
    const native = loadNative();
    expect(() => native.stats({} as NativeState)).toThrow();
    expect(() => native.stats(undefined as unknown as NativeState)).toThrow();
    expect(native.stats(native.createState({}))).toEqual(zero);
  });

  test("fails the build when the hook gets no state", async () => {
    const napiModule = loadNative();
    const result = await Bun.build({
      entrypoints: [join(fixtures, "build/leaf.ts")],
      throw: false,
      plugins: [{
        name: "no-state",
        setup(build) {
          build.onBeforeParse({ filter: /\.ts$/ }, { napiModule, symbol: "devtools_transform" });
        },
      }],
    });
    expect(result.success).toBe(false);
    expect(result.logs.map(String).join("\n")).toContain("needs the `external`");
  });
});

describe("replace", () => {
  test("runs on every module without parsing and keeps the TS loader", async () => {
    const plugin = devtools({ replace: [{ from: "__MODE__", to: JSON.stringify("dev") }] });
    const result = await Bun.build({ entrypoints: [join(fixtures, "build/entry.ts")], plugins: [plugin] });
    expect(result.success).toBe(true);
    const code = await result.outputs[0]!.text();
    expect(code).not.toContain("__MODE__");
    expect(code).toContain('"dev"');
    expect(plugin.stats()).toEqual({ ...zero, files: 2, changed: 2, replaced: 3 });
  });

  test("counts exactly across 200 modules transformed in parallel", async () => {
    const directory = join(scratch, "many");
    const count = 200;
    await Promise.all(Array.from({ length: count }, (_, index) =>
      Bun.write(join(directory, `m${index}.ts`), `export const v${index}: string = __MODE__;\n`)));
    const imports = Array.from({ length: count }, (_, index) => `export * from "./m${index}.ts";`);
    await Bun.write(join(directory, "index.ts"), `${imports.join("\n")}\n`);
    const plugin = devtools({ define: { __MODE__: JSON.stringify("dev") } });
    const result = await Bun.build({ entrypoints: [join(directory, "index.ts")], plugins: [plugin] });
    expect(result.success).toBe(true);
    expect(await result.outputs[0]!.text()).not.toContain("__MODE__");
    expect(plugin.stats()).toEqual({ ...zero, files: count + 1, changed: count, parsed: count, defined: count });
  });
});

describe("define", () => {
  test("replaces only real global references", async () => {
    const plugin = devtools({
      define: { __MODE__: JSON.stringify("dev"), "process.env.NODE_ENV": JSON.stringify("production") },
    });
    const { result } = await buildAndImport(join(fixtures, "define/entry.ts"), plugin, "define");
    expect(result).toEqual({
      mode: "dev",
      label: "__MODE__ in a string stays",
      local: "param",
      env: "production",
      shorthand: { __MODE__: "dev" },
    });
    expect(plugin.stats()).toEqual({ ...zero, files: 1, changed: 1, parsed: 1, defined: 3 });
  });

  test("does not parse files that cannot match", async () => {
    const plugin = devtools({ define: { __ABSENT__: "1" } });
    const result = await Bun.build({ entrypoints: [join(fixtures, "build/leaf.ts")], plugins: [plugin] });
    expect(result.success).toBe(true);
    expect(plugin.stats()).toEqual({ ...zero, files: 1 });
  });
});

describe("inject", () => {
  // The runtime wiring test (fixture app.ts, needs @coldmind/inject) is archived in
  // .archive/2026-11-25/toolchain/react/packages/bun/tests: the package was deleted in efd5ece.
  test("leaves classes it cannot resolve and reports the parameter at build end", async () => {
    const warnings: string[] = [];
    const plugin = devtools({ inject: true, onWarning: message => warnings.push(message) });
    const result = await Bun.build({ entrypoints: [join(fixtures, "inject/unresolved.ts")], plugins: [plugin] });
    expect(result.success).toBe(true);
    expect(await result.outputs[0]!.text()).not.toContain("$inject");
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toContain("unresolved.ts: devtools inject: Typed not wired");
    expect(warnings[0]).toContain("`clock`");
    expect(warnings[0]).toContain("type-only import");
    expect(plugin.takeWarnings()).toEqual([]);
  });
});

describe("stages together", () => {
  test("replace, define and inject apply to the same file in one hook", async () => {
    const entry = join(scratch, "chain/entry.ts");
    await Bun.write(join(scratch, "chain/dep.ts"), "export class Dep {}\n");
    await Bun.write(entry, [
      'import { Dep } from "./dep.ts";',
      "export class Uses { constructor(readonly dep: Dep) {} }",
      "export const mode = __NAME__;",
      "",
    ].join("\n"));
    const plugin = devtools({
      replace: [{ from: "__NAME__", to: "__MODE__" }],
      define: { __MODE__: JSON.stringify("chained") },
      inject: { all: true },
    });
    const module = await buildAndImport(entry, plugin, "chain/out");
    expect(module.mode).toBe("chained");
    expect(module.Uses.$inject).toHaveLength(1);
    expect(plugin.stats()).toMatchObject({ replaced: 1, defined: 1, injected: 1 });
  });
});

describe("Bun.serve dev server", () => {
  test("runs the oxc define stage for HTML-imported modules with HMR on", async () => {
    const server = Bun.spawn([process.execPath, "server.ts"], {
      cwd: join(fixtures, "serve"),
      stdout: "pipe",
      stderr: "pipe",
    });
    try {
      const url = await readUrl(server.stdout);
      const html = await (await fetch(url)).text();
      const script = html.match(/<script[^>]+src="([^"]+)"/)?.[1];
      expect(script).toBeTypeOf("string");
      const code = await (await fetch(new URL(script!, url))).text();
      expect(code).not.toContain("__MODE__");
      expect(code).toContain('"served"');
    } finally {
      server.kill();
      await server.exited;
    }
  }, 20_000);
});

/** Reads the child's stdout until it prints `url=<address>`. */
async function readUrl(stream: ReadableStream<Uint8Array>): Promise<string> {
  const decoder = new TextDecoder();
  let output = "";
  for await (const chunk of stream) {
    output += decoder.decode(chunk, { stream: true });
    const match = output.match(/url=(\S+)/);
    if (match) return match[1]!;
  }
  throw new Error(`dev server exited before printing its url:\n${output}`);
}


test("legacy plugin source export resolves to the canonical implementation", async () => {
  const legacy = await import("@coldmind/devtools/src/devtools.plugin.ts");
  const canonical = await import("@coldmind/devtools");
  expect(legacy.devtools).toBe(canonical.devtools);
});
