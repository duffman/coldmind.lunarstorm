/** TextEdit: a single-line input (Delphi's TEdit). `text` is the value; typing sets it. */
import type { ReactNode } from "react";
import { Control, cx, prop } from "@coldmind/vcl.core";

export class TextEdit extends Control {
  @prop accessor text = "";
  @prop accessor placeholder = "";
  @prop accessor password = false;
  @prop accessor readOnly = false;
  @prop accessor maxLength = 0;
  /** Focus when first rendered. */
  @prop accessor autoFocus = false;
  onChange: ((sender: TextEdit) => unknown) | undefined;
  onEnter: ((sender: TextEdit) => unknown) | undefined;

  render(): ReactNode {
    return (
      <input
        className={cx("vcl-edit", `vcl-align-${this.align}`, this.className)}
        type={this.password ? "password" : "text"}
        value={this.text}
        placeholder={this.placeholder || undefined}
        aria-label={this.placeholder || this.name || undefined}
        readOnly={this.readOnly}
        maxLength={this.maxLength > 0 ? this.maxLength : undefined}
        autoFocus={this.autoFocus}
        disabled={!this.enabled}
        onChange={e => {
          this.text = e.currentTarget.value;
          this.invoke(this.onChange, this);
        }}
        onKeyDown={e => {
          if (e.key === "Enter") this.invoke(this.onEnter, this);
        }}
      />
    );
  }
}
