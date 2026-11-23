/**
 * Application: owns the forms, mounts the React root and draws the screen (main form, open windows,
 * modal stack, unhandled errors). Lifecycle: `initialize` → `run` → `terminate`; closing the main
 * form terminates it.
 */
import { useEffect, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Component, ControlHost, installStyles, kernel, load, prop, useComponent } from "@coldmind/vcl.core";
import { Form, FormHost, messageOf } from "@coldmind/vcl.app/src/vcl.form.tsx";

export type FormClass<F extends Form = Form> = new (owner: Application) => F;

type Shown = { id: number; message: string };

export class Application extends FormHost {
  @prop accessor title = "";
  /** Exceptions from event handlers and render errors. Default: shown on screen and passed to the kernel's trace. */
  onException: ((error: unknown, sender: Component) => void) | undefined;
  /** The main form `run()` creates when it is called without one. */
  protected readonly mainFormClass: FormClass | undefined = undefined;

  #main: Form | undefined;
  #windows: Form[] = [];
  #modals: Form[] = [];
  #errors: Shown[] = [];
  #root: Root | undefined;
  #terminated: (() => void) | undefined;
  #terminating = false;

  constructor(name = "application") {
    super(undefined, name);
    kernel.register(this);
  }

  get mainForm(): Form | undefined {
    return this.#main;
  }

  set mainForm(form: Form | undefined) {
    this.#main = form;
    this.changed();
  }

  get forms(): readonly Form[] {
    return this.components.filter((c): c is Form => c instanceof Form);
  }

  /**
   * Constructs a form owned by this application, injects its tree from `kernel.container` and calls `loaded()` on it.
   * The first becomes the main form.
   */
  createForm<F extends Form>(FormClass: FormClass<F>): F {
    const form = new FormClass(this);
    load(form);
    this.#main ??= form;
    this.changed();
    return form;
  }

  /** Runs before the main form exists. */
  initialize(): void | Promise<void> {}

  /**
   * Creates the main form, mounts the root into `container` (default `#app`) and shows the main form.
   * Resolves when the application terminates.
   * @throws {Error} When there is no main form.
   */
  async run(MainForm: FormClass | undefined = this.mainFormClass, container?: Element): Promise<void> {
    await this.initialize();
    if (MainForm && !this.#main) this.createForm(MainForm);
    const main = this.#main;
    if (!main) throw new Error(`${this.describeName()}: run() needs a main form`);
    installStyles(document);
    const target = container ?? document.getElementById("app") ?? document.body.appendChild(document.createElement("div"));
    const finished = new Promise<void>(resolve => (this.#terminated = resolve));
    this.#root = createRoot(target);
    this.#root.render(<ApplicationHost application={this} />);
    main.show();
    return finished;
  }

  /** Unmounts the root, destroys every form and leaves the kernel. */
  terminate(): void {
    if (this.#terminating) return;
    this.#terminating = true;
    this.#root?.unmount();
    this.#root = undefined;
    this.destroy();
    kernel.unregister(this);
    this.#terminated?.();
  }

  showForm(form: Form, modal: boolean): void {
    if (form === this.#main) return;
    if (modal) this.#modals = [...this.#modals, form];
    else this.#windows = [...this.#windows, form];
    this.changed();
  }

  hideForm(form: Form): void {
    this.#windows = this.#windows.filter(f => f !== form);
    this.#modals = this.#modals.filter(f => f !== form);
    this.changed();
    if (form === this.#main) this.terminate();
  }

  override handleException(error: unknown, sender: Component = this): void {
    if (this.onException) {
      this.onException(error, sender);
      return;
    }
    kernel.onTrace?.({ kind: "exception", component: sender, error });
    console.error(`[${sender.name || sender.constructor.name}]`, error);
    this.#errors = [...this.#errors, { id: Date.now() + Math.random(), message: messageOf(error) }];
    this.changed();
  }

  dismissError(id: number): void {
    this.#errors = this.#errors.filter(e => e.id !== id);
    this.changed();
  }

  render(): ReactNode {
    return (
      <div className="vcl-screen">
        {this.#main && <ControlHost control={this.#main} />}
        {this.#windows.map(f => (
          <div key={f.id} className="vcl-window">
            <ControlHost control={f} />
          </div>
        ))}
        {this.#modals.map(f => (
          <div key={f.id} className="vcl-modal" onKeyDown={e => e.key === "Escape" && (f.modalResult = "cancel")}>
            <ControlHost control={f} />
          </div>
        ))}
        {this.#errors.length > 0 && (
          <div className="vcl-errors" role="alert">
            {this.#errors.map(e => (
              <div key={e.id} className="vcl-error">
                <span>{e.message}</span>
                <button className="vcl-form-close" aria-label="Dismiss" onClick={() => this.dismissError(e.id)}>
                  ×
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    );
  }
}

function ApplicationHost({ application }: { application: Application }): ReactNode {
  useComponent(application);
  const title = application.title;
  useEffect(() => {
    if (title) document.title = title;
  }, [title]);
  return application.render();
}
