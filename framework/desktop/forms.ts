/**
 * The backend runtime of a model project (spec 2.5, 4c): the `Form` base class,
 * the control proxies a form class reads and writes, and the dispatch that
 * routes host messages to handlers.
 *
 * The generated backend entry (spec 4c) calls `start` with, per form, the form
 * class, the control table from its view resource and its event connections.
 *
 * Decisions:
 * - A form class has no constructor of its own (spec 2.5); its field
 *   initializers already call `this.find`. So the base constructor finds the
 *   form's table by the class's name, which is the form's name.
 * - One proxy class per control kind, built once from the kind's prop list. A
 *   prop is an accessor: reading returns the mirrored value, writing stores it
 *   and sends it to the form's WebView (spec 4d). Writing the value a prop
 *   already has sends nothing.
 * - A WebView event first updates the proxy with the values the user changed,
 *   then runs the connected handlers, so a handler always reads current values.
 * - `onCloseQuery` is a request: the handler gets a query object and sets
 *   `cancel`, as in Delphi; the answer to the host is `{ cancel }`.
 * - A control linked to an action (`action="X"`) runs the action's `onExecute`
 *   after its own click handlers, and follows the action's `caption`, `enabled`
 *   and `checked` when the form class writes them.
 */
import { restored, serve, setProperty, type HostMessage } from '@desktop/bridge';

/** The `windowId` of application events, as the host sends it. */
export const APPLICATION = '@application';

/** The props of an action that its linked controls follow. */
const ACTION_PROPS = ['caption', 'enabled', 'checked'];

/** What the generated backend entry passes `start` for one form. */
export interface FormDefinition {
  /** The form class. */
  create: new () => Form;
  /** Control name to its kind and initial values. */
  controls: Record<string, [kind: string, values: Record<string, unknown>]>;
  /** `<target>.<event>` to the names of the handlers to call, in order. */
  connections: Record<string, string[]>;
  /** Action name to the controls linked to it. */
  actions?: Record<string, string[]>;
}

/** What the generated backend entry passes `start`. */
export interface Project {
  /** Application-level wiring. */
  application: { mainForm: string; connections: Record<string, string[]> };
  /** Form name to its definition. */
  forms: Record<string, FormDefinition>;
}

/** Form name to its generated definition. */
const definitions = new Map<string, FormDefinition>();

/** Control kind to its proxy class. */
const proxies = new Map<string, typeof Control>();

/** Form name to its instance. */
const forms = new Map<string, Form>();

/** A control as the form class sees it. */
export class Control {
  /** The form the control belongs to. */
  #form: Form;
  /**
   * The owning form's name. Held here and not read from the form: a form class
   * may declare a field called `name`, which would shadow `Form.name` and send
   * every write to the wrong window.
   */
  #owner: string;
  /** The control's name. */
  #name: string;
  /** The control's kind, e.g. `button`. */
  #kind: string;
  /** The current property values. */
  #values: Record<string, unknown>;

  /**
   * @param form the owning form
   * @param owner the owning form's name
   * @param name the control's name
   * @param kind the control's kind
   * @param values the initial property values
   */
  constructor(form: Form, owner: string, name: string, kind: string, values: Record<string, unknown>) {
    this.#form = form;
    this.#owner = owner;
    this.#name = name;
    this.#kind = kind;
    this.#values = values;
  }

  /** @return the control's name */
  get name(): string {
    return this.#name;
  }

  /** @return the control's kind, e.g. `button` */
  get kind(): string {
    return this.#kind;
  }

  /**
   * Reads a property.
   * @param prop the property name
   * @return its value
   */
  get(prop: string): unknown {
    return this.#values[prop];
  }

