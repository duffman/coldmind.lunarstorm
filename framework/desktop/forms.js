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
import { restored, serve, setProperty } from '@desktop/bridge';

/** The `windowId` of application events, as the host sends it. */
export const APPLICATION = '@application';

/** The props of an action that its linked controls follow. */
const ACTION_PROPS = ['caption', 'enabled', 'checked'];

/** @type {Map<string, FormDefinition>} form name -> its generated definition */
const definitions = new Map();

/** @type {Map<string, typeof Control>} control kind -> its proxy class */
const proxies = new Map();

/** @type {Map<string, Form>} form name -> its instance */
const forms = new Map();

/**
 * @typedef {object} FormDefinition
 * @property {new () => Form} create the form class
 * @property {Record<string, [string, Record<string, unknown>]>} controls control
 *   name to its kind and initial values
 * @property {Record<string, string[]>} connections `<target>.<event>` to the
 *   names of the handlers to call, in order
 * @property {Record<string, string[]>} [actions] action name to the controls
 *   linked to it
 */

/** A control as the form class sees it. */
export class Control {
  /** @type {Form} the form the control belongs to */
  #form;
  /**
   * @type {string} the owning form's name. Held here and not read from the form:
   * a form class may declare a field called `name`, which would shadow
   * `Form.name` and send every write to the wrong window.
   */
  #owner;
  /** @type {string} the control's name */
  #name;
  /** @type {string} the control's kind, e.g. `button` */
  #kind;
  /** @type {Record<string, unknown>} the current property values */
  #values;

  /**
   * @param {Form} form the owning form
   * @param {string} owner the owning form's name
   * @param {string} name the control's name
   * @param {string} kind the control's kind
   * @param {Record<string, unknown>} values the initial property values
   */
  constructor(form, owner, name, kind, values) {
    this.#form = form;
    this.#owner = owner;
    this.#name = name;
    this.#kind = kind;
    this.#values = values;
  }

  /** @return {string} the control's name */
  get name() {
    return this.#name;
  }

  /** @return {string} the control's kind, e.g. `button` */
  get kind() {
    return this.#kind;
  }

  /**
   * Reads a property.
   * @param {string} prop the property name
   * @return {unknown} its value
   */
  get(prop) {
    return this.#values[prop];
  }

  /**
   * Writes a property and sends it to the form's WebView.
   * @param {string} prop the property name
   * @param {unknown} value the new value
   */
  set(prop, value) {
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
   * @param {Record<string, unknown>} props the changed values
   */
  receive(props) {
    Object.assign(this.#values, props);
  }
}

/**
 * Returns the proxy class of a control kind, creating it on first use.
 * @param {string} kind the control kind
 * @param {Iterable<string>} props the kind's property names
 * @return {typeof Control} the proxy class
 */
function proxyOf(kind, props) {
  let proxy = proxies.get(kind);
  if (proxy) return proxy;
  proxy = class extends Control {};
  for (const prop of props) {
    Object.defineProperty(proxy.prototype, prop, {
      /** @return {unknown} the property's value */
      get() {
        return this.get(prop);
      },
      /** @param {unknown} value the new value */
      set(value) {
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
  /** @type {string} the form's name */
  #name;
  /** @type {Map<string, Control>} control name -> its proxy */
  #controls = new Map();
  /** @type {Record<string, string[]>} action name -> linked control names */
  #actions;

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

  /** @return {string} the form's name */
  get name() {
    return this.#name;
  }

  /**
   * Finds a control of this form.
   * @template {Control} T
   * @param {string} name the control's name
   * @return {T} the control
   */
  find(name) {
    const control = this.#controls.get(name);
    if (!control) throw new Error(`${this.#name} has no control named '${name}'`);
    return /** @type {T} */ (control);
  }

  /**
   * Lists the controls linked to an action. Static, and reading the private
   * tables directly, so that no field or method a form class declares can
   * shadow it.
   * @param {Form} form the form
   * @param {string} action the action's name
   * @return {Control[]} the linked controls
   */
  static linked(form, action) {
    return (form.#actions[action] ?? []).map(name => form.#controls.get(name)).filter(Boolean);
  }

  /**
   * Finds a control for the dispatch, bypassing anything a form class declares.
   * @param {Form} form the form
   * @param {string} name the control's name
   * @return {Control} the control
   */
  static control(form, name) {
    const control = form.#controls.get(name);
    if (!control) throw new Error(`${form.#name} has no control named '${name}'`);
    return control;
  }
}

/**
 * Runs the handlers connected to one event.
 * @param {Form} form the form whose class holds the handlers
 * @param {string[] | undefined} handlers the handler names
 * @param {unknown} argument what each handler receives
 * @return {unknown} a promise when any handler returned one, else `undefined`
 */
function run(form, handlers, argument) {
  const waiting = [];
  for (const handler of handlers ?? []) {
    const result = form[handler](argument);
    if (result && typeof result.then === 'function') waiting.push(result);
  }
  return waiting.length ? Promise.all(waiting) : undefined;
}

/**
 * Handles one message from the host.
 * @param {{form: string, target: string, event: string, payload: any, request: boolean}} message
 * @param {{mainForm: string, connections: Record<string, string[]>}} application
 * @return {unknown} the answer to a request, or a promise the host waits on
 */
function dispatch(message, application) {
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
 * @param {{application: {mainForm: string, connections: Record<string, string[]>}, forms: Record<string, FormDefinition>}} project
 *   the generated project table
 */
export function start(project) {
  definitions.clear();
  forms.clear();
  for (const [name, definition] of Object.entries(project.forms)) definitions.set(name, definition);
  for (const [name, definition] of definitions) forms.set(name, new definition.create());
  serve(message => dispatch(message, project.application));
}
