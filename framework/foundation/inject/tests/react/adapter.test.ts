import { afterEach, expect, test } from 'vitest';
import { act, Component, createElement, StrictMode, Suspense, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { renderToString } from 'react-dom/server';
import { Container, createToken } from '@coldmind/inject';
import { ContainerProvider, ScopeProvider, createRequestScope, resetService, useScope, useService } from '@coldmind/inject/react';

let root: Root | undefined;
let host: HTMLElement | undefined;

afterEach(async () => {
  if (root) await act(async () => root!.unmount());
  host?.remove();
  root = undefined;
  host = undefined;
});

async function mount(element: ReturnType<typeof createElement>): Promise<void> {
  host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => root!.render(element));
}

const settle = (ms = 0) => new Promise(resolve => setTimeout(resolve, ms));

test.runIf(process.env.INJECT_EXPECT_BUN === '1')('Bun verification executes the test worker on Bun', () => {
  expect('bun' in process.versions).toBe(true);
});

/** Captures render failures without treating suspension as an error. */
class ServiceBoundary extends Component<{ children?: ReactNode; onError(error: unknown): void }, { failed: boolean }> {
  /** Whether a child has failed. */
  state = { failed: false };
  /** Shows the failure fallback. */
  static getDerivedStateFromError(): { failed: boolean } { return { failed: true }; }
  /** Reports the error to the test. */
  componentDidCatch(error: unknown): void { this.props.onError(error); }
  /** Renders the service or its error fallback. */
  render(): ReactNode { return this.state.failed ? 'failed' : this.props.children; }
}

test.each([[false, false], [true, false], [false, true], [true, true]])('useService retains failures and retries only after reset (StrictMode: %s, sync: %s)', async (strict, sync) => {
  const container = new Container();
  const Data = createToken<string>('failing-data');
  let built = 0;
  let failure: unknown;
  const result = () => { if (built === 1) throw new Error('service failed'); return 'recovered'; };
  container.singleton(Data, sync ? () => { built++; return result(); } : async () => { built++; await settle(5); return result(); });
  const View = () => createElement('p', null, useService(Data));
  const tree = (key: number) => createElement(strict ? StrictMode : Suspense, null,
    createElement(ContainerProvider, { container }, createElement(ServiceBoundary, { key, onError: error => { failure = error; } },
      createElement(Suspense, { fallback: 'loading' }, createElement(View)))));
  await mount(tree(0));
  await act(async () => { await settle(30); });
  expect(host!.textContent).toBe('failed');
  expect(failure).toBeInstanceOf(Error);
  expect(built).toBe(1);
  await act(async () => root!.render(tree(1)));
  expect(host!.textContent).toBe('failed');
  expect(built).toBe(1);
  resetService(container, Data);
  await act(async () => root!.render(tree(2)));
  await act(async () => { await settle(30); });
  expect(host!.textContent).toBe('recovered');
  expect(built).toBe(2);
  await container.dispose();
});

test.each([undefined, null, 0, ''])('useService retains a falsy rejection (%s)', async reason => {
  const container = new Container();
  const Data = createToken<string>('falsy-failure');
  let built = 0;
  const failures: unknown[] = [];
  container.singleton(Data, async () => { built++; await settle(5); throw reason; });
  const View = () => createElement('p', null, useService(Data));
  await mount(createElement(ContainerProvider, { container },
    createElement(ServiceBoundary, { onError: error => { failures.push(error); } },
      createElement(Suspense, { fallback: 'loading' }, createElement(View)))));
  await act(async () => { await settle(30); });
  expect(host!.textContent).toBe('failed');
  expect(failures).toEqual([reason]);
  expect(built).toBe(1);
  await container.dispose();
});

test('resetService preserves a pending shared build', async () => {
  const container = new Container();
  const Data = createToken<string>('pending-reset');
  let built = 0;
  let finish!: (value: string) => void;
  container.singleton(Data, () => { built++; return new Promise<string>(resolve => { finish = resolve; }); });
  const View = () => createElement('p', null, useService(Data));
  const tree = (key: number) => createElement(ContainerProvider, { container },
    createElement(Suspense, { fallback: 'loading' }, createElement(View, { key })));
  await mount(tree(0));
  resetService(container, Data);
  await act(async () => root!.render(tree(1)));
  expect(built).toBe(1);
  await act(async () => { finish('loaded'); await settle(); });
  expect(host!.textContent).toBe('loaded');
  expect(built).toBe(1);
  await container.dispose();
});

test('a failed service does not poison another scope or token', async () => {
  const first = new Container();
  const second = new Container();
  const Data = createToken<string>('isolated-failure');
  const Other = createToken<string>('unaffected');
  let built = 0;
  first.singleton(Data, async () => { built++; await settle(5); throw new Error('first failed'); });
  first.singleton(Other, () => 'other');
  second.singleton(Data, () => 'second');
  const View = ({ token }: { token: typeof Data }) => createElement('p', null, useService(token));
  const tree = (container: Container, token: typeof Data, key: number) => createElement(ContainerProvider, { container },
    createElement(ServiceBoundary, { key, onError: () => {} },
      createElement(Suspense, { fallback: 'loading' }, createElement(View, { token }))));
  await mount(tree(first, Data, 0));
  await act(async () => { await settle(30); });
  expect(host!.textContent).toBe('failed');
  resetService(second, Data);
  await act(async () => root!.render(tree(second, Data, 1)));
  expect(host!.textContent).toBe('second');
  await act(async () => root!.render(tree(first, Other, 2)));
  expect(host!.textContent).toBe('other');
  await act(async () => root!.render(tree(first, Data, 3)));
  expect(host!.textContent).toBe('failed');
  expect(built).toBe(1);
  await first.dispose();
  await second.dispose();
});

