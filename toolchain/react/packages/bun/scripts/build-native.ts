/**
 * Builds the shared transform crate with Bun bindings and installs its host library.
 *
 * The Rust workspace owns compilation; only the platform artifact belongs to this adapter.
 * Cargo metadata locates configured target directories instead of assuming a local target/.
 * A failed compile or metadata query stops before replacing the previously working library.
 */
import { copyFileSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { nativeFileName } from "@coldmind/devermind";

const productRoot = resolve(import.meta.dir, "../../..");
const crate = join(productRoot, "crates/transform");
const manifest = join(crate, "Cargo.toml");
const started = performance.now();
const cargo = Bun.spawnSync(["cargo", "build", "--release", "--manifest-path", manifest, "--features", "bun", "--config", "profile.release.lto=true"], {
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

const library = process.platform === "win32" ? "devermind_native.dll"
  : process.platform === "darwin" ? "libdevermind_native.dylib"
  : "libdevermind_native.so";
const outputDirectory = resolve(import.meta.dir, "../native");
mkdirSync(outputDirectory, { recursive: true });
const target = join(outputDirectory, nativeFileName());
copyFileSync(join(targetDirectory, "release", library), target);
console.log(`devermind: ${target} (${Math.round(performance.now() - started)} ms)`);
