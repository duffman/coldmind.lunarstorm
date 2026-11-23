import { Container, createToken, qualify, tokenName } from '@coldmind/inject';
import { inspect } from '@coldmind/inject/inspect';

/** Inspects declared/observed wiring and selects qualified conditional bindings. */
export async function runExample(): Promise<string> {
  const Db = createToken<{ name: string }>('Db');
  const Replica = qualify(Db, 'replica');
  const Label = createToken<string>('Label');
  const root = new Container();
  try {
    root.singleton(Db, () => ({ name: 'primary' }), { deps: [] });
    root.singleton(Replica, () => ({ name: 'fallback' }), { deps: [] });
    root.singleton(Replica, () => ({ name: 'replica' }), {
      deps: [], when: ({ requester }) => requester?.tokenName === tokenName(Label),
    });
    root.singleton(Label, {
      inject: [Db, Replica] as const,
      factory: (primary, replica) => `${primary.name}/${replica.name}`,
    });
    root.validate({ requireDeps: true });
    const label = await root.resolve(Label);
    const fallback = await root.resolve(Replica);
    root.validate({ requireDeps: true, includeObserved: true });
    const snapshot = inspect(root);
    if (qualify(Db, 'replica') !== Replica) throw new Error('Qualified token was not interned');
    if (snapshot.dependencies.nodes.length === 0 || snapshot.resources.resources.length === 0) throw new Error('Empty inspection');
    return `${label}|top-level:${fallback.name}|issues:${snapshot.issues.length}`;
  } finally {
    await root.dispose();
  }
}
