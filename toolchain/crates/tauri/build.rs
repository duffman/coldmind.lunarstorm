fn main() {
    if std::env::var_os("CARGO_FEATURE_PLUGIN").is_some() {
        tauri_plugin::Builder::new(&["transform"]).build();
    }
}
