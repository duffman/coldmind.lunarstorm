//! Runs the real `lunarstation` binary: its name, version and mounted packages.

use std::process::Command;

/// Runs `lunarstation` with `args`.
///
/// # Arguments
/// * `args` - the command-line words.
///
/// # Returns
/// `(exit status, stdout)`.
fn run(args: &[&str]) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_lunarstation")).args(args).env("NO_COLOR", "1").output().expect("spawn lunarstation");
    (output.status.code().expect("exit status"), String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `--version` names `lunarstation`, not `lunar`.
#[test]
fn version_names_lunarstation() {
    let (status, out) = run(&["--version"]);
    assert_eq!(status, 0);
    assert!(out.contains("lunarstation"), "missing program name in {out}");
}

/// Help lists every package mounted by the default features under the
/// `lunarstation` name, plus the host's own commands.
#[cfg(all(feature = "runtime", feature = "build", feature = "frontend", feature = "project"))]
#[test]
fn help_lists_every_mounted_package() {
    let (status, out) = run(&["--help"]);
    assert_eq!(status, 0);
    for part in ["lunarstation", "run a script", "build a desktop project", "serve", "project", "describe", "completions"] {
        assert!(out.contains(part), "missing {part:?} in {out}");
    }
    let (status, out) = run(&["build", "--help"]);
    assert_eq!(status, 0);
    assert!(out.contains("lunarstation build [options] [<command>]"), "usage not under lunarstation in {out}");
}
