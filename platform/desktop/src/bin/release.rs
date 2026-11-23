//! Release runtime: runs the archive attached to its own executable (spec 6.3). Built
//! once, prebuilt for the developer; `lunarstorm-pack` attaches each application's archive.

use lunarstorm_platform::{assets, ui};

/// Reads the attached archive and runs it.
fn main() {
    let build = std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|exe| assets::read_attached(&exe))
        .and_then(|bytes| assets::read_archive(&bytes));
    match build {
        Ok(build) => ui::run(build, ui::Mode::Release, None),
        Err(e) => {
            eprintln!("lunarstorm: {e}");
            let _ = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Error")
                .set_description(format!("This application is damaged or incomplete.\n\n{e}"))
                .show();
            std::process::exit(1);
        }
    }
}
