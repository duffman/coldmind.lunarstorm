//! Sets the identity of jetbunny's QuickJS-NG adapter (`engines::quickjs_ng`):
//! `JETBUNNY_QJS_TARGET` and `JETBUNNY_QJS_BUILD_ID`, which the adapter stamps on
//! module images and checks when loading them.
//!
//! lunarstorm patch: jetbunny composes the identity from its rquickjs-sys `links`
//! metadata (`quickjs-ng:<revision>;patch=<identities>;abi=<n>;...`) plus
//! `;bytecode=<BC_VERSION byte>;inputs_sha256=<hash>` over the engine, the bridge
//! and the Rust compiler sources. Lunarstorm's rquickjs-sys is rquickjs's own build
//! script, so this composes the same fields here. Fix: it said
//! `lunarstorm-c-parser;abi=14` and hashed the engine only, both false since the C
//! parser was removed. The ABI stays 14: see `JBQ_ABI` in bridge.h.

use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Directory of the vendored engine and bridge, relative to this crate.
const ENGINE: &str = "../vendor/rquickjs-binding/sys/quickjs";
/// The Rust compiler crates whose sources are part of the identity, as in jetbunny.
const COMPILER: [&str; 4] = ["scalar-ssa-ir", "js-parser-frontend", "bytecode-emitter", "bytecode-image-verifier"];
/// Patch identities: jetbunny's list, then Lunarstorm's (`PATCHES.md`).
const PATCHES: &str = "jetbunny-sticky-oom-v1+modules-v3+debugger-v2+eval-control-v1+binding-edit-v1+safe-lookup-v1+exception-info-v1+async-context-v1+rust-compiler-v1+test262-vm-v1+host-jobs-v1+lunarstorm-user-catch-table+import-attributes+int32-constants+writer-out-of-memory+compiler-temporaries";
/// The bridge ABI revision (`JBQ_ABI` in bridge.h).
const ABI: u32 = 14;
/// The native image format byte (`BC_VERSION`, `NATIVE_BC_VERSION` in qjs-bytecode-image-verifier).
const BYTECODE: u8 = 94;

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let identity = format!(
        "quickjs-ng:7b5f588e0f9b643e2996f291bfc861abeaceddc2;patch={PATCHES};abi={ABI};target={target};bytecode={BYTECODE};inputs_sha256={}",
        inputs_sha256()
    );
    println!("cargo:rustc-env=JETBUNNY_QJS_TARGET={target}");
    println!("cargo:rustc-env=JETBUNNY_QJS_BUILD_ID={identity}");
}

/// # Returns
/// SHA-256 (hex) over every `.c`, `.h` and `.inc` of the engine directory and
/// every `.rs` of the compiler crates' `src` (recursively), each framed by its path relative to
/// this crate and its length, in path order.
fn inputs_sha256() -> String {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(ENGINE)
        .expect("vendored QuickJS inputs")
        .map(|entry| entry.expect("QuickJS directory entry").path())
        .filter(|path| matches!(path.extension().and_then(|x| x.to_str()), Some("c" | "h" | "inc")))
        .collect();
    for name in COMPILER {
        rust_sources(&PathBuf::from("../compiler").join(name).join("src"), &mut paths);
    }
    paths.sort();
    let mut digest = Sha256::new();
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.to_str().expect("ASCII input path").as_bytes();
        let bytes = std::fs::read(&path).expect("identity input bytes");
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name);
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    digest.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Collects every `.rs` file below `dir`, subdirectories included.
///
/// Fix: the walk read only the top of each `src`, so files in subfolders (the
/// emitter's `script_emitter/`) were left out of the identity and an emitter
/// change there kept the old build id. Decision: recurse, rather than list the
/// subfolders, so a new one is covered without touching this script.
///
/// # Arguments
/// * `dir` - the folder to walk.
/// * `paths` - receives the files found.
fn rust_sources(dir: &std::path::Path, paths: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("Rust compiler sources") {
        let path = entry.expect("compiler source entry").path();
        if path.is_dir() {
            rust_sources(&path, paths);
        } else if path.extension().and_then(|x| x.to_str()) == Some("rs") {
            paths.push(path);
        }
    }
}
