export class Clock {
  now(): number {
    return 42;
  }
}

export class Logger {
  readonly lines: string[] = [];
}
