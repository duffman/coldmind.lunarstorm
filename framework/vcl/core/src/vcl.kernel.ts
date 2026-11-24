/**
 * The kernel: one per process. Registry of running applications, the root injector, an in-process bus,
 * the last stop for exceptions no application handled, and a trace hook.
 */
import { Container } from "@coldmind/inject";
import { SignalBus } from "@coldmind/bus";
import type { Component } from "@coldmind/vcl.core/src/vcl.component.ts";

export type KernelEvent =
  | { kind: "register" | "unregister"; component: Component }
  | { kind: "exception"; component: Component; error: unknown };

export class Kernel {
  static readonly instance = new Kernel();

  /** Root injector; applications and forms resolve shared services from it. */
  readonly container = new Container();
  /** In-process bus shared by every application in this process. */
  readonly bus = new SignalBus();
  /** Exceptions no application handled. Default: `console.error`. */
  onException: ((error: unknown, sender: Component) => void) | undefined;
  /** Every registration and exception, for tracing. */
  onTrace: ((event: KernelEvent) => void) | undefined;

  readonly #applications = new Set<Component>();

  get applications(): readonly Component[] {
    return [...this.#applications];
  }

  register(application: Component): void {
    this.#applications.add(application);
    this.onTrace?.({ kind: "register", component: application });
  }

  unregister(application: Component): void {
    if (this.#applications.delete(application)) this.onTrace?.({ kind: "unregister", component: application });
  }

  handleException(error: unknown, sender: Component): void {
    this.onTrace?.({ kind: "exception", component: sender, error });
    if (this.onException) this.onException(error, sender);
    else console.error(`[${sender.name || sender.constructor.name}]`, error);
  }
}

export const kernel = Kernel.instance;
