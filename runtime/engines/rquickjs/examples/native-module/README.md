# native-module (rquickjs example)

A QuickJS native module built as a `cdylib` (`libnative_module`) that a host can load at run time with rquickjs's `NativeLoader`. It exports `n = 123`, `s = "abc"` and `f(a, b) = (a + b) * 0.5`, and ends in `rquickjs::module_init!(NativeModule)`. Not a runtime component; a reference for the module API.

- Two builds of the same module: `src/using_macro.rs` (`#[rquickjs::module]`, default feature `macro`) and `src/hand_written.rs` (`ModuleDef::declare` / `evaluate` with `Declarations` and `Exports`, selected by `--no-default-features`).
- Features on `rquickjs`: `futures`, `rust-alloc`, `std`.
- Consumed by `../module-loader`, which imports it both from the shared library and compiled in (`bundle/native_module`).
- Build: `cargo build -p native-module` (add `--no-default-features` for the hand-written variant, `--release` for release). Nothing runs on its own; no tests.
- Hazard: the artifact lands in the shared cargo target dir (`~/.cargo-target/<profile>`), but `module-loader` looks in `../../target/debug` (see its README).
