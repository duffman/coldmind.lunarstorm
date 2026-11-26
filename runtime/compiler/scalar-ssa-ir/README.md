# qjs-scalar-ssa-ir

A standard-library-only, function-level SSA IR for an explicitly typed scalar
subset. Construct `Function` values directly; this crate is not a JavaScript
frontend or QuickJS bytecode backend.

```sh
cargo test -p qjs-scalar-ssa-ir
cargo run -p qjs-scalar-ssa-ir --example fold
```

The example validates a branching function, folds `6 * 7`, passes the selected
value to a block parameter and interprets `Number(42.0)` before and after folding.

`Function` owns blocks identified by `BlockId`. `Parameter` and `Instruction`
definitions have globally unique `ValueId`s. Blocks end in `Jump`, boolean
`Branch` or `Return`; edge arguments bind destination parameters simultaneously.
Entry block parameters are function arguments. IDs can be sparse, and block
storage order does not determine execution order.

`validate(&Function)` checks identifiers, targets, reachability, byte ranges,
types, edge arity, definition order and dominance. Blocks, parameters,
instructions and terminators carry `Span` ranges into a single source whose
byte length is `Function::source_len`. Empty ranges are valid. Source text is
not stored, so validation checks bounds, not UTF-8 character boundaries.

`execute(&Function, &[Scalar], step_limit)` validates first and interprets
binary64 numbers and booleans. Each instruction and terminator consumes one
step; exhausting the budget returns `ExecutionError::StepLimit`. All declared
operations execute: numeric negation, boolean negation, numeric arithmetic,
same-type scalar equality and numeric comparisons. Operations do not coerce.

`fold_constants(&mut Function)` validates before mutation and returns the number
of replaced instructions. It folds numeric constants transitively across
dominating definitions, including numeric comparisons. Result IDs and source
spans remain attached to the replaced instructions. It does not propagate
through block parameters, fold boolean operators, alter control flow, eliminate
dead instructions, reassociate arithmetic or apply identities such as `x * 0`.
It preserves signed zero and NaN behavior using the interpreter's arithmetic;
NaN payload bits remain subject to the host's IEEE-754 operations.

This IR excludes strings, null/undefined, BigInt, objects, coercion, calls,
exceptions, memory, closures and side effects. All blocks must be reachable.
