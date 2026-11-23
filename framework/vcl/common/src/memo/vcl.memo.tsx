/** Memo: multi-line text (Delphi's TMemo). `lines` reads and writes `text` split on newlines. */
import type { ReactNode } from "react";
import { Control, cx, prop } from "@coldmind/vcl.core";

export class Memo extends Control {
  @prop accessor text = "";
  @prop accessor placeholder = "";
  @prop accessor readOnly = false;
  @prop accessor rows = 4;
  onChange: ((sender: Memo) => unknown) | undefined;

  get lines(): readonly string[] {
    return this.text === "" ? [] : this.text.split("\n");
  }

  set lines(lines: readonly string[]) {
    this.text = lines.join("\n");
  }

  render(): ReactNode {
    return (
      <textarea
        className={cx("vcl-edit", "vcl-memo", `vcl-align-${this.align}`, this.className)}
        value={this.text}
        rows={this.rows}
        placeholder={this.placeholder || undefined}
        aria-label={this.placeholder || this.name || undefined}
        readOnly={this.readOnly}
        disabled={!this.enabled}
        onChange={e => {
          this.text = e.currentTarget.value;
          this.invoke(this.onChange, this);
        }}
      />
    );
  }
}
