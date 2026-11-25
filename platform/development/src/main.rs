//! The `development` application of the Lunarstorm platform.

/// Entry point: prints the application name and version.
fn main() {
    println!("development {}", env!("CARGO_PKG_VERSION"));
}
