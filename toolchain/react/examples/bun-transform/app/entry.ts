class Clock {
  now(): number {
    return Date.now();
  }
}

const service = <T>(target: T, _context: ClassDecoratorContext): T => target;

@service
class Report {
  constructor(readonly clock: Clock) {}
}

const mode: string = __MODE__;
console.log(`devermind example: ${mode}`, Report, Clock);
