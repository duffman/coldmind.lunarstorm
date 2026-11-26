//! Direct eval against a frame's lexical layout: reads and writes, shadowing, closures,
//! unavailable bindings and caller strictness.

use qjs_bytecode_image_verifier::image::{
    self, ScopeLink,
    eval::{EnvBinding, EnvLayout, VerifiedEnvLayout, verify_env_layout},
};
use qjs_js_parser_frontend::source::SourceInput;

fn layout() -> VerifiedEnvLayout {
    verify_env_layout(EnvLayout {
        locals: vec![EnvBinding {
            name: "x".encode_utf16().collect(),
            is_lexical: true,
            is_captured: true,
            live: true,
            initialized: true,
            scope_level: 2,
            scope_next: ScopeLink::End,
            var_ref_idx: 0,
            ..Default::default()
        }],
        var_ref_count: 1,
        scope_head: ScopeLink::Local(0),
        ..Default::default()
    })
    .unwrap()
}
fn compile(
    source: &str,
    env: &VerifiedEnvLayout,
) -> Result<image::eval::VerifiedEvalImage, String> {
    let input = SourceInput::from_utf16(&source.encode_utf16().collect::<Vec<_>>()).unwrap();
    qjs_bytecode_emitter::eval::compile_eval(&input, "selected-eval.js", env, false)
}
#[test]
fn lexical_eval_reads_and_writes_only_its_proven_live_slot() {
    let env = layout();
    let image = compile("x=x+2;x;", &env).unwrap();
    let capture = &image.function().captures[0];
    assert_eq!((capture.closure_type, capture.var_idx), (0, 0));
    assert!(capture.is_lexical);
    assert_eq!(image.layout_digest(), env.digest());
    assert!(
        image::decode_and_verify(image.bytes()).is_err(),
        "ordinary loading must reject frame-dependent code"
    );
}
#[test]
fn eval_locals_shadow_external_names_without_global_declarations() {
    let image = compile("let x=40;x+2;", &layout()).unwrap();
    assert!(image.function().captures.is_empty());
    let decoded = qjs_bytecode_image_verifier::decode_schema(
        &image.function().code,
        qjs_bytecode_image_verifier::Limits::default(),
        qjs_bytecode_image_verifier::production::FINAL_OPCODES,
    )
    .unwrap();
    assert!(!decoded.instructions().iter().any(|i| matches!(
        i.opcode().name,
        "define_var" | "check_define_var" | "define_func"
    )));
}
#[test]
fn nested_ordinary_function_captures_eval_reference_without_inheriting_its_this() {
    let image = compile("let f=function(){return x;};f();", &layout()).unwrap();
    let child = image
        .function()
        .constants
        .iter()
        .find_map(|c| {
            if let image::Constant::Function(f) = c {
                Some(f)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        child
            .captures
            .iter()
            .any(|c| c.closure_type == 2 && c.name == "x".encode_utf16().collect::<Vec<_>>())
    );
}
#[test]
fn unavailable_frame_bindings_and_unimplemented_eval_environments_fail_closed() {
    for source in ["this;", "var y=1;y;", "function f(){return 1;}f();"] {
        assert!(compile(source, &layout()).is_err(), "{source}");
    }
    let mut env = layout().layout().clone();
    env.flags.arguments_allowed = true;
    let env = verify_env_layout(env).unwrap();
    assert!(compile("arguments;", &env).is_err());
    let mut env = layout().layout().clone();
    env.locals[0].live = false;
    let env = verify_env_layout(env).unwrap();
    assert!(compile("x;", &env).is_err());
}
#[test]
fn caller_strictness_is_never_silently_lowered() {
    let mut env = layout().layout().clone();
    env.strict = true;
    let env = verify_env_layout(env).unwrap();
    let inherited = compile("x+2;", &env).unwrap();
    assert!(inherited.function().strict);
    assert_eq!(
        inherited.function().source.as_deref(),
        Some("x+2;".encode_utf16().collect::<Vec<_>>().as_slice())
    );
    for source in ["let eval=1;eval;", "010;", "function f(a,a){}f();"] {
        assert!(
            compile(source, &env).is_err(),
            "inherited strict early error missing: {source}"
        );
    }
    let image = compile("'use strict';x+2;", &env).unwrap();
    assert!(image.function().strict);
}

#[test]
fn rust_source_eval_mutates_live_native_argument_local_and_inherited_reference() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let root=qjs_bytecode_emitter::compile_script(
        "function outer(z){return function caller(x){let y=2;probeEval();return x+y+z;};}outer(100)(40);",
        "caller.js",
    ).unwrap();
    fn find(f: &image::FunctionImage) -> Option<&image::FunctionImage> {
        if f.function_name == "caller" {
            return Some(f);
        }
        f.constants.iter().find_map(|c| match c {
            image::Constant::Function(child) => find(child),
            _ => None,
        })
    }
    let caller = find(root.function()).unwrap();
    let binding = |v: &image::VarDef| EnvBinding {
        name: v.name.clone(),
        is_const: v.is_const,
        is_lexical: v.is_lexical,
        var_kind: v.var_kind,
        scope_level: v.scope_level,
        scope_next: v.scope_next,
        is_captured: v.is_captured,
        var_ref_idx: v.var_ref_idx,
        initialized: true,
        live: v.is_captured,
    };
    let locals: Vec<_> = caller.locals[caller.arg_count as usize..]
        .iter()
        .map(binding)
        .collect();
    let y = locals
        .iter()
        .position(|v| v.name == "y".encode_utf16().collect::<Vec<_>>())
        .unwrap();
    let env = verify_env_layout(EnvLayout {
        args: caller.locals[..caller.arg_count as usize]
            .iter()
            .map(binding)
            .collect(),
        locals,
        refs: caller
            .captures
            .iter()
            .map(|v| EnvBinding {
                name: v.name.clone(),
                is_const: v.is_const,
                is_lexical: v.is_lexical,
                var_kind: v.var_kind,
                initialized: true,
                live: true,
                ..Default::default()
            })
            .collect(),
        var_ref_count: caller.var_ref_count,
        scope_head: ScopeLink::Local(y as u32),
        strict: caller.strict,
        flags: caller.flags.clone(),
        ..Default::default()
    })
    .unwrap();
    let eval = compile("x=x+1;y=y+1;z=z+1;x+y+z;", &env).unwrap();
    assert_eq!(
        eval.function()
            .captures
            .iter()
            .map(|c| c.closure_type)
            .collect::<Vec<_>>(),
        vec![1, 0, 2]
    );
    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let vendor = workspace.join("vendor/quickjs-ng");
    let directory = std::env::temp_dir().join(format!("qjs-source-eval-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    let executable = directory.join("probe");
    let output = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()))
        .args(["-std=c11", "-D_GNU_SOURCE", "-DQJS_DISABLE_PARSER", "-I"])
        .arg(&vendor)
        .arg(workspace.join("bytecode-image-verifier/tools/checksum_probe.c"))
        .args([
            vendor.join("dtoa.c"),
            vendor.join("libregexp.c"),
            vendor.join("libunicode.c"),
        ])
        .args(["-lm", "-pthread", "-o"])
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut bytes = (root.bytes().len() as u32).to_le_bytes().to_vec();
    bytes.extend_from_slice(root.bytes());
    bytes.extend_from_slice(eval.bytes());
    assert!(bytes.len() < 4096);
    let mut child = Command::new(&executable)
        .arg("eval")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "145");
}
