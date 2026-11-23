import { describe, expect, test } from "vitest";
import { createTauriDevermind, type TauriInvoke, type TransformRequest, type TransformResult } from "../src/tauri-client.ts";

const result: TransformResult = {
  code: "export const mode = 1;",
  changed: true,
  parsed: true,
  replaced: 0,
  defined: 1,
  injected: 0,
  warnings: [],
};

describe("Tauri client", () => {
  test("uses the plugin transform command with its exact typed request", async () => {
    const calls: { command: string; args?: Record<string, unknown> }[] = [];
    const invoke: TauriInvoke = async <T>(command: string, args?: Record<string, unknown>) => {
      calls.push({ command, args });
      return result as T;
    };
    const request: TransformRequest = {
      source: "export const mode = __MODE__;",
      loader: "ts",
      filename: "input.ts",
      options: { define: { __MODE__: "1" } },
    };
    expect(await createTauriDevermind(invoke).transform(request)).toBe(result);
    expect(calls).toEqual([{ command: "plugin:devermind|transform", args: { request } }]);
  });

  test("leaves omitted options and filename to the Rust defaults", async () => {
    let args: Record<string, unknown> | undefined;
    const invoke: TauriInvoke = async <T>(_command: string, value?: Record<string, unknown>) => {
      args = value;
      return result as T;
    };
    const request: TransformRequest = { source: "export {};", loader: "js" };
    await createTauriDevermind(invoke).transform(request);
    expect(args).toEqual({ request: { source: "export {};", loader: "js" } });
  });

  test("propagates command and permission failures without falling back to Bun", async () => {
    const error = new Error("transform command not allowed");
    const invoke: TauriInvoke = async () => { throw error; };
    await expect(createTauriDevermind(invoke).transform({ source: "", loader: "tsx" })).rejects.toBe(error);
  });
});
