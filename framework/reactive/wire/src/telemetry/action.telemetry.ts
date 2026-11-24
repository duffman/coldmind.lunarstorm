/** Log levels, lowest first. */
export type ActionLogLevel = 'debug' | 'info' | 'warn' | 'error';
/** How a span ended. */
export type ActionSpanStatus = 'ok' | 'error' | 'cancelled';
/** Flat, primitive-valued attributes for logs and spans; payloads are never captured. */
export type ActionAttributes = Readonly<Record<string, string | number | boolean>>;

/** @internal Error summaries avoid retaining exception objects or stacks. */
export function actionErrorAttributes(error: unknown): ActionAttributes {
  try {
    if (error instanceof Error) return { errorName: error.name.slice(0, 128), errorMessage: error.message.slice(0, 1024) };
    if (typeof error === 'string') return { errorMessage: error.slice(0, 1024) };
  } catch {}
  return { errorType: typeof error };
}

/** W3C-style trace ids: 32 and 16 lowercase hex digits, never all zeros. Travels as `meta.trace`. */
export interface ActionTraceContext {
  readonly traceId: string;
  readonly spanId: string;
}

/** What `onLog` receives. */
export interface ActionLogRecord {
  readonly timestamp: string;
  readonly level: ActionLogLevel;
  readonly message: string;
  readonly attributes: ActionAttributes;
  readonly context?: ActionTraceContext;
}

/** What `onSpan` receives when a span ends. */
export interface ActionSpanRecord extends ActionTraceContext {
  readonly name: string;
  readonly parentSpanId?: string;
  readonly startedAt: string;
  readonly durationMs: number;
  readonly status: ActionSpanStatus;
  readonly attributes: ActionAttributes;
}

/** Aggregate of all ended spans of one name; what `onMetric` receives after each. */
export interface ActionMetric {
  readonly name: string;
  readonly count: number;
  readonly errors: number;
  readonly cancelled: number;
  readonly totalMs: number;
  readonly minMs: number;
  readonly maxMs: number;
  readonly meanMs: number;
}

/** The sinks; a failing sink goes to `onDiagnostic`, never to the caller. */
export interface ActionTelemetryOptions {
  readonly onLog?: (record: ActionLogRecord) => unknown;
  readonly onSpan?: (record: ActionSpanRecord) => unknown;
  readonly onMetric?: (metric: ActionMetric) => unknown;
  readonly onDiagnostic?: (error: unknown) => unknown;
  /** Logs below this level are dropped. Default "info". */
  readonly minLevel?: ActionLogLevel;
  /** Bounds unique metric names; spans/logs stream directly to your sinks. */
  readonly maxMetrics?: number;
}

/** A parent continues its trace; without one the span starts a new trace. */
export interface ActionSpanOptions {
  readonly parent?: ActionTraceContext;
  readonly attributes?: ActionAttributes;
}

const levels: Record<ActionLogLevel, number> = { debug: 0, info: 1, warn: 2, error: 3 };

function boundedName(name: string): string {
  if (!name.trim()) throw new TypeError('Span name must be nonempty');
  if (name.length <= 256) return name;
  let hash = 14695981039346656037n;
  for (let index = 0; index < name.length; index++) hash = BigInt.asUintN(64, (hash ^ BigInt(name.charCodeAt(index))) * 1099511628211n);
  return name.slice(0, 239) + '.' + hash.toString(16).padStart(16, '0');
}

function id(bytes: number): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), byte => byte.toString(16).padStart(2, '0')).join('');
}

/** True for a well-formed trace context; use it on anything that arrived over the network. */
export function isActionTraceContext(value: unknown): value is ActionTraceContext {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Record<string, unknown>;
  return typeof candidate.traceId === 'string' && /^[0-9a-f]{32}$/.test(candidate.traceId) &&
    !/^0+$/.test(candidate.traceId) && typeof candidate.spanId === 'string' &&
    /^[0-9a-f]{16}$/.test(candidate.spanId) && !/^0+$/.test(candidate.spanId);
}

/** A span ends once. Explicit parents keep unrelated concurrent work isolated. */
export class ActionSpan {
  readonly context: ActionTraceContext;
  readonly #telemetry: ActionTelemetry;
  readonly #name: string;
  readonly #options: ActionSpanOptions;
  readonly #startedAt = new Date().toISOString();
  readonly #started = performance.now();
  #ended = false;

