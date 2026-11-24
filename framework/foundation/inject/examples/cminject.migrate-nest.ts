import { Container, createToken, defineModule } from '@coldmind/inject';

/** Migrates a module factory, alias and explicit request boundary without framework dependencies. */
export async function runExample(): Promise<string> {
  const Options = createToken<{ prefix: string }>('Options');
  const Greeting = createToken<{ greet(name: string): string }>('Greeting');
  const Alias = createToken<{ greet(name: string): string }>('Alias');
  const Names = createToken<string[]>('Names');
  const greetings = defineModule({
    name: 'greetings',
    register(c) {
      c.singleton(Options, () => ({ prefix: 'Hello' }), { deps: [] });
      c.singleton(Greeting, {
        inject: [Options] as const,
        factory: options => ({ greet: (name: string) => `${options.prefix} ${name}` }),
      });
      c.singleton(Alias, { inject: [Greeting] as const, factory: greeting => greeting });
      c.request(Names, () => [], { deps: [] });
    },
  });
  const root = new Container();
  try {
    await root.load(greetings);
    root.validate({ requireDeps: true });
    const service = await root.resolve(Greeting);
    if (await root.resolve(Alias) !== service) throw new Error('Alias changed service identity');
    const first = root.createRequestScope('first');
    const second = root.createRequestScope('second');
    (await first.resolve(Names)).push('Ada');
    const isolated = (await second.resolve(Names)).length === 0;
    return `${service.greet((await first.resolve(Names))[0]!)}|alias:true|request-isolated:${isolated}`;
  } finally {
    await root.dispose();
  }
}
