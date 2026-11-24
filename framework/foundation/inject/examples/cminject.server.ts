import { Container, createToken } from '@coldmind/inject';
import { currentContext, DiRuntime } from '@coldmind/inject/server';

/** Acquires request leases explicitly and releases each before draining the runtime. */
export async function runExample(): Promise<string> {
  const Region = createToken<string>('Region');
  const Events = createToken<string[]>('Events');
  const root = new Container();
  const runtime = new DiRuntime<string>(root, {
    configureTenant(id, tenant) { tenant.singleton(Region, () => id, { deps: [] }); },
    eagerRequest: false,
  });
  let disposed = 0;
  let result = '';
  try {
    root.request(Events, { factory: () => [], onDispose: () => { disposed++; } }, { deps: [] });
    await runtime.start();
    const lease = await runtime.acquireRequest('north');
    try {
      result = await lease.run(async request => {
        const events = await request.resolve(Events);
        events.push(await request.resolve(Region));
        return `${events.join()}|context:${currentContext()?.identity.id === request.identity.id}`;
      });
    } finally {
      await lease.release();
    }
    await runtime.withRequest('north', async request => {
      if ((await request.resolve(Events)).length !== 0) throw new Error('Request data leaked');
    });
    await runtime.drain();
  } finally {
    await runtime.dispose();
  }
  return `${result}|request-disposed:${disposed}`;
}
