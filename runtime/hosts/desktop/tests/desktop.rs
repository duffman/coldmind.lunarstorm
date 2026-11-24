//! The `lunarstorm-desktop` binary end to end: a real tao event loop and a real
//! native window, driven by `examples/selfcheck.js`. It needs a desktop session;
//! the window is on screen for about a third of a second.

use std::path::Path;
use std::process::Command;

#[test]
fn a_script_opens_retitles_and_closes_a_native_window() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/selfcheck.js");
    let output = Command::new(env!("CARGO_BIN_EXE_lunarstorm-desktop")).arg(script).output().expect("lunarstorm-desktop runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stdout,
        "created\nretitled\nrefused: no window named 'Missing'\nrefused: this host has no native method dialog.open\nMain closed\n",
        "stderr: {stderr}"
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
}

#[test]
fn a_script_that_does_not_load_exits_with_1() {
    let directory = std::env::temp_dir().join(format!("lunarstorm-desktop-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temp dir");
    let script = directory.join("broken.js");
    std::fs::write(&script, "let = = 1;").expect("write");
    let output = Command::new(env!("CARGO_BIN_EXE_lunarstorm-desktop")).arg(&script).output().expect("lunarstorm-desktop runs");
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("did not load"));

    let usage = Command::new(env!("CARGO_BIN_EXE_lunarstorm-desktop")).output().expect("lunarstorm-desktop runs");
    assert_eq!(usage.status.code(), Some(2));
}
