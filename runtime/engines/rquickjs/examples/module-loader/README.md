# module-loader (rquickjs example)

Runs four imports through rquickjs's composable `set_loader(resolver, loader)` tuples: a script module from disk (`script_module.js`), a native module from a shared library, and the same two compiled in (`bundle/script_module`, `bundle/native_module`). Each import prints `n`, `s` and `f(2, 4)`; any failure panics (`unwrap`). A reference for the loader API, not a runtime component.

- Resolvers: `BuiltinResolver` (bundled names), `FileResolver` with paths `./` and `../../target/debug` plus `.with_native()`. Loaders: `BuiltinLoader`, `ModuleLoader`, `ScriptLoader`, `NativeLoader`. Features on `rquickjs`: `futures`, `rust-alloc`, `loader`, `dyn-load`, `std`.
- `src/bundle/` holds the compiled-in native module (macro or hand-written, selected by the default `macro` feature, same as `../native-module`).
- Run: `cargo build -p native-module && cd runtime/engines/rquickjs/examples/module-loader && cargo run` (`--release`, `--no-default-features` variants exist; `examples/Makefile` targets `native.*` and `run` chain both builds). No tests.
- Hazard: the "native module" import finds `libnative_module` through the relative path `../../target/debug`, which is not where this repo builds (`target-dir = ~/.cargo-target` in `~/.cargo/config.toml`), so that step fails unless the library is copied or the path is changed. It also must run from this crate's directory so `./script_module.js` resolves.
