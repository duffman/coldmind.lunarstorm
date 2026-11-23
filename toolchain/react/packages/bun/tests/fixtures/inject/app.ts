import { Container } from "@coldmind/inject";
import { Clock, Logger } from "@coldmind/devtools/tests/fixtures/inject/services.ts";

const wired = <T>(target: T, _context: ClassDecoratorContext): T => target;

/** Its constructor types are the only dependency declaration. */
@wired
export class Greeter {
  constructor(readonly clock: Clock, readonly late: Late, readonly logger?: Logger) {}

  greet(): string {
    return `${this.late.name}@${this.clock.now()}`;
  }
}

/** Declared after `Greeter`: a static `$inject` field would hit the TDZ here, the getter does not. */
@wired
export class Late {
  readonly name = "late";
}

export async function run(): Promise<{ greeting: string; logger: boolean; inject: unknown[] }> {
  const root = new Container();
  root.singleton(Clock);
  root.singleton(Late);
  root.singleton(Greeter);
  const greeter = await root.resolve(Greeter);
  return {
    greeting: greeter.greet(),
    logger: greeter.logger !== undefined,
    inject: (Greeter as unknown as { $inject: unknown[] }).$inject,
  };
}
