/**
 * `@lunarstorm/devmode`: a developer console and a Postman-style API executor, mounted as a dev mode in a React app.
 *
 * ```tsx
 * import { DevMode, installCapture } from "@lunarstorm/devmode";
 * if (import.meta.env.DEV) installCapture(); // optional: capture logs from before the first render
 * root.render(<><App /><DevMode enabled={import.meta.env.DEV} env={{ baseUrl: "http://localhost:3000" }} /></>);
 * ```
 */
export { DevMode, matchesHotkey, requestFromNet, type DevModeProps } from "./devmode.tsx";
export { capture, Capture, installCapture, MAX_ENTRIES, type EntryType, type LogEntry, type NetRecord } from "./capture.ts";
export { evaluate } from "./console-tab.tsx";
export {
  buildRequest, envVars, execute, interpolate, METHODS, newRequest, parseCurl, splitQuery, toCurl,
  type ApiRequest, type ApiResponse, type Auth, type BodyMode, type BuiltRequest, type Method, type Pair,
} from "./request.ts";
