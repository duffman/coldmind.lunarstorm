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
const ENGINE: &str = "../engines/rquickjs/sys/quickjs";
/// The Rust compiler crates whose sources are part of the identity, as in jetbunny.
const COMPILER: [&str; 4] = ["qjs-ir", "qjs-frontend", "qjs-compiler-core", "qjs-bytecode"];
/// Patch identities: jetbunny's list, then Lunarstorm's (`PATCHES.md`).
const PATCHES: &str = "jetbunny-sticky-oom-v1+modules-v3+debugger-v2+eval-control-v1+binding-edit-v1+safe-lookup-v1+exception-info-v1+async-context-v1+rust-compiler-v1+test262-vm-v1+host-jobs-v1+lunarstorm-user-catch-table+import-attributes+int32-constants+writer-out-of-memory+compiler-temporaries";
/// The bridge ABI revision (`JBQ_ABI` in bridge.h).
const ABI: u32 = 14;
/// The native image format byte (`BC_VERSION`, `NATIVE_BC_VERSION` in qjs-bytecode).
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
/// every `.rs` of the compiler crates' `src`, each framed by its path relative to
/// this crate and its length, in path order.
fn inputs_sha256() -> String {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(ENGINE)
        .expect("vendored QuickJS inputs")
        .map(|entry| entry.expect("QuickJS directory entry").path())
        .filter(|path| matches!(path.extension().and_then(|x| x.to_str()), Some("c" | "h" | "inc")))
        .collect();
    for name in COMPILER {
        let dir = PathBuf::from("../compiler/crates").join(name).join("src");
        paths.extend(
            std::fs::read_dir(&dir)
                .expect("Rust compiler sources")
                .map(|entry| entry.expect("compiler source entry").path())
                .filter(|path| path.extension().and_then(|x| x.to_str()) == Some("rs")),
        );
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
