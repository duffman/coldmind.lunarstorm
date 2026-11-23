import type { Clock } from "@coldmind/devtools/tests/fixtures/inject/services.ts";

const wired = <T>(target: T, _context: ClassDecoratorContext): T => target;

@wired
export class Typed {
  constructor(readonly clock: Clock, readonly count: number) {}
}
