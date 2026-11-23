/** Label: one line of text. */
import type { ReactNode } from "react";
import { Control, cx, prop } from "@coldmind/vcl.core";

export class Label extends Control {
  @prop accessor caption = "";
  @prop accessor tone: "normal" | "muted" | "danger" = "normal";

  render(): ReactNode {
    return (
      <span className={cx("vcl-label", `vcl-label-${this.tone}`, `vcl-align-${this.align}`, this.className)}>
        {this.caption}
      </span>
    );
  }
}
