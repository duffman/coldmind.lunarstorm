import { describe, expect, test } from 'vitest';
import { Container, DiError, DiGraphError, Inject, Injectable, InjectOptional, LifecycleError, ServiceLifetime, createToken } from '@coldmind/inject';

class Clock { now(): number { return 42; } }
class Owned {
  static $inject = [undefined, Clock];
  constructor(readonly owner: string, readonly clock: Clock) {}
}
@Injectable()
class Plain { @Inject(Clock) clock!: Clock; @InjectOptional('absent') absent?: number; }
class Needs { static $inject = [Clock]; constructor(readonly clock: Clock) {} }

describe('synchronous construction', () => {
  test('a class with injected members resolves through resolveSync', () => {
    const root = new Container();
    root.singleton(Clock);
    root.singleton(Plain);
    const plain = root.resolveSync(Plain);
    expect(plain.clock).toBe(root.resolveSync(Clock));
    expect(plain.absent).toBe(undefined);
  });

  test('an asynchronous member does not resolve through resolveSync, but resolves through resolve', async () => {
    const root = new Container();
    root.singleton(Clock, async () => new Clock());
    root.singleton(Plain);
    expect(() => root.resolveSync(Plain)).toThrow(LifecycleError);
    expect((await root.resolve(Plain)).clock).toBeInstanceOf(Clock);
  });
});

describe('instantiate', () => {
  test('leading arguments fill the first parameters and the container resolves the rest', () => {
    const root = new Container();
    root.singleton(Clock);
    const value = root.instantiate(Owned, 'app');
    expect(value.owner).toBe('app');
    expect(value.clock).toBe(root.resolveSync(Clock));
  });

  test('the instance is not registered', () => {
    const root = new Container();
    root.singleton(Clock);
    expect(root.instantiate(Owned, 'a')).not.toBe(root.instantiate(Owned, 'a'));
    expect(root.isRegistered(Owned)).toBe(false);
  });

  test('instantiateAsync waits for asynchronous dependencies; instantiate refuses them', async () => {
    const root = new Container();
    root.singleton(Clock, async () => new Clock());
    expect(() => root.instantiate(Owned, 'a')).toThrow(LifecycleError);
    expect((await root.instantiateAsync(Owned, 'a')).clock).toBeInstanceOf(Clock);
  });

  test('a parameter with no class to resolve by fails with DI_UNRESOLVABLE_PARAMETER', () => {
    class Loose { constructor(readonly value: unknown) {} }
    const root = new Container();
    try { root.instantiate(Loose); expect.unreachable(); }
    catch (error) {
      expect(error).toBeInstanceOf(DiError);
      expect((error as DiError).code).toBe('DI_UNRESOLVABLE_PARAMETER');
      expect((error as Error).message).toMatch(/index 0 for Loose/);
    }
  });
});

describe('injectMembers', () => {
  test('applies the members once, however often it is called', () => {
    const root = new Container();
    root.singleton(Clock);
    const plain = new Plain();
    expect(root.injectMembers(plain)).toBe(plain);
    const clock = plain.clock;
    root.injectMembers(plain);
    expect(plain.clock).toBe(clock);
  });

  test('injectMembersAsync waits for asynchronous members', async () => {
    const root = new Container();
    root.singleton(Clock, async () => new Clock());
    const plain = await root.injectMembersAsync(new Plain());
    expect(plain.clock).toBeInstanceOf(Clock);
  });
});

describe('autoInject', () => {
  test('is off by default: an unregistered class is not registered', () => {
    const root = new Container();
    expect(root.autoInject).toBe(false);
    expect(() => root.resolveSync(Clock)).toThrow(/No registration/);
  });

  test('registers an unregistered class on demand, transient by default, tagged auto', () => {
    const root = new Container();
    root.autoInject = true;
    const a = root.resolveSync(Needs);
    expect(a).toBeInstanceOf(Needs);
    expect(a.clock).toBeInstanceOf(Clock);
    expect(root.resolveSync(Needs)).not.toBe(a);
    expect(root.bindingOf(Needs)?.kindTag).toBe('auto');
    expect(root.bindingOf(Needs)?.lifetime).toBe(ServiceLifetime.Transient);
  });

  test('uses the lifetime option', () => {
    const root = new Container();
    root.autoInject = { lifetime: ServiceLifetime.Singleton };
    expect(root.resolveSync(Clock)).toBe(root.resolveSync(Clock));
  });

  test('a scope inherits the setting until it sets its own', () => {
    const root = new Container();
    root.autoInject = true;
    const child = root.createScope();
    expect(child.autoInject).toBe(true);
    expect(child.resolveSync(Clock)).toBeInstanceOf(Clock);
    child.autoInject = false;
    expect(() => child.resolveSync(Needs)).toThrow(/No registration/);
  });

  test('never registers strings, symbols or typed tokens', () => {
    const root = new Container();
    root.autoInject = true;
    expect(() => root.resolveSync('name')).toThrow(/No registration/);
    expect(() => root.resolveSync(createToken<number>('typed'))).toThrow(/No registration/);
  });

  test('auto registrations are part of the graph', () => {
    const root = new Container();
    root.autoInject = true;
    root.resolveSync(Needs);
    expect(() => root.validate()).not.toThrow(DiGraphError);
    expect(root.getRegisteredTokens().length).toBe(2);
  });
});
