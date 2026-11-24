fn main() {
    #[cfg(feature = "bun")]
    bun_build();
}

#[cfg(feature = "bun")]
fn bun_build() {
    napi_build::setup();
    // napi symbols come from the host (Bun) at load time; lib unit tests never call them.
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => println!("cargo:rustc-link-arg=-Wl,-undefined,dynamic_lookup"),
        Ok("linux") => println!("cargo:rustc-link-arg=-Wl,--unresolved-symbols=ignore-all"),
        _ => {}
    }
}
