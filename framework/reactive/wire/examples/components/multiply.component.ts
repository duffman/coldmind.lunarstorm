import { ActionComponent, actionPort } from '@coldmind/wire';
import type { ActionComponentContext, ActionLifecycleOptions } from '@coldmind/wire';

/** Complete class template: replace port types and configure() for your component. */
export class MultiplyComponent extends ActionComponent<{ value: number }, { result: number }> {
  readonly #factor: number;
  constructor(factor: number, options: ActionLifecycleOptions = {}) {
    if (!Number.isFinite(factor)) throw new TypeError('factor must be finite');
    super({ name: 'multiply', inputs: { value: actionPort<number>() }, outputs: { result: actionPort<number>() } }, options);
    this.#factor = factor;
  }
  protected override configure(ctx: ActionComponentContext<{ value: number }, { result: number }>): void {
    ctx.outputs.result.connect(ctx.inputs.value.map(value => value * this.#factor));
  }
}
