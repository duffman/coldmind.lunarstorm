import { Container, composeModules, createToken, defineModule } from '@coldmind/inject';

/** Loads an async typed module graph once and unloads its owned registrations. */
export async function runExample(): Promise<string> {
  const Config = createToken<{ region: string }>('Config');
  const Label = createToken<string>('Label');
  let disposed = 0;
  const configuration = defineModule<{ config: { region: string } }, {}>()({
    name: 'configuration',
    register(c) { c.singleton(Config, () => ({ region: 'north' }), { deps: [] }); },
  });
  const labels = defineModule<{ label: string }, { config: { region: string } }>()({
    name: 'labels',
    imports: [configuration],
    async register(c) {
      await Promise.resolve();
      c.singleton(Label, {
        inject: [Config] as const,
        factory: config => `Region: ${config.region}`,
        onDispose: () => { disposed++; },
      });
    },
  });
  const composed = composeModules(configuration, labels);
  const root = new Container();
  try {
    const [handle] = await root.load(composed);
    if (!handle) throw new Error('Module load returned no handle');
    const label = await root.resolve(Label);
    await handle.unload();
    if (root.has(Label) || root.has(Config)) throw new Error('Unload retained module bindings');
    return `${label}|unloaded:${disposed}`;
  } finally {
    await root.dispose();
  }
}
