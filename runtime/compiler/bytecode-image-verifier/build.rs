//! Generates the opcode schema and production tables from the vendored QuickJS-NG
//! headers (`opcodes.rs`, `production.rs` in `OUT_DIR`).

use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest path"));
    let header_dir = manifest.join("../vendor/quickjs-ng");
    let source = manifest.join("tools/opcode_schema.c");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let generator = out.join("opcode-schema-generator");
    println!(
        "cargo:rerun-if-changed={}",
        header_dir.join("quickjs-opcode.h").display()
    );
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-env-changed=HOST_CC");

    let compiler = env::var_os("HOST_CC").unwrap_or_else(|| "cc".into());
    let compilation = Command::new(compiler)
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(&header_dir)
        .arg(&source)
        .arg("-o")
        .arg(&generator)
        .output()
        .expect("run host C compiler (set HOST_CC to its executable path)");
    assert!(
        compilation.status.success(),
        "opcode schema compilation failed:\n{}",
        String::from_utf8_lossy(&compilation.stderr)
    );
    let generated = Command::new(&generator)
        .output()
        .expect("run host opcode schema generator");
    assert!(
        generated.status.success(),
        "opcode schema generation failed:\n{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    fs::write(out.join("opcodes.rs"), generated.stdout).expect("write generated Rust metadata");
    let prod = manifest.join("../vendor/quickjs-ng");
    let generator = out.join("production-schema");
    assert!(
        Command::new(env::var_os("HOST_CC").unwrap_or_else(|| "cc".into()))
            .args(["-std=c11", "-DSHARED_FORMATS", "-I"])
            .arg(&prod)
            .arg(&source)
            .arg("-o")
            .arg(&generator)
            .status()
            .unwrap()
            .success()
    );
    let data = Command::new(generator).output().unwrap();
    assert!(data.status.success());
    fs::write(out.join("production.rs"), data.stdout).unwrap();
    println!("cargo:rerun-if-changed={}", prod.display());
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let mut files: Vec<_> = fs::read_dir(&prod)
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e == "c" || e == "h" || e == "inc")
        })
        .collect();
    files.sort();
    for f in files {
        let name = f.file_name().unwrap().as_encoded_bytes();
        let bytes = fs::read(&f).unwrap();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name);
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    for key in ["CFLAGS", "HOST_CFLAGS", "TARGET_CFLAGS", "CC"] {
        println!("cargo:rerun-if-env-changed={key}");
        hash.update(key);
        hash.update(env::var(key).unwrap_or_default());
    }
    let target = env::var("TARGET").unwrap();
    for key in [
        format!("CFLAGS_{target}"),
        format!("CFLAGS_{}", target.replace('-', "_")),
    ] {
        println!("cargo:rerun-if-env-changed={key}");
        hash.update(&key);
        hash.update(env::var(key).unwrap_or_default());
    }
    hash.update(env::var("TARGET").unwrap());
    hash.update(env::var("CARGO_CFG_TARGET_ENDIAN").unwrap());
    // lunarstorm patch: bc94 (was bc93), the native format version.
    hash.update("bc94;no-parser;short-opcodes;rust-native-locations-v3");
    println!(
        "cargo:rustc-env=QJS_PRODUCTION_FINGERPRINT=7b5f588e0f9b643e2996f291bfc861abeaceddc2:{:x}",
        hash.finalize()
    );
}
