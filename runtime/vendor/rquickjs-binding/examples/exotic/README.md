# exotic (rquickjs example)

An exotic object: `#[rquickjs::class(exotic)]` with `#[rquickjs::exotic]` hooks (`#[qjs(get)]`, `set`, `has`, `delete`) that intercept property access by `Atom`, so `my_class.value`, assignment, `delete` and `"value" in my_class` run Rust code. Only the property named `value` is handled; any other name falls through. `main.rs` prints the results of six evals.

- Features on `rquickjs`: `macro`. Run: `cargo run -p exotic`. No assertions, output is for reading; no tests.
- Note: `delete my_class.value` sets the Rust field to 0 and reports success, so the next read returns `0` rather than `undefined`; `set` calls `value.as_int().unwrap()` and panics if JS assigns a non-integer.