test('useService reads a built singleton under a provider', async () => {
  const container = new Container();
  const Name = createToken<string>('name');
  container.singleton(Name, () => 'zinject');
  await container.resolve(Name);
  const View = () => createElement('p', null, useService(Name));
  await mount(createElement(ContainerProvider, { container }, createElement(View)));
  expect(host!.textContent).toBe('zinject');
});

test('useService suspends until an async singleton is built', async () => {
  const container = new Container();
  const Data = createToken<string>('data');
  let finish!: (value: string) => void;
  const pending = new Promise<string>(resolve => { finish = resolve; });
  container.singleton(Data, () => pending);
  const View = () => createElement('p', null, useService(Data));
  await mount(createElement(ContainerProvider, { container },
    createElement(Suspense, { fallback: createElement('span', null, 'loading') }, createElement(View))));
  expect(host!.textContent).toBe('loading');
  await act(async () => { finish('loaded'); await pending; });
  expect(host!.textContent).toBe('loaded');
});

test('StrictMode keeps a mounted scope alive and disposes it on unmount', async () => {
  const container = new Container();
  const Session = createToken<{ id: number }>('session');
  let disposed = 0;
  let scopesCreated = 0;
  const createScope = container.createScope.bind(container);
  container.createScope = () => { scopesCreated++; return createScope(); };
  container.scoped(Session, { factory: () => ({ id: 1 }), onDispose: () => { disposed++; } });
  const View = () => createElement('p', null, String(useService(Session).id));
  await mount(createElement(StrictMode, null,
    createElement(ContainerProvider, { container }, createElement(ScopeProvider, null, createElement(View)))));
  await act(async () => { await settle(); });
  expect(host!.textContent).toBe('1');
  expect(disposed).toBe(0);
  expect(scopesCreated).toBe(1);
  await act(async () => root!.unmount());
  root = undefined;
  await settle();
  expect(disposed).toBe(1);
});

test('server render uses one request scope per render', async () => {
  const container = new Container();
  const Id = createToken<number>('id');
  let built = 0;
  container.request(Id, () => ++built);
  const View = () => createElement('p', null, String(useService(Id)));
  const scope = createRequestScope(container);
  await scope.resolve(Id);
  const html = renderToString(createElement(ContainerProvider, { container: scope }, createElement(View)));
  expect(html).toBe('<p>1</p>');
  expect(built).toBe(1);
  await scope.dispose();
});

test('ScopeProvider isolates independent server renders using the same container', async () => {
  const container = new Container();
  const Id = createToken<number>('render-id');
  let built = 0;
  container.request(Id, () => ++built);
  const View = () => createElement('p', null, String(useService(Id)));
  const tree = createElement(ContainerProvider, { container },
    createElement(ScopeProvider, { kind: 'request' }, createElement(View)));
  expect(renderToString(tree)).toContain('<p>1</p>');
  expect(renderToString(tree)).toContain('<p>2</p>');
  await container.dispose();
});

test('nested ScopeProviders inherit the enclosing scope registrations', async () => {
  const container = new Container();
  const Name = createToken<string>('outer-name');
  const Outer = () => {
    const scope = useScope();
    if (!scope.isRegistered(Name)) scope.registerInstance(Name, 'outer');
    return createElement(ScopeProvider, null, createElement(Inner));
  };
  const Inner = () => createElement('p', null, useService(Name));
  await mount(createElement(ContainerProvider, { container },
    createElement(ScopeProvider, null, createElement(Outer))));
  expect(host!.textContent).toBe('outer');
});

test('an async scoped service survives StrictMode and Suspense retries', async () => {
  const container = new Container();
  const Data = createToken<string>('scoped-data');
  let built = 0;
  let disposed = 0;
  container.scoped(Data, {
    factory: async () => { built++; await settle(10); return 'loaded'; },
    onDispose: () => { disposed++; },
  });
  const View = () => createElement('p', null, useService(Data));
  await mount(createElement(StrictMode, null, createElement(ContainerProvider, { container },
    createElement(ScopeProvider, { fallback: 'loading' }, createElement(View)))));
  await act(async () => { await settle(30); });
  expect(host!.textContent).toBe('loaded');
  expect(built).toBe(1);
  expect(disposed).toBe(0);
  await act(async () => root!.unmount());
  root = undefined;
  await settle();
  expect(disposed).toBe(1);
});

test('changing the parent container replaces and disposes the previous scope', async () => {
  const first = new Container();
  const second = new Container();
  const Name = createToken<string>('parent-name');
  let disposed = 0;
  first.scoped(Name, { factory: () => 'first', onDispose: () => { disposed++; } });
  second.scoped(Name, () => 'second');
  const View = () => createElement('p', null, useService(Name));
  const tree = (container: Container) => createElement(ContainerProvider, { container },
    createElement(ScopeProvider, null, createElement(View)));
  await mount(tree(first));
  expect(host!.textContent).toBe('first');
  await act(async () => root!.render(tree(second)));
  await settle();
  expect(host!.textContent).toBe('second');
  expect(disposed).toBe(1);
});
