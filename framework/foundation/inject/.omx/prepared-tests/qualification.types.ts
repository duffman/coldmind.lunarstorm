/** Compiler fixture; proposed API only, never a nonexistent live public import. */
import { Container, Inject, type BindingHandle } from '@coldmind/inject';
import { lazy, provider, type Lazy, type Provider } from '@coldmind/inject/src/core/cminject.lazy.ts';
import {
  qualify, createToken, delay, type Token,
} from '../evidence/prio-one/qualification-prep/cminject.token.proposed.ts';

type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends (<T>() => T extends B ? 1 : 2) ? true : false;
function exact<T extends true>(): void {}
class Db { constructor(readonly url: string) {} }
abstract class AbstractDb { abstract readonly url: string; }
const primary = qualify(Db, 'primary');
const typed = qualify(createToken<Db>('db'), 'replica');
const delayed = qualify(delay(() => Db), 'replica');
const abstract = qualify(AbstractDb, 'replica');
const string = qualify('db', 'replica');
const symbol = qualify(Symbol('db'), 'replica');
const explicit = qualify<Db>('db', 'replica');
exact<Equal<typeof primary, Token<Db>>>();
exact<Equal<typeof typed, Token<Db>>>();
exact<Equal<typeof delayed, Token<Db>>>();
exact<Equal<typeof abstract, Token<AbstractDb>>>();
exact<Equal<typeof string, Token<unknown>>>();
exact<Equal<typeof symbol, Token<unknown>>>();
exact<Equal<typeof explicit, Token<Db>>>();
declare const root: Container;
const resolved = root.resolve(primary);
const sync = root.resolveSync(typed);
const all = root.resolveAll(primary);
const binding = root.singleton(primary, () => new Db('primary'));
exact<Equal<typeof resolved, Promise<Db>>>();
exact<Equal<typeof sync, Db>>();
exact<Equal<typeof all, Promise<Db[]>>>();
exact<Equal<typeof binding, BindingHandle<Db>>>();
const lazySlot = lazy(primary);
const providerSlot = provider(primary);
// Qualifying the service before wrapping preserves deferred-value inference.
import type { DependencyValue } from '@coldmind/inject/src/core/cminject.lazy.ts';
exact<Equal<DependencyValue<typeof lazySlot>, Lazy<Db>>>();
exact<Equal<DependencyValue<typeof providerSlot>, Provider<Db>>>();
class Consumer {
  @Inject(primary) field!: Db;
  static $inject = [primary];
  constructor(readonly db: Db) {}
}
// @ts-expect-error Qualified service types cannot resolve as unrelated values.
const wrong: Promise<number> = root.resolve(primary);
// @ts-expect-error The requested qualifier contract accepts strings, not symbols.
qualify(Db, Symbol('replica'));
// @ts-expect-error Qualify ordinary service tokens, not lazy declaration markers.
qualify(lazySlot, 'replica');
// @ts-expect-error Qualify ordinary service tokens, not provider declaration markers.
qualify(providerSlot, 'replica');
void Consumer;
