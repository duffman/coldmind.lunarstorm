/**
 * ListBox: a list with one selected item. Assign a new `items` array to change it (the identity
 * change is what notifies). Arrow keys move the selection, Enter and double-click fire `onActivate`.
 */
import type { KeyboardEvent, ReactNode } from "react";
import { Control, cx, prop } from "@coldmind/vcl.core";

export class ListBox<T = string> extends Control {
  /** -1 when nothing is selected. Kept in range when `items` shrinks. */
  @prop accessor itemIndex = -1;
  @prop accessor emptyText = "";
  format: (item: T) => string = String;
  onSelect: ((sender: ListBox<T>) => unknown) | undefined;
  onActivate: ((sender: ListBox<T>) => unknown) | undefined;

  #items: readonly T[] = [];

  get items(): readonly T[] {
    return this.#items;
  }

  set items(items: readonly T[]) {
    if (items === this.#items) return;
    this.beginUpdate();
    try {
      this.#items = items;
      this.changed();
      if (this.itemIndex >= items.length) this.itemIndex = items.length - 1;
    } finally {
      this.endUpdate();
    }
  }

  get selected(): T | undefined {
    return this.itemIndex >= 0 ? this.items[this.itemIndex] : undefined;
  }

  select(index: number): void {
    const next = Math.max(-1, Math.min(index, this.items.length - 1));
    if (next === this.itemIndex) return;
    this.itemIndex = next;
    this.invoke(this.onSelect, this);
  }

  render(): ReactNode {
    return (
      <ul
        className={cx("vcl-listbox", `vcl-align-${this.align}`, this.className)}
        role="listbox"
        tabIndex={this.enabled ? 0 : -1}
        aria-label={this.name || undefined}
        aria-disabled={!this.enabled || undefined}
        aria-activedescendant={this.itemIndex >= 0 ? `${this.id}-${this.itemIndex}` : undefined}
        onKeyDown={e => this.keyDown(e)}
      >
        {this.items.length === 0 && this.emptyText && <li className="vcl-listbox-empty">{this.emptyText}</li>}
        {this.items.map((item, index) => (
          <li
            key={index}
            id={`${this.id}-${index}`}
            className="vcl-listbox-item"
            role="option"
            aria-selected={index === this.itemIndex}
            onClick={() => this.enabled && this.select(index)}
            onDoubleClick={() => this.enabled && this.invoke(this.onActivate, this)}
          >
            {this.format(item)}
          </li>
        ))}
      </ul>
    );
  }

  private keyDown(e: KeyboardEvent): void {
    if (!this.enabled) return;
    if (e.key === "ArrowDown") this.select(this.itemIndex + 1);
    else if (e.key === "ArrowUp") this.select(Math.max(0, this.itemIndex - 1));
    else if (e.key === "Enter" && this.selected !== undefined) this.invoke(this.onActivate, this);
    else return;
    e.preventDefault();
  }
}
