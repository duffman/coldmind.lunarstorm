/**
 * Button. A `modalResult` other than `"none"` sets it on the button's form when clicked, which closes
 * a modal form, as in Delphi.
 */
import type { ReactNode } from "react";
import { Control, cx, prop } from "@coldmind/vcl.core";
import { Form, type ModalResult } from "@coldmind/vcl.app";

export class Button extends Control {
  @prop accessor caption = "";
  @prop accessor kind: "normal" | "primary" = "normal";
  @prop accessor modalResult: ModalResult = "none";
  onClick: ((sender: Button) => unknown) | undefined;

  click(): void {
    if (!this.enabled) return;
    this.invoke(this.onClick, this);
    if (this.modalResult === "none") return;
    for (let p = this.parent; p; p = p.parent)
      if (p instanceof Form) {
        p.modalResult = this.modalResult;
        return;
      }
  }

  render(): ReactNode {
    return (
      <button
        type="button"
        className={cx("vcl-button", this.kind === "primary" && "vcl-button-primary", `vcl-align-${this.align}`, this.className)}
        disabled={!this.enabled}
        onClick={() => this.click()}
      >
        {this.caption}
      </button>
    );
  }
}
