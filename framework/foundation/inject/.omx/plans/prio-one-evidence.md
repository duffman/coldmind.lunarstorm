# Packaging and verification evidence (2026-10-27)

Researcher read official/upstream documentation; these are implementation constraints, not dependency-comparison decisions.

- Export condition order: `types`, `bun`, `default`. Node tests object order; Bun recognizes source condition. https://nodejs.org/download/release/v22.12.0/docs/api/packages.html#conditional-exports and https://bun.sh/docs/runtime/module-resolution#importing-packages
- TS self-package exports targeting output are remapped to inputs via rootDir/outDir for clean builds. Build external dependency decorate before inject. https://www.typescriptlang.org/docs/handbook/modules/reference#packagejson-imports-and-self-name-imports
- rewriteRelativeImportExtensions affects relative extensions, not package specifiers. https://www.typescriptlang.org/tsconfig/rewriteRelativeImportExtensions.html
- V8 coverage does not work under Bun. Run functional suites on Bun and Node, but coverage-v8 under Node. Include src/**/*.ts, derive thresholds from actual measurements. https://vitest.dev/guide/coverage.html and https://vitest.dev/config/coverage
- Run type assertions through Vitest typecheck.only with explicit include/config, ignoreSourceErrors:false. https://vitest.dev/config/typecheck and https://vitest.dev/guide/testing-types.html
- Vitest5 runner requires Node>=22.12, separately from library Node>=22 contract. https://vitest.dev/guide/migration/
- Force Bun runtime; plain bun run can honor a Node shebang. https://bun.sh/docs/runtime#--bun
- TypeDoc requires no plugin for HTML; provide explicit public entrypoints and config/output. https://typedoc.org/documents/Options.Input.html and https://typedoc.org/documents/Options.Configuration.html and https://typedoc.org/documents/Options.Output.html

Functional Bun evidence must assert process.versions.bun inside the worker, not only on the outer command. Decorator-mode tests must cover both transformed and plain built/static Node paths.
