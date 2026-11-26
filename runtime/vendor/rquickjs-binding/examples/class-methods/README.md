# class-methods (rquickjs example)

Defines a Rust struct as a JS class with `#[rquickjs::class]` and `#[rquickjs::methods(rename_all = "camelCase")]` (`src/class.rs`): a constructor, a `data` getter, and methods bound to well-known names with `#[qjs(rename = PredefinedAtom::ToJSON | ToString | SymbolToPrimitive)]`. `main.rs` defines `MyClass` on the global and asserts `toString()`, `JSON.stringify` and string coercion.

- Features on `rquickjs`: none beyond `macro` (default feature). Run: `cargo run -p class-methods`; it panics on a failed assertion, which is the only check. No tests.
- `#[qjs(skip_trace)]` on the `String` field: only fields holding JS values need tracing. `to_primitive` returns `null` for any hint other than `"string"`.
