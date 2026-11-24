# qjs-reference

This std-only crate runs a real QuickJS-NG engine in a subprocess. The engine is
pinned to `e22ce4de4362dc5fd4794cfa3c03fc7fdba5f015` and built from the vendored C
sources, independently of the Rust interpreter crates.

```sh
sh scripts/build-reference.sh
printf '%s' '6 * 7' | build/qjs-reference
cargo run -p qjs-reference --example evaluate -- path/to/source.js
printf '%s' '6 * 7' | cargo run -p qjs-reference --example evaluate -- -
```

```rust,no_run
use qjs_reference::{ReferenceRunner, ReferenceValue};

let reference = ReferenceRunner::new("build/qjs-reference");
assert_eq!(reference.evaluate("6 * 7")?, ReferenceValue::Number(42.0));
# Ok::<(), qjs_reference::ReferenceError>(())
```

`ReferenceRunner::from_env()` uses `QJS_REFERENCE` or the workspace's
`build/qjs-reference`. Each request starts a fresh runtime, evaluates one global
script, and returns its final completion value. `evaluate` passes UTF-8 source to
stdin. `evaluate_file` reads a bounded regular-file snapshot and streams it over stdin with its original canonical diagnostic filename; the C executable never reopens that path. Exceptions and input
errors produce a nonzero exit and `ReferenceError`; object, function, and symbol
completion values are explicitly unsupported. Objects and closures are usable
inside the script.

The executable prints exactly one ASCII line on success:

| JavaScript type | Protocol |
| --- | --- |
| Undefined | `undefined` |
| Null | `null` |
| Boolean | `boolean<TAB>true` or `boolean<TAB>false` |
| Number | `number<TAB>42`, `-0`, `NaN`, `Infinity`, or `-Infinity` |
| BigInt | `bigint<TAB>12345678901234567890` |
| String | `string<TAB>0041,d83d,de00,d800,0000` |

An empty string has an empty payload. String payloads are exact UTF-16 code units,
including embedded NUL and unmatched surrogates. Finite numbers use the engine's
round-trippable ECMAScript number formatting; negative zero is preserved
explicitly. No host globals, modules, timers, event loop, or debugger are added.
The C runner installs a monotonic one-second execution budget and interrupts
JavaScript loops when the budget expires. The Rust runner additionally enforces a five-second external process deadline and a 16 MiB input/combined-output cap by default. `ReferenceRunner::with_limits(Limits { timeout, max_input_bytes, max_output_bytes })` customizes these budgets. Timeouts and capacity failures have distinct `ReferenceError::Timeout` and `Capacity` variants.

On POSIX, the child starts in its own process group. Nonblocking I/O threads prevent blocked stdin or retained stdout/stderr from bypassing the deadline. A completed leader's remaining group is terminated before collecting pipe EOF; timeout/error paths terminate and reap the group and stop the pipe tasks. Descendants must remain in the inherited group: this is not a sandbox for malicious executables that deliberately escape process groups. Non-POSIX execution fails explicitly as unsupported.

File snapshot memory is bounded even if the source grows while read; regular-file filesystem I/O itself remains subject to operating-system I/O behavior. The standalone C runner also supports `--source-name NAME` for named stdin; its original `[source.js|-]` interface is preserved. Run trusted programs.

`cargo test -p qjs-reference` builds and exercises the actual C engine.
`python3 scripts/test-reference.py` checks its standalone stdin/file protocol.
`CC`, `CFLAGS`, `LDFLAGS`, and `QJS_REFERENCE_BUILD_DIR` configure the C build.
