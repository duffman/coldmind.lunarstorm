# Generic build server

This Rust crate coordinates the existing toolchain development server and production bundler for a project. It reuses compilation, module resolution, development watching/HMR, JS/TS/JSX bundling, stylesheets, imported assets and public-file handling.

From `toolchain/`:

```sh
cargo run -p ls-build-server -- serve /path/to/project --port 5173
cargo run -p ls-build-server -- bundle /path/to/project --out dist
```

A project supplies `index.html` with a module entry and its own dependencies. The same `.devtools/config.toml` and optional Vite configuration support as the existing toolchain applies. `serve` runs until Ctrl-C; `--no-watch` disables watching. `bundle` accepts `--no-minify`. Run `--help` for usage.

The library exposes `BuildServer::new`, `start`, `stop`, `development`, and asynchronous `bundle`. The development handle exposes the existing module graph, invalidation and application channels. Dropping the build server stops the development listener and watcher.

Bundle calls on one `BuildServer` instance are serialized. Separate instances or processes are not coordinated; callers must avoid building to the same output concurrently. The underlying bundler replaces the output directory before compilation, so an unsuccessful build may leave partial output. This initial crate operates on generic web projects and provides the machinery for subsequent project analysis and generation.

Verification:

```sh
cargo test -p ls-build-server
```
