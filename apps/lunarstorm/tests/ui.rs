//! End-to-end tests of the command-line interface (`src/cli.rs` and the
//! `lunarstorm-cliui` package): help, version,
//! usage errors, color control, error reports and the `lunarstorm:ui` module.
//! Each test runs the real `lunarstorm` binary; the last one runs it on a
//! pseudo-terminal to exercise the animated path.

// Every test here needs `run` and `lunarstorm:ui`: only the `runtime` package has them.
#![cfg(feature = "runtime")]

use std::fs::File;
use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};
use std::process::{Command, Output, Stdio};

/// Builds a `lunarstorm` command with a known color environment.
///
/// # Arguments
/// * `args` - command-line arguments.
/// * `env` - variables to set after `NO_COLOR`, `FORCE_COLOR` and `COLORTERM` are
///   cleared and `TERM` is set to `xterm`.
///
/// # Returns
/// The command, not yet spawned.
fn command(args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lunarstorm"));
    command.args(args).env_remove("NO_COLOR").env_remove("FORCE_COLOR").env_remove("COLORTERM").env("TERM", "xterm");
    command.envs(env.iter().copied());
    command
}

/// Runs `lunarstorm` with piped output.
///
/// # Arguments
/// * `args` - command-line arguments.
/// * `env` - see [`command`].
///
/// # Returns
/// `(exit status, stdout, stderr)`.
fn run(args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let Output { status, stdout, stderr } = command(args, env).output().expect("spawn lunarstorm");
    (
        status.code().expect("exit status"),
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

/// Script that finishes one progress bar and one spinner.
const BAR_SCRIPT: &str = "
import { progress, spinner } from 'lunarstorm:ui';
const bar = progress({ total: 3, label: 'build' });
const spin = spinner('link');
console.log('between');
let n = 0;
const timer = setInterval(() => {
  bar.tick();
  if (++n === 3) { clearInterval(timer); spin.succeed('linked'); }
}, 60);
";

#[test]
fn help_and_version_are_plain_when_piped() {
    let (code, out, err) = run(&["--version"], &[]);
    assert_eq!((code, out.as_str(), err.as_str()), (0, concat!("lunarstorm ", env!("CARGO_PKG_VERSION"), "\n"), ""));

    let (code, out, _) = run(&["--help"], &[]);
    assert_eq!(code, 0);
    assert!(!out.contains('\x1b'), "{out:?}");
    for part in ["☾ lunarstorm v", "USAGE", "OPTIONS", "--color <when>", "lunarstorm:ui", "NO_COLOR"] {
        assert!(out.contains(part), "missing {part:?} in {out}");
    }

    // No script: the same screen on stderr, status 2.
    let (code, out, err) = run(&[], &[]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.contains("USAGE"), "{err}");
}

#[test]
fn color_follows_flag_then_force_color_then_no_color() {
    let (_, basic, _) = run(&["--color=always", "--help"], &[]);
    assert!(basic.contains("\x1b[1;94m☾ lunarstorm\x1b[0m"), "{basic:?}");
    let (_, indexed, _) = run(&["--color", "always", "--help"], &[("TERM", "xterm-256color")]);
    assert!(indexed.contains("\x1b[38;5;"), "{indexed:?}");
    let (_, truecolor, _) = run(&["--color=always", "--version"], &[("COLORTERM", "truecolor")]);
    assert!(truecolor.contains("\x1b[38;2;167;139;250m☾"), "{truecolor:?}");
    assert!(truecolor.contains("quickjs-ng"), "{truecolor:?}");

    let log = "console.log({ a: 1, b: 'x', c: null, d: undefined, e: true }, 'top')";
    let colored = "{ a: \x1b[33m1\x1b[0m, b: \x1b[32m'x'\x1b[0m, c: \x1b[1mnull\x1b[0m, d: \x1b[90mundefined\x1b[0m, e: \x1b[33mtrue\x1b[0m } top\n";
    let plain = "{ a: 1, b: 'x', c: null, d: undefined, e: true } top\n";
    assert_eq!(run(&["-e", log], &[("FORCE_COLOR", "1")]).1, colored);
    assert_eq!(run(&["-e", log], &[("FORCE_COLOR", "1"), ("NO_COLOR", "1")]).1, colored);
    assert_eq!(run(&["--color=never", "-e", log], &[("FORCE_COLOR", "1")]).1, plain);
    assert_eq!(run(&["-e", log], &[("FORCE_COLOR", "0")]).1, plain);
    assert_eq!(run(&["-e", log], &[]).1, plain);
}

#[test]
fn usage_errors_exit_2_with_a_hint() {
    let (code, out, err) = run(&["--verison"], &[]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert_eq!(err, "error: unknown option '--verison'\n  hint: did you mean '--version'?\n  usage: lunarstorm --help\n");

    let (code, _, err) = run(&["--zzzzzz"], &[]);
    assert_eq!(code, 2);
    assert_eq!(err, "error: unknown option '--zzzzzz'\n  usage: lunarstorm --help\n");

    let (code, _, err) = run(&["--color=sometimes", "x.js"], &[]);
    assert_eq!(code, 2);
    assert!(err.contains("invalid value 'sometimes' for --color") && err.contains("auto, always or never"), "{err}");

    let (code, _, err) = run(&["--color=always", "-e"], &[]);
    assert_eq!(code, 2);
    assert!(err.contains("\x1b[1;91merror:\x1b[0m") && err.contains("-e needs an argument"), "{err:?}");
}

#[test]
fn options_after_the_script_belong_to_the_script() {
    let (code, out, _) = run(&["--color=never", "-e", "console.log(process.argv.slice(2).join(' '))", "--help", "--color=x"], &[]);
    assert_eq!((code, out.as_str()), (0, "--help --color=x\n"));
}

#[test]
fn uncaught_errors_get_a_code_frame_on_color_streams() {
    let dir = std::env::temp_dir().join(format!("lunarstorm-ui-frame-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let script = dir.join("boom.js");
    std::fs::write(&script, "function inner(x) {\n  const y = x * 2;\n\tthrow new RangeError('kaboom ' + y);\n}\ninner(21);\n").expect("write script");
    let path = script.to_str().expect("utf-8 path");

    let (code, _, plain) = run(&[path], &[]);
    assert_eq!(code, 1);
    assert!(plain.starts_with("Uncaught (in promise) RangeError: kaboom 42\n    at inner ("), "{plain}");
    assert!(!plain.contains('\x1b') && !plain.contains('│'), "{plain:?}");

    let (code, _, pretty) = run(&["--color=always", path], &[]);
    assert_eq!(code, 1);
    for part in [
        "\x1b[1;91m✖ Uncaught (in promise) RangeError\x1b[0m: \x1b[1mkaboom 42\x1b[0m\n",
        // Approved 2026-11-24: the Rust compiler places the throw at `new` (3:8),
        // as Node does; the removed C parser placed it at `y` (3:35).
        "boom.js:3:8\x1b[0m\n",
        "\x1b[2m2 │\x1b[0m \x1b[2m  const y = x * 2;\x1b[0m\n",
        "\x1b[1;91m3\x1b[0m \x1b[2m│\x1b[0m  throw new RangeError('kaboom ' + y);\n",
        "\x1b[2m  │\x1b[0m        \x1b[1;91m^\x1b[0m\n",
        "\x1b[2m4 │\x1b[0m \x1b[2m}\x1b[0m\n",
        "\x1b[2m    at inner (",
    ] {
        assert!(pretty.contains(part), "missing {part:?} in {pretty:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ui_module_prints_only_final_lines_when_piped() {
    let (code, out, err) = run(&["-e", BAR_SCRIPT], &[]);
    assert_eq!((code, out.as_str()), (0, "between\n"));
    assert!(!err.contains('\x1b'), "{err:?}");
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines.len(), 2, "{err:?}");
    assert!(lines[0].starts_with("✔ build  3 in 0."), "{err:?}");
    assert!(lines[1].starts_with("✔ linked 0."), "{err:?}");

    let style = "import { style } from 'lunarstorm:ui'; console.log(style.enabled, style.bold('b') + style.gradient('ab'))";
    assert_eq!(run(&["-e", style], &[]).1, "false bab\n");
    let (_, out, _) = run(&["--color=always", "-e", style], &[("COLORTERM", "truecolor")]);
    assert_eq!(out, "\x1b[33mtrue\x1b[0m \x1b[1mb\x1b[0m\x1b[38;2;167;139;250ma\x1b[38;2;34;211;238mb\x1b[0m\n");
}

#[test]
fn hostapi_lists_ui() {
    let (code, out, err) = run(&["-e", "import { areas } from 'lunarstorm:hostapi'; console.log(areas.join())"], &[]);
    assert_eq!((code, out.as_str(), err.as_str()), (0, "ui\n", ""));
}

#[test]
fn ui_alias_is_the_same_module() {
    let script = "import ui, { style } from 'lunarstorm:ui'; import hostui, { style as hoststyle } from 'lunarstorm:hostapi/ui';
                  console.log(style === hoststyle, ui === hostui, typeof hostui.progress)";
    let (code, out, err) = run(&["-e", script], &[]);
    assert_eq!((code, out.as_str(), err.as_str()), (0, "true true function\n", ""));
}

#[test]
fn ui_module_animates_on_a_terminal_and_restores_the_cursor() {
    // A pseudo-terminal 100 columns wide stands in for the user's terminal.
    let (mut master, mut slave) = (0, 0);
    let size = libc::winsize { ws_row: 30, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: openpty fills the two descriptors; the name and termios are not requested.
    let opened = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &size as *const _ as *mut _) };
    assert_eq!(opened, 0, "openpty failed");
    // SAFETY: both descriptors were just opened and are owned from here on.
    let (master, slave) = unsafe { (File::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };

    let mut child = command(&["-e", BAR_SCRIPT], &[("COLORTERM", "truecolor")])
        .stdin(Stdio::null())
        .stdout(slave.try_clone().expect("clone pty"))
        .stderr(slave.try_clone().expect("clone pty"))
        .spawn()
        .expect("spawn lunarstorm");
    // The reader runs while the child does so the pty buffer never fills. This
    // process keeps `slave` open until the child has exited, so no output is
    // discarded; dropping it afterwards ends the read loop.
    let reader = std::thread::spawn(move || {
        let (mut master, mut seen, mut chunk) = (master, Vec::new(), [0u8; 4096]);
        while let Ok(n @ 1..) = master.read(&mut chunk) {
            seen.extend_from_slice(&chunk[..n]);
            if seen.ends_with(b"\x1b[?25h") {
                break;
            }
        }
        String::from_utf8_lossy(&seen).into_owned()
    });
    let status = child.wait().expect("wait for lunarstorm");
    drop(slave);
    let seen = reader.join().expect("reader thread");

    assert_eq!(status.code(), Some(0), "{seen:?}");
    assert!(seen.starts_with("\x1b[?25l"), "cursor not hidden first: {seen:?}");
    assert!(seen.ends_with("\x1b[?25h"), "cursor not restored last: {seen:?}");
    assert!(seen.contains("\x1b[38;2;167;139;250m━"), "no gradient bar: {seen:?}");
    assert!(seen.contains("⠋\x1b[0m link"), "no spinner frame: {seen:?}");
    assert!(seen.contains("\x1b[2A"), "two-line region was not redrawn in place: {seen:?}");
    // console.log output lands above the region: erase, line, redraw.
    assert!(seen.contains("\x1b[2A\x1b[Jbetween\r\n\r\x1b[2K"), "{seen:?}");
    assert!(seen.contains("✔\x1b[0m build") && seen.contains("✔\x1b[0m linked"), "{seen:?}");
}
