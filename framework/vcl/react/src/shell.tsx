/**
 * The shells that wrap a generated form (spec 4, "Shell"): `<ShellDebug>` for dev
 * and debug, `<Shell>` for build and release. Both put an error boundary around
 * the generated form and give it its property mirror.
 *
 * Decisions:
 * - The shell owns the mirror's lifetime: one mirror per mounted form, disposed
 *   when the form unmounts, so a remount after `dev.refresh` starts from the
 *   host's current values and no listener leaks.
 * - The two shells differ only in what a render error shows. Debug shows the
 *   message and the stack; release shows one sentence, because a stack is of no
 *   use to an end user and can leak paths.
 */
import { Component, useEffect, useMemo, type ComponentType, type ErrorInfo, type ReactElement, type ReactNode } from 'react';
import { MirrorContext } from './control';
import { Mirror, type Bridge } from './mirror';

/** Props of both shells. */
export interface ShellProps {
  /** The shell's IPC client, `window.zebra`. */
  bridge: Bridge;
  /** The generated form components, by form name. */
  forms: Readonly<Record<string, ComponentType>>;
}

/** Props of the error boundary. */
interface BoundaryProps {
  /** Whether to show the error's details. */
  debug: boolean;
  /** The generated form. */
  children: ReactNode;
}

/** The error boundary around a generated form. */
class Boundary extends Component<BoundaryProps, { error: Error | null }> {
  /** The render error being shown, if any. */
  override state: { error: Error | null } = { error: null };

  /**
   * Switches to the error view after a render error.
   * @param error what was thrown
   * @return the state showing it
   */
  static getDerivedStateFromError(error: unknown): { error: Error } {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  /**
   * Logs the error with the component stack, for the WebView devtools.
   * @param error what was thrown
   * @param info where in the tree
   */
  override componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error('[vcl] the form failed to render', error, info.componentStack);
  }

  /** @return the form, or the error view */
  override render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="vcl-shell-error" role="alert">
        <strong>This window could not be displayed.</strong>
        {this.props.debug ? <pre>{error.stack ?? error.message}</pre> : null}
      </div>
    );
  }
}

/**
 * Renders the form of this WebView inside its mirror and an error boundary.
 * @param props the shell props, and whether this is the debug shell
 * @return the shell
 */
function ShellCore(props: ShellProps & { debug: boolean }): ReactElement {
  const { bridge, forms, debug } = props;
  const mirror = useMemo(() => new Mirror(bridge), [bridge]);
  useEffect(() => () => mirror.dispose(), [mirror]);
  const FormView = forms[bridge.windowId];
  return (
    <Boundary debug={debug}>
      <MirrorContext.Provider value={mirror}>
        {FormView ? <FormView /> : <UnknownForm name={bridge.windowId} />}
      </MirrorContext.Provider>
    </Boundary>
  );
}

/**
 * Fails rendering for a window whose form was not generated, so the boundary
 * reports it instead of showing an empty window.
 * @param props `name`: the window's form name
 */
function UnknownForm(props: { name: string }): never {
  throw new Error(`No form named '${props.name}' was generated for this window.`);
}

/** The shell of dev and debug builds: a render error shows its message and stack. */
export function ShellDebug(props: ShellProps): ReactElement {
  return <ShellCore {...props} debug />;
}

/** The shell of release builds: a render error shows one sentence. */
export function Shell(props: ShellProps): ReactElement {
  return <ShellCore {...props} debug={false} />;
}
