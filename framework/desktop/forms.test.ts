/**
 * Tests of the backend runtime against a fake `@desktop/bridge`: what the form
 * class reads and writes, and what each host message does.
 */
import { beforeEach, describe, expect, mock, test } from 'bun:test';
import type { HostMessage } from '@desktop/bridge';

/** Every property write sent to a WebView, as `[form, control, prop, value]`. */
const writes: unknown[][] = [];
/** Carried-over control values, by form. */
let carried: Record<string, Record<string, Record<string, unknown>>> = {};
/** The handler `start` installed. */
let handler: ((message: HostMessage) => unknown) | null = null;

mock.module('@desktop/bridge', () => ({
  setProperty: (...write: unknown[]) => void writes.push(write),
  restored: (form: string) => carried[form] ?? {},
  serve: (fn: (message: HostMessage) => unknown) => void (handler = fn),
}));

const { Control, Form, start } = await import('./forms.ts');

/** What the handlers of the test form saw, in order. */
let log: string[] = [];

/** A control proxy with the properties of its kind. */
type Proxy = InstanceType<typeof Control> & Record<string, any>;

/** A form class as a developer writes it: no constructor, fields call `find`. */
class Main extends Form {
  save = this.find<Proxy>('Save');
  // Shadows `Form.name` on purpose: the hazard `Control#owner` guards against.
  // @ts-expect-error TS2610: a field overriding an accessor.
  name = this.find<Proxy>('Name');
  act = this.find<Proxy>('Act');
  clicked = () => void log.push(`clicked:${this.name.text}`);
  executed = () => void log.push('executed');
  created = () => void log.push('created');
  resized = (size: { width: number }) => void log.push(`resized:${size.width}`);
  closing = (query: { cancel: boolean }) => {
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
 * @return the form instance, captured through its constructor
 */
function boot(): Main {
  let instance!: Main;
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
        create: Captured as unknown as new () => InstanceType<typeof Form>,
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
 * @param method `<target>.<event>`
 * @param payload the message payload
 * @param form the window id
 * @return what the dispatch returned
 */
function send(method: string, payload?: HostMessage['payload'], form = 'Main'): unknown {
  const [target, event] = method.split('.') as [string, string];
  return handler!({ form, target, event, payload, request: event === 'onCloseQuery' });
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
    // A value from the WebView is not sent back.
    expect(writes).toEqual([]);
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
