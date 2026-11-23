import { expectTypeOf, test } from 'vitest';
import { createToken } from '@coldmind/inject/token';
import type { BindingHandle, ServiceLifecycle } from '@coldmind/inject/contracts';
import type { IContainer as TypedFactoryContainer } from '@coldmind/inject/contracts';
declare const c: TypedFactoryContainer;
const numberToken = createToken<number>('number');
const stringToken = createToken<string>('string');
// Regression for structural legacy fallback: inject reserves the tuple dialect.
const predeclaredTooShort = { inject: [numberToken] as const, factory: () => 'legacy-compatible' };
// @ts-expect-error Known inject prevents fallback to the container/data lifecycle overload.
c.singleton(stringToken, predeclaredTooShort);
const predeclaredValid = { inject: [numberToken] as const, factory: (n: number) => n.toFixed() };
const accepted: BindingHandle<string> = c.singleton(stringToken, predeclaredValid);
const legacy: ServiceLifecycle<string> = { factory: scope => scope.getScopeId() };
const acceptedLegacy: BindingHandle<string> = c.singleton(stringToken, legacy);
// Static erasure of inject cannot be detected: TS intentionally permits ordinary
// structural width subtyping. The runtime must discriminate by inject presence.
// A caller explicitly hiding inject behind ServiceLifecycle has discarded proof.
void [accepted, acceptedLegacy];

test('compiled positive and negative contracts: typed-factory.dialects.test-d.ts', () => { expectTypeOf(accepted).toEqualTypeOf<BindingHandle<string>>(); expectTypeOf(acceptedLegacy).toEqualTypeOf<BindingHandle<string>>(); });
