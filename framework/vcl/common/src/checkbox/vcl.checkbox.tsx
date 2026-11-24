/** CheckBox: a labelled boolean. */
import type { ReactNode } from "react";
import { Control, cx, prop } from "@coldmind/vcl.core";

export class CheckBox extends Control {
  @prop accessor caption = "";
  @prop accessor checked = false;
  onChange: ((sender: CheckBox) => unknown) | undefined;

  render(): ReactNode {
    return (
      <label className={cx("vcl-checkbox", `vcl-align-${this.align}`, this.className)}>
        <input
          type="checkbox"
          checked={this.checked}
          disabled={!this.enabled}
          onChange={e => {
            this.checked = e.currentTarget.checked;
            this.invoke(this.onChange, this);
          }}
        />
        <span>{this.caption}</span>
      </label>
    );
  }
}