  constructor(telemetry: ActionTelemetry, name: string, options: ActionSpanOptions = {}) {
    if (options.parent !== undefined && !isActionTraceContext(options.parent)) throw new TypeError('Invalid parent trace context');
    this.#telemetry = telemetry;
    this.#name = boundedName(name);
    this.#options = {
      ...options,
      ...(options.parent ? { parent: Object.freeze({ ...options.parent }) } : {}),
      attributes: Object.freeze({ ...options.attributes }),
    };
    this.context = Object.freeze({ traceId: options.parent?.traceId ?? id(16), spanId: id(8) });
  }

  /** A log record carrying this span's context. */
  log(level: ActionLogLevel, message: string, attributes: ActionAttributes = {}): void {
    this.#telemetry.log(level, message, attributes, this.context);
  }

  /** Records the span; later calls do nothing. */
  end(status: ActionSpanStatus = 'ok'): void {
    if (this.#ended) return;
    this.#ended = true;
    this.#telemetry.record(Object.freeze({
      name: this.#name,
      ...this.context,
      ...(this.#options.parent ? { parentSpanId: this.#options.parent.spanId } : {}),
      startedAt: this.#startedAt,
      durationMs: Math.max(0, performance.now() - this.#started),
      status,
      attributes: this.#options.attributes ?? {},
    }));
  }
}

/** No global configuration, runtime imports, automatic payload capture, or retained span history. */
export class ActionTelemetry {
  readonly #options: ActionTelemetryOptions;
  readonly #metrics = new Map<string, ActionMetric>();
  readonly #maxMetrics: number;
  #dropped = 0;

  constructor(options: ActionTelemetryOptions = {}) {
    const max = options.maxMetrics ?? 256;
    if (!Number.isSafeInteger(max) || max < 1 || max > 100_000) throw new RangeError('maxMetrics must be 1–100000');
    if (options.minLevel !== undefined && !(options.minLevel in levels)) throw new TypeError('Invalid log level');
    for (const key of ['onLog', 'onSpan', 'onMetric', 'onDiagnostic'] as const) {
      if (options[key] !== undefined && typeof options[key] !== 'function') throw new TypeError(key + ' must be a function');
    }
    this.#options = Object.freeze({ ...options });
    this.#maxMetrics = max;
  }

  /** Span names not aggregated because `maxMetrics` was reached. */
  get droppedMetrics(): number { return this.#dropped; }

  #diagnostic(error: unknown): void {
    try { void Promise.resolve(this.#options.onDiagnostic?.(error)).catch(() => {}); } catch {}
  }

  #send<T>(sink: ((record: T) => unknown) | undefined, record: T): void {
    if (!sink) return;
    try { void Promise.resolve(sink(record)).catch(error => this.#diagnostic(error)); }
    catch (error) { this.#diagnostic(error); }
  }

  /** Sends a record to `onLog` when `level` is at or above `minLevel`. */
  log(level: ActionLogLevel, message: string, attributes: ActionAttributes = {}, context?: ActionTraceContext): void {
    if (levels[level] < levels[this.#options.minLevel ?? 'info']) return;
    this.#send(this.#options.onLog, Object.freeze({
      timestamp: new Date().toISOString(), level, message,
      attributes: Object.freeze({ ...attributes }),
      ...(context ? { context: Object.freeze({ ...context }) } : {}),
    }));
  }

  /** A running span; nothing is recorded until `end()`. */
  startSpan(name: string, options: ActionSpanOptions = {}): ActionSpan {
    return new ActionSpan(this, name, options);
  }

  /** Measures asynchronous completion rather than just promise creation. */
  async measure<T>(name: string, work: (span: ActionSpan) => T | PromiseLike<T>, options: ActionSpanOptions = {}): Promise<T> {
    const span = this.startSpan(name, options);
    try { const result = await work(span); span.end(); return result; }
    catch (error) { span.end('error'); throw error; }
  }

  /** @internal Completed spans update aggregates before being sent to exporters. */
  record(span: ActionSpanRecord): void {
    const previous = this.#metrics.get(span.name);
    if (!previous && this.#metrics.size >= this.#maxMetrics) this.#dropped++;
    else {
      const count = (previous?.count ?? 0) + 1;
      const totalMs = (previous?.totalMs ?? 0) + span.durationMs;
      const metric = Object.freeze({
        name: span.name, count, totalMs, meanMs: totalMs / count,
        errors: (previous?.errors ?? 0) + Number(span.status === 'error'),
        cancelled: (previous?.cancelled ?? 0) + Number(span.status === 'cancelled'),
        minMs: Math.min(previous?.minMs ?? Infinity, span.durationMs),
        maxMs: Math.max(previous?.maxMs ?? 0, span.durationMs),
      });
      this.#metrics.set(span.name, metric);
      this.#send(this.#options.onMetric, metric);
    }
    this.#send(this.#options.onSpan, span);
  }

  /** A snapshot of the aggregates, one per span name. */
  metrics(): readonly ActionMetric[] { return Object.freeze([...this.#metrics.values()]); }
  /** Clears the aggregates and the dropped count. */
  resetMetrics(): void { this.#metrics.clear(); this.#dropped = 0; }
}
