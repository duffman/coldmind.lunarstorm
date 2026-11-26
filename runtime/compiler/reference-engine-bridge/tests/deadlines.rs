#![cfg(unix)]
use qjs_reference_engine_bridge::ReferenceRunner;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Helper(PathBuf);
impl Helper {
    fn new(body: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "qjs-deadline-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        let f = p.join("reference.sh");
        fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o700)).unwrap();
        Self(p)
    }
    fn runner(&self) -> ReferenceRunner {
        ReferenceRunner::new(self.0.join("reference.sh"))
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn reference_external_deadline_prevents_a_late_success() {
    let h = Helper::new("cat >/dev/null\nsleep 6\nprintf 'number\\t42\\n'");
    let start = Instant::now();
    let result = h.runner().evaluate("42");
    assert!(
        result.is_err(),
        "late success must not escape external deadline: {result:?}"
    );
    assert!(result.unwrap_err().to_string().contains("deadline"));
    assert!(start.elapsed().as_secs() < 6);
}

fn bounded(h: &Helper, input: usize, output: usize) -> ReferenceRunner {
    h.runner().with_limits(qjs_reference_engine_bridge::Limits {
        timeout: qjs_reference_engine_bridge::Limits::default().timeout,
        max_input_bytes: input,
        max_output_bytes: output,
    })
}
// Deadline probes deliberately use a short budget; capacity/protocol probes use
// the unchanged production default, avoiding startup contention masking their signal.
fn short_deadline(h: &Helper, input: usize, output: usize) -> ReferenceRunner {
    h.runner().with_limits(qjs_reference_engine_bridge::Limits {
        timeout: std::time::Duration::from_secs(1),
        max_input_bytes: input,
        max_output_bytes: output,
    })
}
#[test]
fn zero_deadline_rejects_before_spawning_a_fast_success() {
    let h = Helper::new("printf marker > \"$0.spawned\"\nprintf 'number\\t42\\n'");
    let runner = h.runner().with_limits(qjs_reference_engine_bridge::Limits {
        timeout: std::time::Duration::ZERO,
        ..qjs_reference_engine_bridge::Limits::default()
    });
    assert!(matches!(
        runner.evaluate("42"),
        Err(qjs_reference_engine_bridge::ReferenceError::Timeout { .. })
    ));
    assert!(!h.0.join("reference.sh.spawned").exists());
}
#[test]
fn blocked_stdin_and_stdout_cannot_evade_deadline() {
    let h = Helper::new("sleep 6");
    let now = Instant::now();
    assert!(matches!(
        short_deadline(&h, 3 * 1024 * 1024, 1024).evaluate(&"x".repeat(2 * 1024 * 1024)),
        Err(qjs_reference_engine_bridge::ReferenceError::Timeout { .. })
    ));
    assert!(now.elapsed().as_secs() < 2);
    let h = Helper::new("cat >/dev/null\nprintf 'number\\t'\nsleep 6");
    assert!(matches!(
        short_deadline(&h, 1024, 1024).evaluate("42"),
        Err(qjs_reference_engine_bridge::ReferenceError::Timeout { .. })
    ));
}
#[test]
fn completed_parent_cannot_leave_descendant_pipes_hanging() {
    let h = Helper::new("cat >/dev/null\n(sleep 6; printf leaked > \"$0.descendant-finished\") &\nprintf '%s\\n' \"$!\" > \"$0.descendant-pid\"\nprintf 'number\\t42\\n'");
    assert_eq!(
        bounded(&h, 10, 100).evaluate("42").unwrap(),
        qjs_reference_engine_bridge::ReferenceValue::Number(42.0)
    );
    // The runner enforces its actual five-second deadline. An unrelated two-
    // second wall-clock assertion was flaky under scheduler/compiler load.
    // Check the intended cleanup directly instead of weakening its deadline.
    assert!(!h.0.join("reference.sh.descendant-finished").exists());
    let pid = fs::read_to_string(h.0.join("reference.sh.descendant-pid")).unwrap();
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let state = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid.trim()])
            .output()
            .unwrap();
        assert!(
            state.stderr.is_empty(),
            "ps failed: {}",
            String::from_utf8_lossy(&state.stderr)
        );
        let state = String::from_utf8(state.stdout).unwrap();
        if state.trim().is_empty() || state.trim_start().starts_with('Z') {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "descendant {} remains live: {state}",
            pid.trim()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
#[test]
fn nonzero_exit_never_becomes_success_and_runner_recovers() {
    let h = Helper::new("cat >/dev/null\nprintf 'number\\t42\\n'\nexit 7");
    assert!(matches!(
        bounded(&h, 10, 100).evaluate("42"),
        Err(qjs_reference_engine_bridge::ReferenceError::Execution { .. })
    ));
    let h = Helper::new("cat >/dev/null\nprintf 'number\\t42\\n'");
    assert_eq!(
        bounded(&h, 10, 100).evaluate("42").unwrap(),
        qjs_reference_engine_bridge::ReferenceValue::Number(42.0)
    );
}
#[test]
fn input_and_combined_output_caps_have_exact_boundaries() {
    use qjs_reference_engine_bridge::{ReferenceError, ReferenceValue};
    let h = Helper::new("cat >/dev/null\nprintf 'number\\t42\\n'");
    assert_eq!(
        bounded(&h, 2, 10).evaluate("42").unwrap(),
        ReferenceValue::Number(42.0)
    );
    assert!(matches!(
        bounded(&h, 1, 10).evaluate("42"),
        Err(ReferenceError::Capacity {
            resource: "input",
            ..
        })
    ));
    assert!(matches!(
        bounded(&h, 2, 9).evaluate("42"),
        Err(ReferenceError::Capacity {
            resource: "output",
            ..
        })
    ));
    let h = Helper::new("cat >/dev/null\nprintf 'number\\t42\\n'\nprintf x >&2");
    let combined = bounded(&h, 2, 10).evaluate("42");
    assert!(
        matches!(
            &combined,
            Err(ReferenceError::Capacity {
                resource: "output",
                ..
            })
        ),
        "combined output result: {combined:?}"
    );
}
#[test]
fn file_input_is_bounded_before_reference_execution() {
    let h = Helper::new("cat >/dev/null\nprintf 'number\\t42\\n'");
    let file = h.0.join("input.js");
    fs::write(&file, b"42").unwrap();
    assert!(matches!(
        bounded(&h, 1, 100).evaluate_file(&file),
        Err(qjs_reference_engine_bridge::ReferenceError::Capacity {
            resource: "input",
            ..
        })
    ));
    assert_eq!(
        bounded(&h, 2, 100).evaluate_file(&file).unwrap(),
        qjs_reference_engine_bridge::ReferenceValue::Number(42.0)
    );
}
#[test]
fn named_stdin_snapshot_keeps_original_filename_in_errors() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    assert!(std::process::Command::new("sh")
        .arg(root.join("scripts/build-reference.sh"))
        .status()
        .unwrap()
        .success());
    let h = Helper::new(":");
    let file = h.0.join("diagnostic-origin.js");
    fs::write(&file, b"throw new Error('expected')").unwrap();
    let error = ReferenceRunner::new(root.join("build/qjs-reference-engine-bridge"))
        .evaluate_file(&file)
        .unwrap_err()
        .to_string();
    assert!(error.contains("diagnostic-origin.js"), "{error}");
}

#[test]
fn nonregular_file_input_cannot_block_snapshot_open() {
    let h = Helper::new("cat >/dev/null");
    let fifo = h.0.join("source.fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let now = Instant::now();
    assert!(matches!(
        h.runner().evaluate_file(&fifo),
        Err(qjs_reference_engine_bridge::ReferenceError::Io(_))
    ));
    assert!(now.elapsed().as_secs() < 1);
}
