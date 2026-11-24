import { GlobalRegistrator } from "@happy-dom/global-registrator";

// Preserve Node's real network implementation before installing the DOM shim.
export const realWebSocket = globalThis.WebSocket;
export const realFetch = globalThis.fetch;
const nodeEvents = {
  Event: globalThis.Event,
  EventTarget: globalThis.EventTarget,
  MessageEvent: globalThis.MessageEvent,
};
GlobalRegistrator.register({ url: "http://localhost/" });
// Undici dispatches through Node's EventTarget and requires Node Event instances.
// Happy DOM keeps its own internal DOM constructors; only the global network
// constructors are restored. Keyboard/pointer input remains Happy DOM input.
for (const [name, constructor] of Object.entries(nodeEvents)) {
  Object.defineProperty(globalThis, name, { value: constructor, writable: true, configurable: true });
}
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
