# OJ source inventory

This directory stages selected, unmodified source snapshots from `toolchain/vendor/oj_*` for evaluation and later adaptation. The snapshots are **not compiled or wired into the bundler**. Some refer to other modules in their original crates; they are not standalone replacements. Existing code continues to use the vendor crates. Inline tests are retained with the copied source; external tests remain in the original vendor crates.

The source was adapted from [lovablelabs/oj](https://github.com/lovablelabs/oj), copyright Raphael Amorim and Lovable Labs Incorporated, under the MIT license. See [LICENSE](LICENSE). Preserve the notices when adapting code.

## Candidates

| Source | Use | Considerations |
| --- | --- | --- |
| `compiler/sourcemap.rs` | Compose plugin transform maps with compiler output | Relevant to a transform chain; requires Oxc sourcemap and base64. |
| `compiler/scan.rs`, `rewrite.rs` | Discover imports/exports, generate specifier edits | Useful for plugin resolution and module rewriting; depends on helpers in the original compiler. |
| `compiler/compile.rs`, `tsconfig.rs` | Assemble Oxc parsing and transforms, class-field configuration, maps | Main reference for removing the compiler wrapper; coupled to other compiler modules. |
| `resolver/` | Oxc resolver policy, aliases, conditions, deduplication and package exports | Compare to Rolldown's resolver before adopting duplicated behavior. |
| `compiler/glob.rs` | `import.meta.glob`, variable imports and URL handling | Useful Vite-compatible behavior; depends on filesystem and Oxc AST. |
| `compiler/interop.rs`, `cjs.rs` | CommonJS import interoperability and conversion | Relevant to development dependency compatibility; assess overlap with Rolldown for production. |
| `compiler/assets.rs`, `json.rs` | Asset classification and JSON modules | Compare existing bundler handling before adopting. |
| `env/` and `compiler/env.rs` | Dotenv, definitions, HTML substitutions, environment overlays | Config semantics and exposure rules must be defined explicitly. |
| `graph/`, `compiler/hot.rs` | Module graph, HMR boundaries and propagation | Useful for development serving; not required for a one-shot bundle. |

`oj_compiler/src/bundle.rs` contains helpers needed by some snapshots, but its separate factory-format bundler is not the production Rolldown pipeline. `oj_compiler/src/ssr.rs` is a candidate if SSR or a server-side module runner becomes a requirement. Both remain available in `toolchain/vendor/oj_compiler/src`.

## Vite plugin compatibility

These snapshots contain building blocks for plugin handling, not a Vite plugin container. The existing configuration adapter explicitly skips plugin hooks. Supporting real Vite plugins requires, among other things, a JavaScript plugin host and lifecycle, ordered `resolveId`, `load`, and `transform` dispatch, plugin context and virtual modules, source-map chaining, plus Vite-specific configuration, HTML, server and HMR hooks as applicable. Production and development integration must be tested independently. No plugin compatibility is enabled by staging this source.
