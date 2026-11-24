//! Sets the identity of jetbunny's QuickJS-NG adapter (`engines::quickjs_ng`):
//! `JETBUNNY_QJS_TARGET` and `JETBUNNY_QJS_BUILD_ID`, which the adapter stamps on
//! module images and checks when loading them.
//!
//! jetbunny took both from its rquickjs-sys `links` metadata plus a hash of its
//! Rust compiler sources. Decision: here the engine (C parser included) and the
//! bridge are the whole native input, so the build ID hashes exactly those files
//! in `runtime/engines/rquickjs/sys/quickjs`, with the same length framing.

use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Directory of the vendored engine and bridge, relative to this crate.
const ENGINE: &str = "../engines/rquickjs/sys/quickjs";

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let identity = format!(
        "quickjs-ng:7b5f588e0f9b643e2996f291bfc861abeaceddc2;lunarstorm-c-parser;abi=14;target={target};inputs_sha256={}",
        inputs_sha256()
    );
    println!("cargo:rustc-env=JETBUNNY_QJS_TARGET={target}");
    println!("cargo:rustc-env=JETBUNNY_QJS_BUILD_ID={identity}");
}

/// # Returns
/// SHA-256 (hex) over every `.c`, `.h` and `.inc` of the engine directory, each
/// framed by its name and length, in name order.
fn inputs_sha256() -> String {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(ENGINE)
        .expect("vendored QuickJS inputs")
        .map(|entry| entry.expect("QuickJS directory entry").path())
        .filter(|path| matches!(path.extension().and_then(|x| x.to_str()), Some("c" | "h" | "inc")))
        .collect();
    paths.sort();
    let mut digest = Sha256::new();
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.file_name().and_then(|n| n.to_str()).expect("ASCII input name").as_bytes();
        let bytes = std::fs::read(&path).expect("engine input bytes");
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name);
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    digest.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
