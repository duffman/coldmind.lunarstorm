/**
 * The property mirror of one form's WebView (spec 6: "VCL components, mirror of
 * control properties").
 *
 * Decisions:
 * - The mirror is the only state a control has. A control renders its literal
 *   props from the view resource, overridden by the mirror's values; it keeps no
 *   React state of its own for a property, so a backend write and a user edit can
 *   never disagree about which value is current.
 * - A control's values are one immutable object, replaced on every change, so a
 *   component can subscribe with `useSyncExternalStore` and re-render only when
 *   its own control changed.
 * - A user event writes the changed values into the mirror first and then sends
 *   them, so the control shows the edit without waiting for the backend.
 */

/** The property values of one control, by property name. */
export type Values = Readonly<Record<string, unknown>>;

/**
 * The shell's IPC client, `window.zebra` (spec 6, "SHELL"). The VCL talks to the
 * host only through this.
 */
export interface Bridge {
  /** The form this WebView shows. */
  readonly windowId: string;
  /** `debug`, `release` or `browser`. */
  readonly mode: string;
  /** Control name to its latest property values, as the host last sent them. */
  readonly state: Readonly<Record<string, Values>>;
  /**
   * Sends a control event.
   * @param control control name
   * @param event event name, e.g. `onClick`
   * @param props the property values the event changed
   * @param args the event's arguments
   */
  emit(control: string, event: string, props?: Record<string, unknown>, args?: unknown): void;
  /**
   * Subscribes to property updates.
   * @param listener receives the changed controls
   * @return unsubscribe
   */
  onUpdate(listener: (controls: Record<string, Record<string, unknown>>) => void): () => void;
  /**
   * Subscribes to `dev.refresh` of this form.
   * @param listener receives the refresh stamp
   * @return unsubscribe
   */
  onRefresh(listener: (stamp: number) => void): () => void;
}

/** The value a control without mirrored properties reads. */
const EMPTY: Values = Object.freeze({});

/** The property mirror of one form. */
export class Mirror {
  /** Control name to its current values. */
  readonly #values = new Map<string, Values>();
  /** Control name to the components showing it. */
  readonly #listeners = new Map<string, Set<() => void>>();
  /** The shell's IPC client. */
  readonly #bridge: Bridge;
  /** Ends the subscription to the bridge. */
  readonly #detach: () => void;

  /**
   * Starts mirroring a form.
   * @param bridge the shell's IPC client
   */
  constructor(bridge: Bridge) {
    this.#bridge = bridge;
    for (const [name, values] of Object.entries(bridge.state)) this.#values.set(name, { ...values });
    this.#detach = bridge.onUpdate(controls => this.#merge(controls));
  }

  /** Stops receiving updates; called when the form unmounts. */
  dispose(): void {
    this.#detach();
    this.#listeners.clear();
  }

  /**
   * Reads a control's mirrored values.
   * @param name control name
   * @return the values; the same object until one of them changes
   */
  values(name: string): Values {
    return this.#values.get(name) ?? EMPTY;
  }

  /**
   * Subscribes a component to one control.
   * @param name control name
   * @param listener called after the control's values changed
   * @return unsubscribe
   */
  subscribe(name: string, listener: () => void): () => void {
    let listeners = this.#listeners.get(name);
    if (!listeners) this.#listeners.set(name, (listeners = new Set()));
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }

  /**
   * Reports a user event: mirrors the values it changed, then sends it.
   * @param name control name
   * @param event event name
   * @param props the property values the event changed
   * @param args the event's arguments
   */
  emit(name: string, event: string, props?: Record<string, unknown>, args?: unknown): void {
    if (props) this.#merge({ [name]: props });
    this.#bridge.emit(name, event, props, args);
  }

  /**
   * Applies property values and notifies the components of every control that
   * actually changed.
   * @param controls control name to changed values
   */
  #merge(controls: Record<string, Record<string, unknown>>): void {
    for (const [name, props] of Object.entries(controls)) {
      const current = this.#values.get(name) ?? EMPTY;
      // A value equal to the current one (the echo of the control's own edit)
      // must not re-render: it would reset a text field's caret.
      if (Object.entries(props).every(([key, value]) => Object.is(current[key], value))) continue;
      this.#values.set(name, { ...current, ...props });
      this.#listeners.get(name)?.forEach(listener => listener());
    }
  }
}
