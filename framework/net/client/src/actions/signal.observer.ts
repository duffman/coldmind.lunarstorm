import type { ActionTelemetry, ActionAttributes } from '@coldmind/wire';
import type { DynObserver } from '@coldmind/bus';

function errorAttributes(error: unknown): ActionAttributes {
  try { if (error instanceof Error) return { errorName: error.name.slice(0, 128), errorMessage: error.message.slice(0, 1024) }; }
  catch {}
  return { errorType: typeof error };
}

/** Install once at application wiring. Existing observers and async exporters cannot break bus control flow. */
export function createSignalObserver(telemetry: ActionTelemetry, previous: DynObserver = {}): DynObserver {
  const invoke = (work: () => unknown) => {
    const failure = (error: unknown) => telemetry.log('error', 'SignalBus observer failed', errorAttributes(error));
    try { void Promise.resolve(work()).catch(failure); } catch (error) { failure(error); }
  };
  return Object.freeze<DynObserver>({
    onAccepted: data => {
      telemetry.log('debug', 'SignalBus accepted', { ...data });
      invoke(() => previous.onAccepted?.(data));
    },
    onQueued: data => {
      telemetry.log('debug', 'SignalBus queued', { ...data });
      invoke(() => previous.onQueued?.(data));
    },
    onDelivered: data => {
      telemetry.log('debug', 'SignalBus delivered', { topic: data.topic, messageId: data.messageId, subId: data.subId, latencyMs: data.latency });
      invoke(() => previous.onDelivered?.(data));
    },
    onError: error => {
      telemetry.log('error', 'SignalBus error', { type: error.type, topic: error.topic, messageId: error.messageId, ...errorAttributes(error.error) });
      invoke(() => previous.onError?.(error));
    },
    onDeadLetter: entry => {
      telemetry.log('error', 'SignalBus dead letter', { topic: entry.message.topic, messageId: entry.message.id, attempts: entry.attempts, ...errorAttributes(entry.error) });
      invoke(() => previous.onDeadLetter?.(entry));
    },
  });
}
