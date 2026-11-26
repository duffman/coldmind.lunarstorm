//! Lunarstorm's side of developer keys: makes the root key and issues developer
//! keys with it. Not part of `lunar`; developers never hold the root.
//!
//! ```text
//! cargo run -p lunarstorm-project --example issue -- root <root.der>
//!     writes a new root key (owner-only) and prints the public key for `ROOT`
//! cargo run -p lunarstorm-project --example issue -- developer <root.der> <id> <days>
//!     prints a developer key file valid for <days> days
//! ```

use std::io::Write;

/// Runs the `root` or `developer` action.
///
/// # Returns
/// Exit status 0, or 2 for a usage error, 1 for a failure.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match words.as_slice() {
        ["root", file] => root(file),
        ["developer", root, id, days] => developer(root, id, days),
        _ => {
            eprintln!("usage: issue root <root.der> | issue developer <root.der> <id> <days>");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

/// Writes a new root key, refusing to overwrite one.
///
/// # Arguments
/// * `file` - where to write the PKCS#8 DER key.
///
/// # Errors
/// The file exists or cannot be written.
fn root(file: &str) -> Result<(), String> {
    let signer = lunarstorm_jbn_package_format::Signer::generate().map_err(|e| e.to_string())?;
    let der = signer.to_pkcs8_der().map_err(|e| e.to_string())?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(file).and_then(|mut out| out.write_all(&der)).map_err(|e| format!("{file}: {e}"))?;
    println!("pub const ROOT: [u8; 32] = {:?};", signer.public_key());
    Ok(())
}

/// Prints a developer key file.
///
/// # Arguments
/// * `root` - the root key file.
/// * `id` - the developer id.
/// * `days` - validity from now.
///
/// # Errors
/// An unreadable root, a bad id or day count.
fn developer(root: &str, id: &str, days: &str) -> Result<(), String> {
    let der = std::fs::read(root).map_err(|e| format!("{root}: {e}"))?;
    let days: u64 = days.parse().map_err(|_| format!("'{days}' is not a number of days"))?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_secs();
    print!("{}", lunarstorm_project::developer::issue(&der, id, now + days * 86_400)?);
    Ok(())
}
