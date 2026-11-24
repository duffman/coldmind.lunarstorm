/** @module @coldmind/devtools-shell/boundary — the error boundary around the app. */

import { Component, type ErrorInfo, type ReactNode } from "react";

import type { ShellErrors } from "./error-store.ts";
import { toShellError } from "./error-store.ts";

/** Props of {@link ShellBoundary}. */
export interface ShellBoundaryProps {
  /** Store the caught error is reported to. */
  errors: ShellErrors;
  /** Changing this value re-mounts the children after an error (the shell bumps it on a hot update). */
  resetKey: number;
  /** Rendered in place of the app while it is failed; `retry` re-mounts the app. Default: nothing. */
  fallback?: (error: Error, retry: () => void) => ReactNode;
  children: ReactNode;
}

interface ShellBoundaryState {
  error: Error | null;
}

/**
 * Catches errors thrown while rendering the app, reports them as `render` errors and renders
 * `fallback` until `resetKey` changes or `retry` is called. Errors in event handlers and async
 * code never reach a boundary; the shell catches those on `window`.
 */
export class ShellBoundary extends Component<ShellBoundaryProps, ShellBoundaryState> {
  override state: ShellBoundaryState = { error: null };

  static getDerivedStateFromError(error: unknown): ShellBoundaryState {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    this.props.errors.report({ ...toShellError("render", error), componentStack: info.componentStack ?? undefined });
  }

  // React Refresh re-mounts a failed boundary on its own, so a fresh instance never runs `retry`:
  // any commit without an error means the app renders again and stale `render` reports go.
  override componentDidMount(): void {
    if (!this.state.error) this.props.errors.clear("render");
  }

  override componentDidUpdate(previous: ShellBoundaryProps): void {
    if (!this.state.error) this.props.errors.clear("render");
    else if (previous.resetKey !== this.props.resetKey) this.retry();
  }

  /** Clears the caught error and its report, and renders the app again. */
  retry = (): void => {
    this.props.errors.clear("render");
    this.setState({ error: null });
  };

  override render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;
    return this.props.fallback ? this.props.fallback(error, this.retry) : null;
  }
}
