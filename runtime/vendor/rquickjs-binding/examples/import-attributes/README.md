# import-attributes (rquickjs example)

Shows `import ... with { type: "json" }` through a custom `Resolver` and `Loader` (`ImportAttributes::get_type()`): module `config` loads only when `type` is `json` (otherwise `Error::new_loading_message(name, "requires type: json")`), module `greeting` is an ordinary ES module. A reference for the import-attributes API, not a runtime component.

- Features on `rquickjs`: `rust-alloc`, `loader`. Run: `cargo run -p import-attributes`. No tests.
- Defect in the example: the JSON served for `config` is `{"hello":"world"}`, but the script prints `config.name` and `config.version`, so both print `undefined`. Nothing asserts the output, so it passes silently.
