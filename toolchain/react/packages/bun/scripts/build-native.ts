/**
 * Builds the shared transform crate with Bun bindings and installs its host library.
 *
 * The Rust workspace owns compilation; only the platform artifact belongs to this adapter.
 * Cargo metadata locates configured target directories instead of assuming a local target/.
 * A failed compile or metadata query stops before replacing the previously working library.
 */
import { copyFileSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { nativeFileName } from "@coldmind/devtools";

/**
 * The toolchain directory that holds `crates/transform`.
 *
 * Fix: was three levels up (`toolchain/react`), where no `crates/transform` exists since the
 * toolchain move, so every build failed with "manifest path does not exist". Four levels up
 * is `toolchain/`.
 */
const productRoot = resolve(import.meta.dir, "../../../..");
/** The shared transform crate (`devtools-native`). */
const crate = join(productRoot, "crates/transform");
/** Its manifest, passed to cargo so the build runs inside the root workspace. */
const manifest = join(crate, "Cargo.toml");
/**
 * Cargo profile for the addon.
 *
 * Fix: was `--release`, which in the root workspace means `panic = "abort"`, so a panic in the
 * addon killed the host Bun process. Decision: `release-node` (root Cargo.toml) inherits
 * release (fat LTO, so the former `--config profile.release.lto=true` is dropped) but keeps
 * `panic = "unwind"`, which napi needs to turn a panic into a JS exception.
 */
const profile = "release-node";
const started = performance.now();
const cargo = Bun.spawnSync(["cargo", "build", "--profile", profile, "--manifest-path", manifest, "--features", "bun"], {
  cwd: productRoot,
  stdout: "inherit",
  stderr: "inherit",
});
if (cargo.exitCode !== 0) process.exit(cargo.exitCode ?? 1);

// Honors CARGO_TARGET_DIR and `build.target-dir` from any cargo config.
const metadata = Bun.spawnSync(["cargo", "metadata", "--format-version", "1", "--no-deps", "--manifest-path", manifest], { cwd: productRoot });
if (metadata.exitCode !== 0) {
  process.stderr.write(metadata.stderr);
  process.exit(metadata.exitCode ?? 1);
}
const { target_directory: targetDirectory } = JSON.parse(metadata.stdout.toString()) as { target_directory: string };

const library = process.platform === "win32" ? "devtools_native.dll"
  : process.platform === "darwin" ? "libdevtools_native.dylib"
  : "libdevtools_native.so";
const outputDirectory = resolve(import.meta.dir, "../native");
mkdirSync(outputDirectory, { recursive: true });
const target = join(outputDirectory, nativeFileName());
copyFileSync(join(targetDirectory, profile, library), target);
console.log(`devtools: ${target} (${Math.round(performance.now() - started)} ms)`);
