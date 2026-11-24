// One class shape declared with defineDecorator, run by the standard test in process and by the legacy fixture under experimentalDecorators.
import { defineDecorator, type FieldWrap } from "@coldmind/rpc";

export const log: string[] = [];

/** A class decorator: records the class, which also flushes the member callbacks queued for it. */
export const tagged = defineDecorator("tagged", (site, label: string) => {
  if (site.kind !== "class") throw new TypeError("@tagged can only decorate a class");
  log.push(`class ${site.name} ${label}`);
});

/** A method decorator that reports its class once the class decorator has run. */
export const traced = defineDecorator("traced", site => {
  if (site.kind !== "method") throw new TypeError("@traced can only decorate a method");
  site.onClass(ctor => log.push(`method ${ctor.name}.${String(site.name)}`));
});

/** A field decorator: counts writes (a reactive property), records the member and uppercases the initial value. */
export const watched = defineDecorator("watched", (site): FieldWrap => {
  if (site.kind !== "field") throw new TypeError("@watched can only decorate a field");
  site.onClass(ctor => log.push(`field ${ctor.name}.${String(site.name)}`));
  return {
    init(initial) {
      return typeof initial === "string" ? initial.toUpperCase() : initial;
    },
    set(value, write, read) {
      if (Object.is(read(), value)) return;
      log.push(`set ${String(site.name)} ${String(value)}`);
      write(value);
    },
  };
});
