use qjs_compiler_core::job::{self, Error, Limits};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

fn worker() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_qjs-compile-worker"))
}
fn units(source: &str) -> Vec<u16> {
    source.encode_utf16().collect()
}

#[test]
fn isolated_worker_compiles_lossless_source_into_verified_graph() {
    let source = units("function outer(x){let y=2;return function(){return x+y;};}outer(40)();");
    let image = job::compile(
        worker(),
        &source,
        "worker.js",
        Limits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(image.function().source.as_deref(), Some(source.as_slice()));
    assert_eq!(
        image.spans().unwrap().source_digest,
        qjs_bytecode::image::source_digest_utf16(&source)
    );
    assert!(
        image
            .function()
            .constants
            .iter()
            .any(|c| matches!(c, qjs_bytecode::image::Constant::Function(_)))
    );
    let source = [vec![39, 0xd800, 39, 59], units(" // unpaired surrogate")].concat();
    let image = job::compile(
        worker(),
        &source,
        "utf16.js",
        Limits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(image.function().source.as_deref(), Some(source.as_slice()));
    assert_eq!(
        image.spans().unwrap().source_digest,
        qjs_bytecode::image::source_digest_utf16(&source)
    );
}

#[test]
fn source_boundaries_and_preflight_cancellation_do_not_launch_a_worker() {
    let source = units("40+2;");
    let missing = Path::new("/missing/qjs-worker-must-not-launch");
    assert!(matches!(
        job::compile(
            missing,
            &source,
            "x.js",
            Limits::default(),
            &AtomicBool::new(true)
        ),
        Err(Error::Cancelled)
    ));
    let limits = Limits {
        deadline: Duration::ZERO,
        ..Limits::default()
    };
    assert!(matches!(
        job::compile(missing, &source, "x.js", limits, &AtomicBool::new(false)),
        Err(Error::Timeout)
    ));
    for delta in [-1, 0, 1] {
        let limits = Limits {
            source_bytes: (source.len() * 2).checked_add_signed(delta).unwrap(),
            ..Limits::default()
        };
        let result = job::compile(worker(), &source, "x.js", limits, &AtomicBool::new(false));
        assert_eq!(result.is_ok(), delta >= 0, "{result:?}");
    }
    let limits = Limits {
        artifact_bytes: 0,
        ..Limits::default()
    };
    assert!(matches!(
        job::compile(worker(), &source, "x.js", limits, &AtomicBool::new(false)),
        Err(Error::Capacity("worker output"))
    ));
}

#[test]
fn worker_reports_compiler_errors_and_recovers_for_next_request() {
    let error = job::compile(
        worker(),
        &units("let = ;"),
        "broken.js",
        Limits::default(),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(
        matches!(error, Error::Compilation(ref text) if text.contains("broken.js")),
        "{error:?}"
    );
    assert!(
        job::compile(
            worker(),
            &units("42;"),
            "next.js",
            Limits::default(),
            &AtomicBool::new(false)
        )
        .is_ok()
    );
}
#[test]
fn worker_compiles_module_without_linking_or_evaluating_dependencies() {
    let source = units("import {value} from 'missing-dependency';export const answer=value+2;");
    let image = job::compile_module(
        worker(),
        &source,
        "module-job.js",
        Limits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let module = image.module().unwrap();
    assert_eq!(
        module.name,
        "module-job.js".encode_utf16().collect::<Vec<_>>()
    );
    assert_eq!(
        module.required_modules,
        vec!["missing-dependency".encode_utf16().collect::<Vec<_>>()]
    );
    assert_eq!(module.function.source.as_deref(), Some(source.as_slice()));
    assert!(
        job::compile(
            worker(),
            &source,
            "module-job.js",
            Limits::default(),
            &AtomicBool::new(false)
        )
        .is_err()
    );
}

#[test]
fn worker_heap_exhaustion_is_capacity_and_does_not_poison_next_job() {
    let limits = Limits {
        heap_bytes: 16 * 1024,
        ..Limits::default()
    };
    let result = job::compile(
        worker(),
        &units("let answer=42;answer;"),
        "heap.js",
        limits,
        &AtomicBool::new(false),
    );
    assert!(
        matches!(result, Err(Error::Capacity("compiler heap"))),
        "{result:?}"
    );
    assert!(
        job::compile(
            worker(),
            &units("42;"),
            "recovery.js",
            Limits::default(),
            &AtomicBool::new(false)
        )
        .is_ok()
    );
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::atomic::AtomicU64};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Script(PathBuf);
    impl Script {
        fn new(body: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "qjs-job-fixture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }
    impl Drop for Script {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
            let mut companion = self.0.as_os_str().to_os_string();
            companion.push(".image");
            let _ = fs::remove_file(PathBuf::from(companion));
        }
    }
    #[test]
    fn watchdog_terminates_hung_worker_and_cancellation_recovers() {
        let hung = Script::new("sleep 30");
        let start = Instant::now();
        let limits = Limits {
            deadline: Duration::from_millis(100),
            ..Limits::default()
        };
        assert!(matches!(
            job::compile(&hung.0, &[], "hung.js", limits, &AtomicBool::new(false)),
            Err(Error::Timeout)
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
        let flag = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                flag.store(true, Ordering::Release);
            });
            assert!(matches!(
                job::compile(&hung.0, &[], "cancel.js", Limits::default(), &flag),
                Err(Error::Cancelled)
            ));
        });
        assert!(
            job::compile(
                worker(),
                &units("42;"),
                "recover.js",
                Limits::default(),
                &AtomicBool::new(false)
            )
            .is_ok()
        );
    }
    #[test]
    fn success_exit_cannot_bypass_independent_bytecode_verification() {
        let forged = Script::new("printf 'SUCCESS' > \"$2\"; exit 0");
        assert!(matches!(
            job::compile(
                &forged.0,
                &[],
                "forged.js",
                Limits::default(),
                &AtomicBool::new(false)
            ),
            Err(Error::InvalidImage(_))
        ));
        let failure = Script::new("printf 'SUCCESS' >&2; exit 7");
        assert!(matches!(
            job::compile(
                &failure.0,
                &[],
                "failed.js",
                Limits::default(),
                &AtomicBool::new(false)
            ),
            Err(Error::Worker(_))
        ));
    }
    #[test]
    fn verified_worker_output_is_bound_to_original_request() {
        let fixture = Script::new(
            r#"python3 - "$1" "$2" "$0.image" <<'PYCODE'
import sys
from pathlib import Path
request=Path(sys.argv[1]).read_bytes()
payload=Path(sys.argv[3]).read_bytes()
Path(sys.argv[2]).write_bytes(b'QJSRES02'+request[8:9]+request[-32:]+len(payload).to_bytes(4,'little')+payload)
PYCODE"#,
        );
        let mut image_path = fixture.0.as_os_str().to_os_string();
        image_path.push(".image");
        let image_path = PathBuf::from(image_path);
        for (source, filename) in [("41;", "requested.js"), ("42;", "foreign.js")] {
            let foreign = qjs_compiler_core::compile_script(source, filename).unwrap();
            fs::write(&image_path, foreign.serialize_artifact().unwrap()).unwrap();
            let result = job::compile(
                &fixture.0,
                &units("42;"),
                "requested.js",
                Limits::default(),
                &AtomicBool::new(false),
            );
            assert!(matches!(result, Err(Error::InvalidImage(_))), "{result:?}");
        }
        let complete = qjs_compiler_core::compile_script("42;", "requested.js").unwrap();
        let no_spans = qjs_bytecode::image::verify_and_encode(complete.function(), None).unwrap();
        fs::write(&image_path, no_spans.serialize_artifact().unwrap()).unwrap();
        assert!(matches!(
            job::compile(
                &fixture.0,
                &units("42;"),
                "requested.js",
                Limits::default(),
                &AtomicBool::new(false)
            ),
            Err(Error::InvalidImage(_))
        ));
    }
}

#[test]
fn worker_distinguishes_unsupported_from_language_errors_and_recovers() {
    let unsupported = job::compile(
        worker(),
        &units("function later() { return import.source('./x.js'); }"),
        "unsupported.js",
        Limits::default(),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(
        format!("{unsupported:?}").starts_with("Unsupported("),
        "{unsupported:?}"
    );
    let language = job::compile(
        worker(),
        &units("let = ;"),
        "invalid.js",
        Limits::default(),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(language, Error::Compilation(_)), "{language:?}");
    assert!(
        job::compile(
            worker(),
            &units("42;"),
            "recover.js",
            Limits::default(),
            &AtomicBool::new(false)
        )
        .is_ok()
    );
}

#[test]
fn cancellation_kills_worker_while_actual_oxc_parser_is_active_and_recovers() {
    let source = units(&"0;".repeat(1_000_000));
    let cancel = AtomicBool::new(false);
    let mut active_after_kill = false;
    let mut saw_allocation = false;
    let mut pid = 0;
    let result = job::compile_with_progress(
        worker(),
        &source,
        "real-parse.js",
        Limits::default(),
        &cancel,
        &mut |event| {
            saw_allocation = event.parse_allocations > 0;
            pid = event.pid;
            active_after_kill = event.parser_active;
            if event.parser_active {
                cancel.store(true, Ordering::Release);
            }
        },
    );
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert!(
        saw_allocation && active_after_kill && pid != 0,
        "cancellation must interrupt real Parser::parse, not startup or later lowering"
    );
    #[cfg(unix)]
    {
        let state = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        assert!(state.stdout.is_empty(), "worker was not reaped");
    }
    assert!(
        job::compile(
            worker(),
            &units("42;"),
            "after-parse-cancel.js",
            Limits::default(),
            &AtomicBool::new(false)
        )
        .is_ok()
    );
}

#[test]
fn isolated_budget_failures_are_capacity_and_recovery_preserves_script_module_modes() {
    for nodes in [2, 3, 4] {
        let mut limits = Limits::default();
        limits.budgets.frontend.max_ast_nodes = nodes;
        let result = job::compile(
            worker(),
            &units("42;"),
            "nodes.js",
            limits,
            &AtomicBool::new(false),
        );
        if nodes < 3 {
            assert!(
                matches!(result, Err(Error::Capacity("compiler model"))),
                "{result:?}"
            );
        } else {
            assert!(result.is_ok(), "{result:?}");
        }
    }
    let mut limits = Limits::default();
    limits.budgets.emission.max_instructions = 0;
    assert!(matches!(
        job::compile(
            worker(),
            &units("42;"),
            "ir-limit.js",
            limits,
            &AtomicBool::new(false)
        ),
        Err(Error::Capacity(_))
    ));
    let mut limits = Limits::default();
    limits.budgets.image.max_work = 0;
    assert!(matches!(
        job::compile(
            worker(),
            &units("42;"),
            "verify-limit.js",
            limits,
            &AtomicBool::new(false)
        ),
        Err(Error::Capacity(_))
    ));
    assert!(
        job::compile_module(
            worker(),
            &units("export const n=42;"),
            "recovered-module.js",
            Limits::default(),
            &AtomicBool::new(false)
        )
        .is_ok()
    );
}

#[test]
fn isolated_eval_preserves_owned_layout_and_protected_source_metadata() {
    use qjs_bytecode::image::{
        self, ScopeLink,
        eval::{EnvBinding, EnvLayout, verify_env_layout},
    };
    let environment = verify_env_layout(EnvLayout {
        locals: vec![EnvBinding {
            name: units("x"),
            is_lexical: true,
            is_captured: true,
            initialized: true,
            live: true,
            scope_level: 1,
            ..Default::default()
        }],
        var_ref_count: 1,
        scope_head: ScopeLink::Local(0),
        ..Default::default()
    })
    .unwrap();
    let source = units("x=x+2;x;");
    let image = job::compile_eval(
        worker(),
        &source,
        "bounded-eval.js",
        &environment,
        Limits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(image.layout_digest(), environment.digest());
    assert_eq!(image.profile(), image::COMPILER_PROFILE);
    assert_eq!(
        image.spans().unwrap().source_digest,
        image::source_digest_utf16(&source)
    );
    assert_eq!(image.function().captures[0].name, units("x"));
    assert!(
        image::decode_and_verify(image.bytes()).is_err(),
        "no ordinary root proof for external captures"
    );
    let mut limits = Limits::default();
    limits.budgets.frontend.max_ast_nodes = 0;
    assert!(matches!(
        job::compile_eval(
            worker(),
            &source,
            "eval-limit.js",
            &environment,
            limits,
            &AtomicBool::new(false)
        ),
        Err(Error::Capacity(_))
    ));
    assert!(
        job::compile_eval(
            worker(),
            &units("x;"),
            "eval-recover.js",
            &environment,
            Limits::default(),
            &AtomicBool::new(false)
        )
        .is_ok()
    );
}

#[test]
fn eval_worker_keeps_language_and_unsupported_failures_distinct() {
    let environment = qjs_bytecode::image::eval::verify_env_layout(Default::default()).unwrap();
    for (source, unsupported) in [("let = ;", false), ("import.source('./x.js')", true)] {
        let error = job::compile_eval(
            worker(),
            &units(source),
            "eval-errors.js",
            &environment,
            Limits::default(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        if unsupported {
            assert!(matches!(error, Error::Unsupported(_)), "{error:?}");
        } else {
            assert!(matches!(error, Error::Compilation(_)), "{error:?}");
        }
    }
    assert!(
        job::compile_eval(
            worker(),
            &units("42;"),
            "eval-recover.js",
            &environment,
            Limits::default(),
            &AtomicBool::new(false)
        )
        .is_ok()
    );
}

#[test]
fn public_default_request_codec_is_accepted_by_the_worker() {
    let directory = std::env::temp_dir().join(format!("qjs-codec-worker-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let request = directory.join("request");
    let response = directory.join("response");
    let bytes = qjs_compiler_core::job_protocol::encode(&units("42;"), "public-codec.js").unwrap();
    std::fs::write(&request, &bytes).unwrap();
    let result = std::process::Command::new(worker())
        .args([&request, &response])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let success = result.status.success();
    let output = std::fs::read(&response).ok();
    std::fs::remove_dir_all(&directory).unwrap();
    assert!(success, "public request must remain usable: {stderr}");
    let expected: &[u8; 32] = bytes[bytes.len() - 32..].try_into().unwrap();
    let payload = qjs_compiler_core::job_request::read_response(
        output.as_ref().unwrap(),
        qjs_compiler_core::job_request::Mode::Script,
        expected,
    )
    .unwrap();
    assert!(qjs_bytecode::image::deserialize_artifact(payload).is_ok());
}
