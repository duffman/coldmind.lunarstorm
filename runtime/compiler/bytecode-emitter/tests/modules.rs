use qjs_bytecode_emitter::compile_module;
#[test]
fn source_modules_have_verified_two_phase_images() {
    for source in [
        "export const answer=42;",
        "export var answer=40;answer+=2;",
        "export function answer(){return 42;}",
        "export default 42;",
        "export default function(){return 42;}",
        "let value=42;export{value as answer};",
        "import value,{x as y}from'dep';export const answer=value+y;",
        "import *as ns from'dep';export const answer=ns.answer;",
        "export{x as answer}from'dep';export*from'other';export*as namespace from'dep';",
    ] {
        let image =
            compile_module(source, "main.mjs").unwrap_or_else(|error| panic!("{source}: {error}"));
        assert!(image.module().is_some());
        assert!(image.function().strict);
        assert_eq!(image.function().flags.kind, 2);
    }
}
#[test]
fn import_assignment_emits_abrupt_completion_not_import_write() {
    compile_module("import value from'dep';value=42;", "main.mjs").unwrap();
}

#[test]
fn source_modules_execute_and_link_in_parser_disabled_native_vm() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = root.join("..");
    let vendor = workspace.join("vendor/quickjs-ng");
    let directory = std::env::temp_dir().join(format!("qjs-source-modules-{}", std::process::id()));
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
        .arg(root.join("tools/module_probe.c"))
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
    let run = |mode: &str, input: &[u8], expected: Option<&str>| {
        assert!(input.len() < 4096, "shared probe input capacity");
        let mut child = Command::new(&executable)
            .arg(mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let output = child.wait_with_output().unwrap();
        if let Some(expected) = expected {
            assert!(
                output.status.success(),
                "native module {:?}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), expected);
        } else {
            assert_eq!(
                output.status.code(),
                Some(5),
                "module must reject its evaluation promise: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    };
    for source in [
        "export const answer=42;",
        "export var answer=40;answer+=2;",
        "export function value(){return 42;}export const answer=value();",
        "export let count=40;export function bump(){count++;}bump();bump();export{count as answer};",
    ] {
        let image = compile_module(source, "main.mjs").unwrap();
        run("module", image.bytes(), Some("42"));
    }
    for (dependency, main, expected) in [
        (
            "let x=40;export default x;x=42;",
            "import value from'dependency.mjs';export const answer=value+2;export{value as forwarded};",
            "42 40",
        ),
        (
            "let x=40;export{x as default};x=41;",
            "import value from'dependency.mjs';export const answer=value+1;export{value as forwarded};",
            "42 41",
        ),
        (
            "export default function(){return 41;}",
            "import value from'dependency.mjs';export const answer=value()+1;export const forwarded=value();",
            "42 41",
        ),
        (
            "export const answer=41;",
            "import{answer as value}from'dependency.mjs';export const answer=value+1;export{value as forwarded};",
            "42 41",
        ),
        (
            "export const answer=41;",
            "import*as ns from'dependency.mjs';export const answer=ns.answer+1;export const forwarded=ns.answer;",
            "42 41",
        ),
        (
            "export default 41;",
            "import value from'dependency.mjs';export const answer=value+1;export{value as forwarded};",
            "42 41",
        ),
        (
            "export let answer=40;export function bump(){answer++;}",
            "import{answer as value,bump}from'dependency.mjs';bump();bump();export const answer=value;export{value as forwarded};",
            "42 42",
        ),
        (
            "export const answer=41;",
            "export{answer as forwarded}from'dependency.mjs';export*from'dependency.mjs';",
            "41 41",
        ),
        (
            "import{get}from'main.mjs';export const answer=get();",
            "export function get(){return 41;}import{answer as value}from'dependency.mjs';export const answer=value+1;export{value as forwarded};",
            "42 41",
        ),
    ] {
        let dependency = compile_module(dependency, "dependency.mjs").unwrap();
        let main = compile_module(main, "main.mjs").unwrap();
        let mut input = (dependency.bytes().len() as u32).to_ne_bytes().to_vec();
        input.extend(dependency.bytes());
        input.extend(main.bytes());
        run("graph", &input, Some(expected));
    }
    for (dependency, main) in [
        (
            "export const answer=41;",
            "import{answer as value}from'dependency.mjs';value=42;export const answer=value;export const forwarded=value;",
        ),
        (
            "import{answer}from'main.mjs';export const value=answer;",
            "import{value}from'dependency.mjs';export const answer=value;export const forwarded=value;",
        ),
    ] {
        let dependency = compile_module(dependency, "dependency.mjs").unwrap();
        let main = compile_module(main, "main.mjs").unwrap();
        let mut input = (dependency.bytes().len() as u32).to_ne_bytes().to_vec();
        input.extend(dependency.bytes());
        input.extend(main.bytes());
        run("graph", &input, None);
    }
    let tdz = compile_module("export const answer=value;let value=42;", "main.mjs").unwrap();
    run("module", tdz.bytes(), None);
}
/// Import attributes reach the image and survive its byte round trip, so the
/// engine's loader receives them; a specifier imported again with a
/// different attribute set is rejected, not silently merged.
#[test]
fn import_attributes_are_carried_in_the_module_image() {
    let u = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let image = compile_module(
        "import a from'cfg'with{type:'json','encoding':'utf-8'};import b from'cfg'with{encoding:'utf-8',type:'json'};import c from'plain';export const x=[a,b,c];",
        "main.mjs",
    )
    .unwrap();
    let expected = vec![
        vec![(u("type"), u("json")), (u("encoding"), u("utf-8"))],
        vec![],
    ];
    assert_eq!(image.module().unwrap().required_modules, vec![u("cfg"), u("plain")]);
    assert_eq!(image.module().unwrap().required_attributes, expected);
    let decoded = qjs_bytecode_image_verifier::image::decode_and_verify(image.bytes()).unwrap();
    assert_eq!(decoded.module().unwrap().required_attributes, expected);
    let error = compile_module(
        "import a from'cfg'with{type:'json'};import b from'cfg';export const x=[a,b];",
        "main.mjs",
    )
    .unwrap_err();
    assert!(error.contains("conflicting import attributes"), "{error}");
}