  /**
   * Writes a property and sends it to the form's WebView.
   * @param prop the property name
   * @param value the new value
   */
  set(prop: string, value: unknown): void {
    if (Object.is(this.#values[prop], value)) return;
    this.#values[prop] = value;
    setProperty(this.#owner, this.#name, prop, value);
    if (this.#kind === 'action' && ACTION_PROPS.includes(prop)) {
      for (const linked of Form.linked(this.#form, this.#name)) linked.set(prop, value);
    }
  }

  /**
   * Takes the values a WebView event changed. They came from the WebView, so
   * they are not sent back.
   * @param props the changed values
   */
  receive(props: Record<string, unknown>): void {
    Object.assign(this.#values, props);
  }
}

/**
 * Returns the proxy class of a control kind, creating it on first use.
 * @param kind the control kind
 * @param props the kind's property names
 * @return the proxy class
 */
function proxyOf(kind: string, props: Iterable<string>): typeof Control {
  let proxy = proxies.get(kind);
  if (proxy) return proxy;
  proxy = class extends Control {};
  for (const prop of props) {
    Object.defineProperty(proxy.prototype, prop, {
      get(this: Control): unknown {
        return this.get(prop);
      },
      set(this: Control, value: unknown) {
        this.set(prop, value);
      },
      enumerable: true,
    });
  }
  proxies.set(kind, proxy);
  return proxy;
}

/** The base of every form class (spec 2.5). */
export class Form {
  /** The form's name. */
  #name: string;
  /** Control name to its proxy. */
  #controls = new Map<string, Control>();
  /** Action name to linked control names. */
  #actions: Record<string, string[]>;

  constructor() {
    const name = new.target.name;
    const definition = definitions.get(name);
    if (!definition) throw new Error(`'${name}' is not a form of this application`);
    this.#name = name;
    this.#actions = definition.actions ?? {};
    const carried = restored(name);
    for (const [control, [kind, values]] of Object.entries(definition.controls)) {
      const initial = { ...values, ...(carried[control] ?? {}) };
      this.#controls.set(control, new (proxyOf(kind, Object.keys(values)))(this, name, control, kind, initial));
    }
  }

  /** @return the form's name */
  get name(): string {
    return this.#name;
  }

  /**
   * Finds a control of this form.
   * @param name the control's name
   * @return the control
   */
  find<T extends Control = Control>(name: string): T {
    const control = this.#controls.get(name);
    if (!control) throw new Error(`${this.#name} has no control named '${name}'`);
    return control as T;
  }

  /**
   * Lists the controls linked to an action. Static, and reading the private
   * tables directly, so that no field or method a form class declares can
   * shadow it.
   * @param form the form
   * @param action the action's name
   * @return the linked controls
   */
  static linked(form: Form, action: string): Control[] {
    return (form.#actions[action] ?? []).map(name => form.#controls.get(name)).filter((c): c is Control => c !== undefined);
  }

  /**
   * Finds a control for the dispatch, bypassing anything a form class declares.
   * @param form the form
   * @param name the control's name
   * @return the control
   */
  static control(form: Form, name: string): Control {
    const control = form.#controls.get(name);
    if (!control) throw new Error(`${form.#name} has no control named '${name}'`);
    return control;
  }
}

/**
 * Runs the handlers connected to one event.
 * @param form the form whose class holds the handlers
 * @param handlers the handler names
 * @param argument what each handler receives
 * @return a promise when any handler returned one, else `undefined`
 */
function run(form: Form, handlers: string[] | undefined, argument: unknown): Promise<unknown[]> | undefined {
  const waiting: PromiseLike<unknown>[] = [];
  const methods = form as unknown as Record<string, (argument: unknown) => unknown>;
  for (const handler of handlers ?? []) {
    const result = methods[handler]!(argument);
    if (result && typeof (result as PromiseLike<unknown>).then === 'function') waiting.push(result as PromiseLike<unknown>);
  }
  return waiting.length ? Promise.all(waiting) : undefined;
}

/**
 * Handles one message from the host.
 * @param message the host's message
 * @param application the application wiring
 * @return the answer to a request, or a promise the host waits on
 */
function dispatch(message: HostMessage, application: Project['application']): unknown {
  const payload = message.payload ?? {};
  const argument = payload.args !== undefined ? payload.args : payload;
  if (message.form === APPLICATION) {
    const main = forms.get(application.mainForm);
    return main && run(main, application.connections[`${message.target}.${message.event}`], argument);
  }
  const form = forms.get(message.form);
  const definition = definitions.get(message.form);
  if (!form || !definition) return undefined;
  const handlers = definition.connections[`${message.target}.${message.event}`];
  if (message.target === message.form) {
    if (message.event !== 'onCloseQuery') return run(form, handlers, argument);
    const query = { cancel: false };
    const done = run(form, handlers, query);
    const answer = () => ({ cancel: Boolean(query.cancel) });
    return done ? done.then(answer) : answer();
  }
  if (payload.props) Form.control(form, message.target).receive(payload.props);
  return run(form, handlers, argument);
}

/**
 * Starts the backend: creates every form and serves the host's messages.
 * @param project the generated project table
 */
export function start(project: Project): void {
  definitions.clear();
  forms.clear();
  for (const [name, definition] of Object.entries(project.forms)) definitions.set(name, definition);
  for (const [name, definition] of definitions) forms.set(name, new definition.create());
  serve(message => dispatch(message, project.application));
}
