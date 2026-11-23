# Migrating TSyringe wiring

TSyringe 4.10 class/value/factory/token providers map to class registrations, registerInstance, factories and explicit aliases. ResolutionScoped (one resolution chain) is not cminject scoped (one resolving container). TSyringe delay uses a transparent proxy; cminject lazy injects an explicit resolver object and delay only delays token lookup. cminject clearInstances preserves supplied values; TSyringe 4.10 removes value-provider registrations when clearing. See [pinned official providers and scopes](https://github.com/microsoft/tsyringe/blob/e033769d97cfb6cc4a8569e2b50eb32015453302/README.md) and [clearInstances implementation](https://github.com/microsoft/tsyringe/blob/e033769d97cfb6cc4a8569e2b50eb32015453302/src/dependency-container.ts#L405-L420).

## Runnable example

[Source](../examples/cminject.migrate-tsyringe.ts). This example is part of the final docs test suite.

```ts
import { Container, createToken } from '@coldmind/inject';

/** Replaces global wiring with explicit roots, static dependencies and child-scoped caches. */
export async function runExample(): Promise<string> {
  interface Logger { log(message: string): void }
  const Logger = createToken<Logger>('Logger');
  const messages: string[] = [];
  let externalDisposals = 0;
  class Worker {
    static $inject = [Logger];
    constructor(readonly logger: Logger) {}
    run(): void { this.logger.log('done'); }
  }
  const logger = {
    log(message: string): void { messages.push(message); },
    onDispose(): void { externalDisposals++; },
  };
  const root = new Container();
  let isolated = false;
  try {
    root.registerInstance(Logger, logger, { ownership: 'external', reason: 'Example host owns logger' });
    root.scoped(Worker);
    const first = root.createScope();
    const second = root.createScope();
    const worker = await first.resolve(Worker);
    worker.run();
    isolated = worker !== await second.resolve(Worker);
    if (worker !== await first.resolve(Worker) || !isolated) throw new Error('Wrong scoped cache boundary');
  } finally {
    await root.dispose();
  }
  return `${messages.join()}|isolated:${isolated}|external-disposed:${externalDisposals}`;
}
```

[API reference](api/index.html) · [Getting started](getting-started.md)
