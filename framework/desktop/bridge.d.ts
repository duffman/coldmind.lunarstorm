/**
 * The host's bridge module as the backend runtime sees it. The host (not a package)
 * provides `@desktop/bridge` at run time; this declaration is its only source of types
 * until lsapi owns the contract (`framework/ls-baselib/lsapi`).
 */
declare module '@desktop/bridge' {
  /** A message from the host: an event or request of one control, form or the application. */
  export interface HostMessage {
    /** The window id: a form's name, or `@application`. */
    form: string;
    /** The control, form or application the event belongs to. */
    target: string;
    /** The event name, e.g. `onClick`. */
    event: string;
    /** The event's payload; `props` holds control values the user changed. */
    payload?: { args?: unknown; props?: Record<string, unknown> } & Record<string, unknown>;
    /** Whether the host waits for an answer. */
    request: boolean;
  }

  /**
   * Sends a property write to a form's WebView.
   * @param form the owning form's name
   * @param control the control's name
   * @param prop the property
   * @param value the new value
   */
  export function setProperty(form: string, control: string, prop: string, value: unknown): void;

  /**
   * The control values carried over from the previous backend of the form.
   * @param form the form's name
   * @return control name to its carried values
   */
  export function restored(form: string): Record<string, Record<string, unknown>>;

  /**
   * Installs the handler of host messages.
   * @param handler called for every message; its result answers a request
   */
  export function serve(handler: (message: HostMessage) => unknown): void;
}
