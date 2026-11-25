/**
 * Dev mode: a developer console and a redis-cli style executor for the app's Redis-protocol API, docked in a React app.
 *
 * ```tsx
 * import { DevApp, installCapture } from "./devmode";
 * if (import.meta.env.DEV) installCapture(); // optional: capture logs from before the first render
 * root.render(<DevApp enabled={import.meta.env.DEV}><App /></DevApp>); // error boundary + docked dev mode
 * ```
 */
export { DevMode, matchesHotkey, type DevModeProps } from "./devmode.tsx";
export { DevApp, type DevAppProps } from "./devapp.tsx";
export { capture, Capture, installCapture, MAX_ENTRIES, type EntryType, type LogEntry, type NetRecord } from "./capture.ts";
export { evaluate } from "./console-tab.tsx";
export { RedisClient, type ClientOptions, type ConnState, type ServerInfo } from "./redis-client.ts";
export { defaultEndpoint, IPC_VERSION, ipcTransport, transportFor, webSocketTransport, type IpcEnvelope, type RespTransport, type TransportEvents } from "./transport.ts";
export { encodeCommand, encodeReply, formatReply, parseCommands, quote, replyText, RespDecoder, RespProtocolError, tokenize, type Line, type Reply, type Seg } from "./resp.ts";
export { commandText } from "./redis-tab.tsx";
export { ApiTree, parseApiSpec, parseCommandDocs, savedCategories, type ApiCall, type ApiCategory, type ApiSpec, type ApiTreeProps, type SavedCall } from "./api-tree.tsx";
