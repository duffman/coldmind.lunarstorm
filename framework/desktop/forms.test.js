/**
 * Tests of the backend runtime against a fake `@desktop/bridge`: what the form
 * class reads and writes, and what each host message does.
 */
import { beforeEach, describe, expect, mock, test } from 'bun:test';

/** Every property write sent to a WebView, as `[form, control, prop, value]`. */
const writes = [];
/** Carried-over control values, by form. */
let carried = {};
/** The handler `start` installed. */
let handler = null;

mock.module('@desktop/bridge', () => ({
  setProperty: (...write) => void writes.push(write),
  restored: form => carried[form] ?? {},
  serve: fn => void (handler = fn),
}));

const { Control, Form, start } = await import('./forms.js');

/** What the handlers of the test form saw, in order. */
let log = [];

/** A form class as a developer writes it: no constructor, fields call `find`. */
class Main extends Form {
  save = this.find('Save');
  name = this.find('Name');
  act = this.find('Act');
  clicked = () => void log.push(`clicked:${this.name.text}`);
  executed = () => void log.push('executed');
  created = () => void log.push('created');
  resized = size => void log.push(`resized:${size.width}`);
  closing = query => {
    query.cancel = this.name.text === 'dirty';
  };
  slow = async () => {
    await Promise.resolve();
    log.push('slow');
  };
  ready = () => void log.push('ready');
}

/**
 * Starts a backend with the test form.
 * @return {Main} the form instance, captured through its `created` handler's `this`
 */
function boot() {
  let instance;
  class Captured extends Main {
    constructor() {
      super();
      instance = this;
    }
  }
  Object.defineProperty(Captured, 'name', { value: 'Main' });
  start({
    application: { mainForm: 'Main', connections: { '@application.onReady': ['ready'] } },
    forms: {
      Main: {
        create: Captured,
        controls: {
          Save: ['button', { caption: 'Save', enabled: true, action: 'Act' }],
          Name: ['edit', { text: '', enabled: true }],
          Act: ['action', { caption: 'Save', enabled: true, checked: false }],
        },
        connections: {
          'Main.onCreate': ['created'],
          'Main.onResize': ['resized'],
          'Main.onCloseQuery': ['closing'],
          'Save.onClick': ['clicked', 'executed'],
          'Name.onChange': ['slow'],
        },
        actions: { Act: ['Save'] },
      },
    },
  });
  return instance;
}

/**
 * Delivers a message as the host does.
 * @param {string} method `<target>.<event>`
 * @param {unknown} [payload] the message payload
 * @param {string} [form] the window id
 * @return {unknown} what the dispatch returned
 */
function send(method, payload, form = 'Main') {
  const [target, event] = method.split('.');
  return handler({ form, target, event, payload, request: event === 'onCloseQuery' });
}

beforeEach(() => {
  writes.length = 0;
  carried = {};
  log = [];
});

describe('control proxies', () => {
  test('read the view values and send writes to the WebView once', () => {
    const form = boot();
    expect(form.save).toBeInstanceOf(Control);
    expect(form.save.name).toBe('Save');
    expect(form.save.kind).toBe('button');
    expect(form.save.caption).toBe('Save');
    form.save.caption = 'Store';
    form.save.caption = 'Store';
    form.save.enabled = false;
    expect(form.save.caption).toBe('Store');
    expect(writes).toEqual([['Main', 'Save', 'caption', 'Store'], ['Main', 'Save', 'enabled', false]]);
  });

  test('carried-over values win over the view values', () => {
    carried = { Main: { Name: { text: 'kept' } } };
    const form = boot();
    expect(form.name.text).toBe('kept');
    expect(form.name.enabled).toBe(true);
  });

  test('an unknown control name is an error naming the form', () => {
    const form = boot();
    expect(() => form.find('Nope')).toThrow("Main has no control named 'Nope'");
  });

  test('a control follows the action it is linked to', () => {
    const form = boot();
    form.act.enabled = false;
    form.act.caption = 'Save all';
    expect(form.save.enabled).toBe(false);
    expect(form.save.caption).toBe('Save all');
    expect(writes).toEqual([
      ['Main', 'Act', 'enabled', false],
      ['Main', 'Save', 'enabled', false],
      ['Main', 'Act', 'caption', 'Save all'],
      ['Main', 'Save', 'caption', 'Save all'],
    ]);
  });

  test('a class that is not a form of the application cannot be created', () => {
    boot();
    class Stray extends Form {}
    expect(() => new Stray()).toThrow("'Stray' is not a form of this application");
  });
});

describe('dispatch', () => {
  test('a control event updates the proxy, then runs its handlers in order', () => {
    const form = boot();
    send('Name.onChange', { props: { text: 'typed' } });
    expect(form.name.text).toBe('typed');
    expect(writes).toEqual([], 'a value from the WebView is not sent back');
    send('Save.onClick', { props: {} });
    expect(log).toEqual(['clicked:typed', 'executed']);
  });

  test('form events get their payload, application events go to the main form', () => {
    boot();
    send('Main.onCreate', {});
    send('Main.onResize', { width: 800, height: 600 });
    send('@application.onReady', {}, '@application');
    expect(log).toEqual(['created', 'resized:800', 'ready']);
  });

  test('onCloseQuery answers with the cancel the handler set', () => {
    const form = boot();
    expect(send('Main.onCloseQuery', {})).toEqual({ cancel: false });
    form.name.receive({ text: 'dirty' });
    expect(send('Main.onCloseQuery', {})).toEqual({ cancel: true });
  });

  test('an async handler returns a promise the host can wait on', async () => {
    boot();
    const pending = send('Name.onChange', { props: { text: 'x' } });
    expect(log).toEqual([]);
    await pending;
    expect(log).toEqual(['slow']);
  });

  test('unconnected events, unknown forms and unknown targets do nothing', () => {
    boot();
    expect(send('Name.onClick', { props: {} })).toBeUndefined();
    expect(send('Main.onShow', {})).toBeUndefined();
    expect(send('Other.onCreate', {}, 'Other')).toBeUndefined();
    expect(() => send('Ghost.onClick', { props: { x: 1 } })).toThrow("Main has no control named 'Ghost'");
    expect(log).toEqual([]);
  });
});
