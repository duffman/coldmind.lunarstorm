# rquickjs-cli (rquickjs example)

A line-at-a-time JS REPL on rquickjs: prompts `> `, evaluates each stdin line with `ctx.eval`, prints the result through a JS `console.log` that joins its arguments, and prints errors with `.catch(&ctx)`. The global `exit()` ends the loop. Not the `lunarstorm` command line (that is `apps/lunarstorm` on the Runtime); this one talks to the engine binding directly.

- Default `rquickjs` features. Run: `cargo run -p rquickjs-cli`, or non-interactively `echo "exit()" | cargo run -p rquickjs-cli` (what `examples/Makefile` `cli` does). No tests.
- Hazard: stdin end-of-file is not handled (`read_line` returns 0 and the loop spins on empty input) unless the input contains `exit()`.
