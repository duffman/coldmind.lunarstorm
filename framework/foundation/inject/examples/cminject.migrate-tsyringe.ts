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
