# oj_graph

The dev server's module graph and HMR boundary propagation, without any I/O: who imports whom,
which modules accept updates (self, declared dependencies, named exports), and the HMR
timestamps that make importers re-fetch changed modules (`?t=<timestamp>`).

Vendored from [lovablelabs/oj](https://github.com/lovablelabs/oj) (MIT, Copyright (c) 2026 Raphael Amorim and Lovable Labs Incorporated; license text in `../LICENSE`, SPDX headers kept in every source file). Version `0.2.29` is the workspace version. No diff against upstream was run for this README; nothing in the code marks a local change except that the manifest inherits `edition`/`license` from the root `[workspace.package]` and dependencies from `[workspace.dependencies]`.

## Surface
- `ModuleGraph` (`ensure_module`, `add_import`, `set_imports`, `set_self_accepting`, `set_accepted_deps`,
  `set_accepted_exports`, `set_imported_bindings`, `accept_invalidation`, `stamp_update`, `stamp_pruned`,
  `hmr_timestamp`, `imports_timestamp`), `ModuleNode`.
- `propagate_update(&path)` and `propagate_from_seeds` return `HmrDecision::{Update { boundaries },
  FullReload { reason }}`; `UpdateTarget` names a boundary, the module it accepts and whether it sits in
  an import cycle (a failed re-import there must reset the page).

## Use in this repo
Used by `devtools_server` (`State::graph`, `hot_reload::on_change`). No dependencies.
Propagation follows Vite's `propagateUpdate`; an unknown changed file or a missing boundary is a full reload.

## Test
`cargo test -p oj_graph`: `model.rs` checks the graph against a reference model after every step
(proptest; regressions are kept in `tests/model.proptest-regressions`), `adverse.rs` (200,000-deep chains
run on a big worker stack; cycles and diamonds terminate), `warm_path.rs`.
