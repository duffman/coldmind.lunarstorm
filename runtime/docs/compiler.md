# The compiler (`runtime/compiler`)

**This is not an ahead-of-time tool.** It is the JavaScript compiler of the engine, called at
run time for every script, module, `eval`, `new Function` and debugger `eval`. The patched
quickjs-ng has no C parser (patch `jetbunny-rust-compiler-v1`; the removed code sits in
`.redundant-legacy/`). If this compiler cannot compile something, nothing runs it.

```
source text (UTF-16)                                     TypeScript never gets here: runtime/modules strips it first
  │ qjs-frontend     SourceInput → OXC 0.157 parse + semantic → budget check → Lower
  ▼
qjs_ir::js::Program  owned, AST-shaped IR (Statement/Expression/Pattern/Function/Scope/Module); not SSA
  │ qjs-compiler-core js_emit::emit_program_with_options (+ class_emit, call_emit, class_references)
  ▼
FunctionImage / ModuleImage
  │ qjs-bytecode     image::verify_and_encode*: verify graph → encode → decode again → verify again
  ▼
VerifiedImage        native quickjs-ng bytecode (JS_WriteObject format, BC_VERSION 94 = 30|0x40)
  │ rquickjs_core::compiler → jbq_install_verified → JS_JetbunnyInstall → JS_ReadObject
  ▼
engine function/module (the C side does NOT re-check: it trusts the Rust verifier)
```

The glue between engine and compiler is `runtime/engines/rquickjs/core/src/compiler.rs`, which
rquickjs installs on every context it creates (`install_base`). Errors: a message starting with
`capacity` becomes a `RangeError`, anything else a `SyntaxError`. A direct `eval` with a live
frame goes through `qjs_compiler_core::eval::compile_eval`. That path is checked against the
caller frame's layout, read through the bridge (`EnvLayout` → `VerifiedEnvLayout` →
`VerifiedEvalImage`, a "frame lease").

## Crates

| crate | role |
|---|---|
| `qjs-frontend` | `SourceInput` (UTF-16 with a reversible UTF-8 view; lone surrogates escaped for OXC), parse, early errors, lowering to IR |
| `qjs-ir` | the IR. `js` is the production IR; the crate root and README describe an older scalar SSA IR used only by tests |
| `qjs-compiler-core` | the emitter: stack depth, labels, unwind/finally, `with`, eval var objects, `using` disposal; emits opcodes by name against tables generated from the engine headers |
| `qjs-bytecode` | image model, encoder, the verifier (`image_verify.rs`: a dataflow interpreter over the function graph), eval layouts. `build.rs` generates opcode/atom/format tables from the vendored headers with the host C compiler and fingerprints the engine sources |
| `qjs-reference` | a differential oracle: a C runner built from **unmodified** upstream quickjs-ng (`vendor/quickjs-ng-upstream`), driven as a subprocess, compares primitive completion values |

`vendor/quickjs-ng` is a **symlink** to `runtime/engines/rquickjs/sys/quickjs`, so the compiler
and the engine read the same headers.

## Supported language

Nearly all ES statements and expressions (from the lowering arms in `qjs-frontend/src/js.rs`; how
complete the semantics are is what the test suites measure): classes with private names, generators and
async, `super`, `new.target`, `import.meta`, dynamic `import()`, tagged templates, BigInt,
optional chaining and `using`.

Refused as unsupported: class decorators, import phase modifiers, and any TS or JSX node.

Regexp literals compile through the multiplexed `strict_op` opcode (selector 4). libregexp then
compiles them at run time, so `qjs-bytecode/src/regexp.rs` has no caller.

## Budgets (admission limits a caller may only tighten)

| layer | limit |
|---|---|
| frontend `CompileLimits` | 1 M AST nodes, nesting 256 |
| compiler-core `EmissionLimits` | 32 M IR nodes, 65 534 locals, 2e9 work units |
| bytecode `ImageLimits` | 16 MiB native image, 64 M verification work units |
| compile worker job | heap 512 MiB, deadline |

## Code that looks live and is not

None of these has a caller outside tests:
- the scalar SSA pipeline (`qjs_frontend::compile` → `qjs_ir::Function` → `qjs_compiler_core::compile`);
- `bytecode::snapshot` and `bytecode::verify`;
- `regexp.rs`;
- the `qjs-compile-worker` binary and its job protocol, meant to isolate a crashing compiler in
  a child process.

## Hazards

- **A verifier gap is a memory-safety bug.** The C loader runs whatever the Rust verifier
  accepted.
- **Hand-mirrored engine facts:**
  - `NATIVE_BC_VERSION = 94` (`image_format.rs:35`);
  - the fingerprint salt in `qjs-bytecode/build.rs:92`;
  - `NATIVE_PROFILE`;
  - the hand-written stack-effect table and the `strict_op` selectors 0–6
    (`image_verify.rs:6-164`), which must match `quickjs.c` and `quickjs-opcode.h:383-391`;
  - the checksum quirk (`image.rs:248-266`);
  - the bridge struct layouts in `compiler.rs`.

  Opcode ids, widths and atoms are generated, not mirrored.
- **`strict_op` is the escape hatch.** New engine behaviour tends to land there as another
  selector. Any new selector must also be added to the verifier's effects.
- **OXC is pinned at `=0.157`**, and the frontend matches its exact diagnostic strings
  (`js.rs:121-128`). An upgrade can change behaviour silently.
- **Two parse hacks**: a parse-only `.a` suffix for Annex-B call targets (`f()=1`), and a
  re-parse with an injected `'use strict'` to validate inherited strictness.
- **Mixed editions**: `qjs-ir`/`qjs-bytecode` are 2021/1.85, `qjs-frontend` 2024/1.97 and
  `qjs-compiler-core` 2024/1.99.
- **A build needs a host C compiler** (`HOST_CC` or `cc`) for `qjs-bytecode/build.rs`.
- **Stale references:**
  - `qjs-reference` and `qjs-bytecode::UPSTREAM_REVISION` name `e22ce4de`, while the vendor
    tree and the script use `7b5f588e`;
  - the README cites `scripts/test-reference.py`, which does not exist;
  - `class_emit.rs:4` says BC93;
  - `PATCHES.md` cites `tests/compiler-contract/target.json`, which is not in the repo;
  - the root `Cargo.toml` says "copied byte for byte", but 28 `lunarstorm patch` edits exist.

## Tests

```sh
cargo test -q -p qjs-ir -p qjs-frontend -p qjs-bytecode -p qjs-compiler-core -p qjs-reference
sh runtime/compiler/scripts/build-reference.sh     # builds runtime/compiler/build/qjs-reference for oracle tests (or set QJS_REFERENCE)
```

Native probe tests compile the patched engine with `cc` and write under `std::env::temp_dir()`.
Set `TMPDIR` to a repo `.scratch/` directory to keep them out of `/tmp`. End-to-end engine
behaviour is tested in the rquickjs, quickjs adapter and debug crates.
