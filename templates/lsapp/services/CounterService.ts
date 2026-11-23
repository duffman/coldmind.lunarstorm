import { Service } from '../common';

/** Application-scoped state for this running session. */
@Service
export class CounterService {
  private count = 0;

  get value(): number { return this.count; }

  increment(): number {
    return ++this.count;
  }

  reset(): void {
    this.count = 0;
  }
}
