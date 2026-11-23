import { ActionCore } from '@coldmind/wire/src/core/action.core.ts';
import { validateMilliseconds } from '@coldmind/wire/src/core/action.errors.ts';
import type { Scope } from '@coldmind/wire/src/core/action.scope.ts';
import type { AsyncMapper, DataHandler, Equality } from '@coldmind/wire/src/core/action.types.ts';
import type { ActionSpan, ActionTraceContext } from '@coldmind/wire/src/telemetry/action.telemetry.ts';

type Consumer<T> = (value: T) => void;

interface AsyncOperation<T> {
  span: ActionSpan | undefined;
  target: ActionCore<T> | undefined;
  controller: AbortController | undefined;
  finish: (() => void) | undefined;
}

function cancelOperation<T>(operation: AsyncOperation<T> | undefined): void {
  if (!operation) return;
  const controller = operation.controller;
  operation.span?.end('cancelled');
  operation.span = undefined;
  operation.target = undefined;
  operation.controller = undefined;
  operation.finish = undefined;
  controller?.abort();
}

function finishOperation<T>(operation: AsyncOperation<T>, failed = false): ActionCore<T> | undefined {
  operation.span?.end(failed ? 'error' : 'ok');
  // Exporters can synchronously deactivate this operator and replace its activation.
  const target = operation.target;
  operation.span = undefined;
  operation.finish?.();
  operation.target = undefined;
  operation.controller = undefined;
  operation.finish = undefined;
  return target;
}

/** Reactions retain only a clearable cell, never an operator activation scope. */
function observeOperation<T>(promise: PromiseLike<T>, operation: AsyncOperation<T>): void {
  void Promise.resolve(promise).then(
    value => {
      const context = operation.span?.context;
      finishOperation(operation)?.publish(value, context);
    },
      error => finishOperation(operation, true)?.error(error),
  );
}

/** One shared operator state per activation, with exclusively downstream ownership. */
function derive<T, R>(
  source: ActionCore<T>, operator: string, initialize: (output: ActionCore<R>, scope: Scope) => Consumer<T>,
): ActionCore<R> {
  source.assertActive();
  const output: ActionCore<R> = new ActionCore({ ...source.options, name: (source.options.name ?? 'action') + '.' + operator }, scope => {
    const consume = initialize(output, scope);
    scope.add(source.observe({
      data: value => {
        if (scope.closed || output.disposed) return;
        try { consume(value); }
        catch (error) { if (!output.disposed) output.error(error); }
      },
      error: error => output.forwardError(error),
      end: () => output.dispose(),
    }));
  });
  output.own(output.dependency(source));
  return output;
}

export function mapAction<T, R>(source: ActionCore<T>, map: (value: T) => R): ActionCore<R> {
  return derive(source, 'map', output => value => output.publish(map(value), source.context));
}

export function filterAction<T>(source: ActionCore<T>, predicate: (value: T) => boolean): ActionCore<T> {
  return derive(source, 'filter', output => value => { if (predicate(value)) output.publish(value, source.context); });
}

export function tapAction<T>(source: ActionCore<T>, effect: DataHandler<T>): ActionCore<T> {
  return derive(source, 'tap', output => value => {
    output.trackResult(effect(value));
    output.publish(value, source.context);
  });
}

export function distinctAction<T>(source: ActionCore<T>, equals: Equality<T> = Object.is): ActionCore<T> {
  return derive(source, 'distinct', output => {
    let previous: { value: T } | undefined;
    return value => {
      if (previous && equals(previous.value, value)) return;
      previous = { value };
      output.publish(value, source.context);
    };
  });
}

export function debounceAction<T>(source: ActionCore<T>, ms: number): ActionCore<T> {
  source.assertActive();
  validateMilliseconds(ms);
  return derive(source, 'debounce', (output, scope) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let pending: { value: T; context: ActionTraceContext | undefined } | undefined;
    scope.add(() => {
      clearTimeout(timer);
      timer = undefined;
      pending = undefined;
    });
    return value => {
      clearTimeout(timer);
      pending = { value, context: source.context };
      timer = setTimeout(() => {
        const item = pending;
        pending = undefined;
        timer = undefined;
        if (!scope.closed && item) output.publish(item.value, item.context);
      }, ms);
    };
  });
}

export function switchMapAction<T, R>(source: ActionCore<T>, map: AsyncMapper<T, R>): ActionCore<R> {
  return derive(source, 'switchMap', (output, scope) => {
    let current: AsyncOperation<R> | undefined;
    scope.add(() => {
      const previous = current;
      current = undefined;
      cancelOperation(previous);
    });
    return value => {
      const controller = new AbortController();
      const operation: AsyncOperation<R> = {
        span: output.span('work', source.context),
        target: output,
        controller,
        finish: () => { if (current === operation) current = undefined; },
      };
      const previous = current;
      current = operation;
      cancelOperation(previous);
      // Abort listeners are user code and can synchronously dispose this pipeline.
      if (scope.closed || current !== operation) return;
      let promise: PromiseLike<R>;
      try { promise = map(value, controller.signal); }
      catch (error) {
        if (!scope.closed && current === operation) {
          current = undefined;
          operation.span?.end('error');
          operation.span = undefined;
          cancelOperation(operation);
          output.error(error);
        }
        return;
      }
      observeOperation(promise, operation);
    };
  });
}

export function connectAction<T>(receiver: ActionCore<T>, source: ActionCore<T>): void {
  receiver.assertActive();
  source.assertActive();
  const detachDependency = receiver.dependency(source);
  receiver.attach(scope => {
    scope.add(detachDependency);
    scope.add(source.observe({
      data: value => receiver.publish(value, source.context),
      error: error => receiver.forwardError(error),
      end: () => scope.close(),
    }));
  });
}

export function mergeActions<T>(sources: readonly ActionCore<T>[]): ActionCore<T> {
  const output: ActionCore<T> = new ActionCore(sources[0]?.options, scope => {
    let remaining = sources.length;
    if (remaining === 0) { output.dispose(); return; }
    for (const source of sources) {
      if (scope.closed) break;
      scope.add(source.observe({
        data: value => output.publish(value, source.context),
        error: error => output.forwardError(error),
        end: () => { if (--remaining === 0) output.dispose(); },
      }));
    }
  });
  for (const source of sources) output.own(output.dependency(source));
  if (sources.length === 0 || sources.every(source => source.disposed)) output.dispose();
  return output;
}
