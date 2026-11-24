# lunarstorm-model

Model extraction, spec 4a. Reads `src/app.tsx` and every `*.view.tsx` of a Lunarstorm
project from the oxc AST into a [`Project`], pairs each view with its form class
(`x.view.tsx` with `x.ts`), and reports every compile error of the model. Nothing is
executed and no import is followed, so extraction is cheap enough to run on every
keystroke of the dev loop. It is its own crate because the same library is linked into the
CLI and the debug runtime (spec 4: "one library, linked into both").

## Public surface

- `extract(root: &Path) -> Extraction { project: Option<Project>, diagnostics }`. A model
  with any error yields `project: None`; generators only ever see a valid project.
- Types: `Project { application, forms, natives }`, `Form { name, view, class, root }`,
  `Element`, `Prop`, `Value` (`Text`, `Number`, `Bool`, `List`, `Handler`), `Diagnostic`
  with a `Code` (`Duplicate`, `MainForm`, ...), `Position` (1-based).
- Constants `APPLICATION = "src/app.tsx"`, `VIEW_SUFFIX = ".view.tsx"`.
- `schema` (public): the element table, `KINDS`, `COMMON` props, `ROLES`, `lookup(tag)`,
  `class_name(tag)`, `accepts(parent, child)`. The pipeline's host-config, frontend and
  type generators all read it, so compiler and generators cannot drift apart.

## How it fits

Only `lunarstorm-pipeline` depends on it (`toolchain/crates/pipeline`); the oxc crates are
its only dependencies. Spec: section 2 (the model: 2.2 parse rules, 2.3 form properties,
2.4 controls, 2.5 form classes, 2.6 application, 2.7 layout) and 4a.

## Build and test

```bash
cargo test -p lunarstorm-model
```

Integration tests in `tests/`: `rules.rs` (parse rules, nesting, handler checks, one per
rule), `controls.rs` (`the_element_table_is_the_spec_table` pins the table against the
spec), `starter.rs` (extracts `templates/starters/desktop` and expects no errors).

## Decisions and hazards

- Resources are configuration: the top level may hold only imports, `declare`s, type
  declarations and the default export; anything that would run is a compile error.
- Every error in a file is reported, not only the first; an element with a bad prop is
  still walked so its children are checked. Diagnostics are sorted by file, line, column.
- Diagnostics carry a `Code`; tests and the dev overlay match on it, never on message text.
- Paths in the model and in diagnostics are relative to the project root.
- A form class is found by exported class name in the paired `.ts`. Only members declared
  on the class itself count (no inherited members: that would mean following imports).
- `this.find<T>('Name')` is resolved here against the view; a non-literal argument is an
  error because it cannot be checked.
- Application events resolve as `form.<member>` against the main form's class.
- Native permissions are derived by scanning every `.ts` under `src/` (not only form
  classes) for `native.<ns>.<method>` on a binding imported from `@desktop/api`; tests
  and view resources are skipped, and a file that does not parse contributes nothing
  (`tsc` reports it).
- `document` and `permissions` prop names are not enumerated by the spec; the schema's
  names follow its prose.
- `popupmenu` is also valid under `<application>` because an application-wide `trayicon`
  names its menu by `popupMenu`.
