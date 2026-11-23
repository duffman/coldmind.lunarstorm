//! Embeds the AppRuntime named by `ZEBRA_APP_RUNTIME` (see `src/runtime/mod.rs`).
//!
//! It writes `$OUT_DIR/selected_runtime.rs`, which `runtime::selected` includes:
//! `NAME` and `create(ctx) -> Box<dyn AppRuntime>` for the chosen runtime.
//!
//! Values: `quickjs` (default), `mock`, or a path (relative to this crate) to a `.rs` file
//! that defines `pub fn create(ctx: crate::runtime::RuntimeContext) -> Box<dyn crate::runtime::AppRuntime>`.

use std::env;
use std::fs;
use std::path::PathBuf;

/// Generates the selection module.
fn main() {
    println!("cargo:rerun-if-env-changed=ZEBRA_APP_RUNTIME");
    let choice = env::var("ZEBRA_APP_RUNTIME").unwrap_or_else(|_| "quickjs".into());
    let code = match choice.as_str() {
        "quickjs" => "pub const NAME: &str = \"quickjs\";\n\
             pub fn create(ctx: RuntimeContext) -> Box<dyn AppRuntime> { Box::new(JsThread::spawn(ctx.out, ctx.natives)) }\n"
            .to_owned(),
        "mock" => "pub const NAME: &str = \"mock\";\n\
             pub fn create(ctx: RuntimeContext) -> Box<dyn AppRuntime> { Box::new(MockRuntime::new(ctx)) }\n"
            .to_owned(),
        path => {
            let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
            let file = manifest.join(path);
            let file = file.canonicalize().unwrap_or_else(|e| {
                panic!("ZEBRA_APP_RUNTIME={path:?} is not `quickjs`, `mock` or a readable .rs file: {}: {e}", file.display())
            });
            println!("cargo:rerun-if-changed={}", file.display());
            format!("pub const NAME: &str = {path:?};\n#[path = {:?}]\npub mod external;\npub use external::create;\n", file.display().to_string())
        }
    };
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("selected_runtime.rs");
    fs::write(&out, code).unwrap_or_else(|e| panic!("cannot write {}: {e}", out.display()));
}
