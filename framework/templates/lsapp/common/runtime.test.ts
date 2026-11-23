import { describe, expect, test } from 'bun:test';
import { Application, Button, Edit, EventLoop, h, Inject, Label, Service, ServiceContainer, View, ViewAction, ViewClass, ViewOutlet, WindowLayout, type CloseQueryEvent, type DesktopHost, type LSNode, type Window } from './index';

class MemoryHost implements DesktopHost {
  readonly opened: Window[] = [];
  readonly closed: Window[] = [];
  open(window: Window): void { this.opened.push(window); }
  close(window: Window): void { this.closed.push(window); }
  dispose(): void {}
}

@Service
class Lookup {
  calls = 0;
  find(): string { this.calls++; return 'Ada'; }
}

@ViewClass
class TestView extends View {
  @ViewOutlet(Edit) declare input: Edit;
  @ViewOutlet(Label) declare result: Label;
  @ViewOutlet(Button) declare button: Button;
  @Inject(Lookup) declare lookup: Lookup;
  createCount = 0;
  preventClose = false;
  protected onCreate(): void { this.createCount++; this.input.text = '1001'; }
  protected onCloseQuery(event: CloseQueryEvent): void { event.cancel = this.preventClose; }
  @ViewAction find(): void { this.result.caption = this.lookup.find(); }
}
function layout(view: TestView): LSNode {
  return h(WindowLayout, {}, h(Edit, { outlet: view.input }), h(Button, { outlet: view.button, onClick: view.find }), h(Label, { outlet: view.result }));
}

describe('retained view runtime', () => {
  test('outlets retain identity and host events invoke bound actions and injected services', () => {
    const host = new MemoryHost();
    const app = new Application(host);
    const view = app.run(TestView, layout);
    const original = view.input;
    view.button.click();
    expect(view.result.caption).toBe('Ada');
    expect(view.lookup.calls).toBe(1);
    expect(view.input).toBe(original);
    expect(view.input.text).toBe('1001');
    expect(view.createCount).toBe(1);
    expect(host.opened).toEqual([view]);
    app.dispose();
    expect(original.disposed).toBe(true);
  });
  test('close query cancels closure; successful close disposes descendants', () => {
    const app = new Application(new MemoryHost());
    const view = app.run(TestView, layout);
    const button = view.button;
    view.preventClose = true;
    expect(view.close()).toBe(false);
    expect(button.disposed).toBe(false);
    view.preventClose = false;
    expect(view.close()).toBe(true);
    expect(button.disposed).toBe(true);
    button.click();
    expect(app.services.get(Lookup).calls).toBe(0);
    app.dispose();
  });
  test('one outlet cannot be connected twice', () => {
    const app = new Application(new MemoryHost());
    expect(() => app.run(TestView, view => h(WindowLayout, {}, h(Edit, { outlet: view.input }), h(Edit, { outlet: view.input })))).toThrow('only be connected once');
    app.dispose();
  });
  test('anonymous callbacks do not qualify as declared view actions', () => {
    const app = new Application(new MemoryHost());
    expect(() => app.run(TestView, () => h(Button, { onClick: () => {} }))).toThrow('@ViewAction');
    app.dispose();
  });
  test('disabled controls do not dispatch actions', () => {
    const app = new Application(new MemoryHost());
    const view = app.run(TestView, layout);
    view.button.enabled = false;
    view.button.click();
    expect(view.lookup.calls).toBe(0);
    app.dispose();
  });
});

describe('application infrastructure', () => {
  test('nested messages run after the current message; failed queues recover', () => {
    const loop = new EventLoop();
    const seen: number[] = [];
    loop.dispatch(() => { seen.push(1); loop.dispatch(() => seen.push(3)); seen.push(2); });
    expect(seen).toEqual([1, 2, 3]);
    expect(() => loop.dispatch(() => { loop.dispatch(() => seen.push(99)); throw new Error('failed'); })).toThrow('failed');
    loop.dispatch(() => seen.push(4));
    expect(seen).toEqual([1, 2, 3, 4]);
    loop.dispose();
    loop.dispatch(() => seen.push(5));
    expect(seen).toEqual([1, 2, 3, 4]);
  });
  test('services are shared per application and explicitly registered factories are supported', () => {
    const services = new ServiceContainer();
    const instance = new Lookup();
    services.registerFactory(Lookup, () => instance);
    expect(services.get(Lookup)).toBe(instance);
    expect(services.get(Lookup)).toBe(instance);
    services.dispose();
    expect(() => services.get(Lookup)).toThrow('disposed');
  });
});

@ViewClass
class FailingView extends View {
  @ViewAction async fail(): Promise<void> {
    await Promise.resolve();
    throw new Error('Lookup unavailable');
  }
}

test('async actions preserve their promise and route failures to the application', async () => {
  const app = new Application(new MemoryHost());
  const errors: unknown[] = [];
  app.onError.subscribe(error => errors.push(error));
  const view = app.run(FailingView, () => h(WindowLayout, {}));
  const completion = view.fail();
  expect(completion).toBeInstanceOf(Promise);
  await expect(completion).rejects.toThrow('Lookup unavailable');
  expect(errors).toHaveLength(1);
  app.dispose();
});
