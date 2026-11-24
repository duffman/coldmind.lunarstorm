# lunarstorm-pack

The release step of spec 6.3: runs the pipeline over a project, writes the `.zebra` archive,
attaches it to the prebuilt `lunarstorm-release` executable and packages and signs the
result for the target OS. A developer tool; the shipped desktop process never links it.

## Public surface

One binary, `lunarstorm-pack`:

```
lunarstorm-pack <project> --runtime <lunarstorm-release> --out <dir>
                [--target macos|windows|linux] [--sign <identity>]
```

Prints the path of the produced artifact. `--target` defaults to the host OS
(`Target::host()`). Module `package` (private to the binary): `Target`, `exe_name`
(lowercase, hyphen-separated ASCII, `app` if nothing is left) and `package`.

| target  | output                                                        | signing                          |
|---------|---------------------------------------------------------------|----------------------------------|
| macOS   | `<Name>.app`, archive at `Contents/Resources/app.zebra`, Info.plist, icns from PNG | `codesign --force --deep --options runtime --sign <id or ->` |
| Windows | `<name>.exe` with the archive appended as an overlay          | `signtool sign /fd SHA256 /n <id>`, only if `--sign` |
| Linux   | `<name>/<name>`, `<name>.zebra`, `<name>.desktop`             | none                             |

## How it fits

Depends on `lunarstorm-platform` (`platform/desktop`, `default-features = false`) for
`OutputDirPipeline`, `write_archive`, `append_overlay` and `HostConfig`, so the archive
writer and the release runtime's reader are the same code. Spec: section 6.3 (Release).
The area overview is `platform/README.md`.

## Build, test, run

```bash
cargo build --profile platform --no-default-features --bin lunarstorm-release --bin lunarstorm-pack
cargo run --bin lunarstorm-pack -- platform/examples/app --runtime ~/.cargo-target/platform/lunarstorm-release --out _build/pack
cargo test -p lunarstorm-pack
```

Four unit tests in `src/package.rs` (names, Windows and Linux layouts carry a readable
archive, plist contents, icns wrapping). The `codesign` and `signtool` paths are not
exercised by tests.

## Decisions and hazards

- The pipeline here is `OutputDirPipeline` only: `<project>` must already be a generated
  output directory, not model sources. Compile errors print as `error: <file>: <message>`, exit 1.
- On macOS the signature is never skipped: without `--sign` the bundle is ad-hoc signed
  (`-`), because Apple silicon will not run an unsigned binary.
- On Windows the overlay is appended before `signtool` runs, because signtool puts the
  certificate table after it; signing first would break the trailer lookup.
- Build the runtime with the `platform` profile, not `release` (abort-on-panic).
- The `--runtime` binary is attached by copy; pack does not check it is a release runtime.
