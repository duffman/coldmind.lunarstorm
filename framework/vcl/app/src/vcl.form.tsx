/**
 * Form: a top-level control. `show()` places it on screen, `showModal()` resolves with its
 * `ModalResult` when it closes, `close()` asks `onCloseQuery` and hides it. Render errors inside the
 * form are caught by its own boundary and routed to `handleException`.
 */
import { Component as ReactComponent, type ReactNode } from "react";
import { Component, Control, cx, prop } from "@coldmind/vcl.core";

export type ModalResult = "none" | "ok" | "cancel" | "yes" | "no" | "abort" | "retry" | "close";
/** What `close()` does after `onClose`: hide the form, destroy it, or keep it open. */
export type CloseAction = "hide" | "free" | "none";

/** What a form is shown on: the `Application`. Declared here so the form module does not import it. */
export abstract class FormHost extends Component {
  abstract showForm(form: Form, modal: boolean): void;
  abstract hideForm(form: Form): void;
}

export class Form extends Control {
  @prop accessor caption = "";
  onShow: ((sender: Form) => unknown) | undefined;
  /** Return false to keep the form open. */
  onCloseQuery: ((sender: Form) => boolean) | undefined;
  /** Gets the default action (`"hide"`) and may return another. */
  onClose: ((sender: Form, action: CloseAction) => CloseAction | void) | undefined;

  #modalResult: ModalResult = "none";
  #resolveModal: ((result: ModalResult) => void) | undefined;

  constructor(owner?: Component, name = "") {
    super(owner, name);
    this.visible = false;
  }

  get modal(): boolean {
    return this.#resolveModal !== undefined;
  }

  get modalResult(): ModalResult {
    return this.#modalResult;
  }

  /** Setting anything but `"none"` on a modal form closes it, as in Delphi. */
  set modalResult(result: ModalResult) {
    if (result === this.#modalResult) return;
    this.#modalResult = result;
    this.changed();
    if (result !== "none" && this.modal) this.close();
  }

  /** @throws {Error} When no `FormHost` owns the form. */
  get host(): FormHost {
    for (let o = this.owner; o; o = o.owner) if (o instanceof FormHost) return o;
    throw new Error(`${this.describeName()}: a form must be owned by an Application`);
  }

  show(): void {
    if (this.visible) return;
    this.host.showForm(this, false);
    this.visible = true;
    this.invoke(this.onShow, this);
  }

  /** @throws {Error} When the form is already visible. */
  showModal(): Promise<ModalResult> {
    if (this.visible) return Promise.reject(new Error(`${this.describeName()}: showModal on a visible form`));
    this.#modalResult = "none";
    return new Promise(resolve => {
      this.#resolveModal = resolve;
      this.host.showForm(this, true);
      this.visible = true;
      this.invoke(this.onShow, this);
    });
  }

  close(): void {
    if (!this.visible) return;
    if (this.onCloseQuery && !this.onCloseQuery(this)) {
      this.#modalResult = "none";
      return;
    }
    const action = this.onClose?.(this, "hide") ?? "hide";
    if (action === "none") {
      this.#modalResult = "none";
      return;
    }
    this.visible = false;
    this.settleModal();
    this.host.hideForm(this);
    if (action === "free") this.destroy();
  }

  override destroy(): void {
    if (this.destroyed) return;
    this.settleModal();
    if (this.visible) this.host.hideForm(this);
    super.destroy();
  }

  render(): ReactNode {
    return (
      <section
        className={cx("vcl-form", this.className)}
        role={this.modal ? "dialog" : undefined}
        aria-modal={this.modal || undefined}
        aria-label={this.caption || undefined}
      >
        {this.caption && (
          <header className="vcl-form-caption">
            <span>{this.caption}</span>
            {this.modal && (
              <button className="vcl-form-close" aria-label="Close" onClick={() => (this.modalResult = "cancel")}>
                ×
              </button>
            )}
          </header>
        )}
        <FormBoundary form={this}>{this.renderClient()}</FormBoundary>
      </section>
    );
  }

  /** The area inside the caption. Default: the child controls in a column. */
  protected renderClient(): ReactNode {
    return <div className="vcl-form-client">{this.renderControls()}</div>;
  }

  private settleModal(): void {
    const resolve = this.#resolveModal;
    this.#resolveModal = undefined;
    resolve?.(this.#modalResult === "none" ? "cancel" : this.#modalResult);
  }
}

type BoundaryState = { failed: boolean; error: unknown };

/** React needs a class for an error boundary; this one belongs to one form. */
class FormBoundary extends ReactComponent<{ form: Form; children: ReactNode }, BoundaryState> {
  override state: BoundaryState = { failed: false, error: undefined };

  static getDerivedStateFromError(error: unknown): BoundaryState {
    return { failed: true, error };
  }

  override componentDidCatch(error: unknown): void {
    this.props.form.handleException(error);
  }

  override render(): ReactNode {
    if (!this.state.failed) return this.props.children;
    return (
      <div className="vcl-form-error" role="alert">
        <p>{messageOf(this.state.error)}</p>
        <button className="vcl-button" onClick={() => this.setState({ failed: false, error: undefined })}>
          Retry
        </button>
      </div>
    );
  }
}

export const messageOf = (error: unknown): string => (error instanceof Error ? error.message : String(error));
