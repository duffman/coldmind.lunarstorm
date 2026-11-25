/**
 * DevApp: the parent app a build wraps around the user's app in development, `<DevApp><MyApp /></DevApp>`.
 * It gives the app an error boundary and docks the dev mode (console + API executor) below it.
 * Decision: the boundary reports into the dev console's capture store, so a render crash is listed with the other
 * errors, and the app's place shows a fallback with the error and a Retry that remounts the app.
 */
import { Component, useState, type ErrorInfo, type ReactNode } from "react";
import { capture } from "./capture.ts";
import { DevMode, type DevModeProps } from "./devmode.tsx";

/** Props of `DevApp`. */
export interface DevAppProps extends DevModeProps {
  /** The wrapped app. */
  children?: ReactNode;
}

/** Props of `Boundary`. */
interface BoundaryProps {
  /** The wrapped app. */
  children?: ReactNode;
  /** Remounts the app (changes its key). */
  onRetry: () => void;
}

/** State of `Boundary`: the caught error and React's component stack, or none. */
interface BoundaryState {
  /** The caught error. */
  error: unknown;
  /** Component stack of the failing render. */
  stack: string;
}

/**
 * The error boundary around the app.
 * Decision: a class component, since React has no hook form of `componentDidCatch`.
 */
class Boundary extends Component<BoundaryProps, BoundaryState> {
  /** No error until a render throws. */
  state: BoundaryState = { error: null, stack: "" };

  /**
   * Switches to the fallback on a render error.
   * @param error what was thrown
   * @return the new state
   */
  static getDerivedStateFromError(error: unknown): Partial<BoundaryState> {
    return { error };
  }

  /**
   * Records the error in the dev console capture with its component stack.
   * @param error what was thrown
   * @param info React's component stack
   */
  componentDidCatch(error: unknown, info: ErrorInfo): void {
    this.setState({ stack: info.componentStack ?? "" });
    capture.push("error", ["React render error:", error, info.componentStack ?? ""]);
  }

  /** Renders the app, or the fallback after a render error. */
  render(): ReactNode {
    const { error, stack } = this.state;
    if (error === null) return this.props.children;
    const message = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
    return (
      <div className="lsdm lsdm-crash" role="alert">
        <div className="lsdm-crash-card">
          <div className="lsdm-crash-title">The app crashed while rendering</div>
          <pre className="lsdm-crash-msg">{message}</pre>
          {stack && <pre className="lsdm-crash-stack">{stack.trim()}</pre>}
          <button className="lsdm-btn lsdm-crash-retry" onClick={() => { this.setState({ error: null, stack: "" }); this.props.onRetry(); }}>Retry</button>
        </div>
      </div>
    );
  }
}

/**
 * The dev parent app: error boundary around `children` plus the docked dev mode.
 * With `enabled={false}` it renders `children` alone, so a production build can keep the wrapper.
 * @param props dev mode options and the wrapped app
 */
export function DevApp({ children, ...devMode }: DevAppProps) {
  const [generation, setGeneration] = useState(0);
  if (devMode.enabled === false) return <>{children}</>;
  return (
    <>
      <Boundary key={generation} onRetry={() => setGeneration((g) => g + 1)}>{children}</Boundary>
      <DevMode {...devMode} />
    </>
  );
}
