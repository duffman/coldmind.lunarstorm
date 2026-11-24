use qjs_compiler_core::compile_script;

#[test]
fn script_completion_globals_and_coercion_have_verified_images() {
    for source in [
        "1+41",
        "let x=40; x+2;",
        "const x='4'; x+2;",
        "var x; x=42; x;",
        "false || 42",
        "null ?? 42",
        "if (true) 42; else 0;",
    ] {
        compile_script(source, "script.js").unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}

#[test]
fn functions_parameters_and_lexical_closures_have_verified_images() {
    for source in [
        "function add(x){return x+2;} add(40);",
        "function outer(x){let y=2;return function(){return x+y;};} outer(40)();",
        "function f(){1;} f();",
        "function f(x){if(x)return 42;return 0;}f(true);",
    ] {
        compile_script(source, "functions.js").unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}

#[test]
fn dead_unsupported_syntax_is_rejected_before_emission() {
    assert!(compile_script("if(false){import.source('./x.js')};42", "dead.js").is_err());
}

#[test]
fn object_array_and_member_call_images_are_verified() {
    for source in [
        "({answer:42}).answer",
        "[40,2][0]+2",
        "let o={x:42,f:function(){return this.x;}};o.f();",
    ] {
        compile_script(source, "objects.js").unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}

#[test]
fn while_loop_has_balanced_native_control_flow() {
    compile_script("let x=0;while(x<42){x=x+1;}x;", "loop.js").unwrap();
}

#[test]
fn property_write_preserves_assignment_value() {
    compile_script("let o={x:0};(o.x=42)+o.x;", "write.js").unwrap();
}

#[test]
fn lexical_scope_links_use_local_indexes_after_argument_prefix() {
    let image = compile_script(
        "function f(a,b){let x=1;let y=2;{let z=3;return a+b+x+y+z;}}f(17,19);",
        "scope.js",
    )
    .unwrap();
    let qjs_bytecode::image::Constant::Function(function) = &image.function().constants[0] else {
        panic!()
    };
    let x = function
        .locals
        .iter()
        .find(|v| v.name == vec![b'x' as u16])
        .unwrap();
    let y = function
        .locals
        .iter()
        .find(|v| v.name == vec![b'y' as u16])
        .unwrap();
    let z = function
        .locals
        .iter()
        .find(|v| v.name == vec![b'z' as u16])
        .unwrap();
    assert_eq!(x.scope_level, 1);
    let local = |name: &str| {
        function.locals[function.arg_count as usize..]
            .iter()
            .position(|v| v.name == name.encode_utf16().collect::<Vec<_>>())
            .unwrap() as u32
    };
    assert_eq!(y.scope_next.local_index(), Some(local("x")));
    assert_eq!(z.scope_next.local_index(), Some(local("y")));
}

#[test]
fn named_function_expression_and_inferred_names_are_verified() {
    for source in [
        "let f=function recur(x){if(x)return recur(false);return 42;};f(true);",
        "let f=function(){return 42;};f.name;",
        "let f=function self(){self=0;return 42;};f();",
    ] {
        compile_script(source, "names.js").unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}

#[test]
fn loop_control_and_conditional_values_are_verified() {
    for source in [
        "let x=0;while(true){let y=x;x=x+1;if(x<42)continue;break;}x;",
        "let x=0;do{x=x+1;}while(x<42);x;",
        "let x=0;(x=1,true)?(x=42,x):0;",
    ] {
        compile_script(source, "control.js").unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}

#[test]
fn bigint_literals_keep_unbounded_integer_value() {
    for source in ["40n+2n", "0xffn", "18446744073709551616n", "0n", "-129n"] {
        compile_script(source, "bigint.js").unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}

#[test]
fn for_loop_uses_native_numeric_updates_and_iteration_closures() {
    for source in [
        "let result=0;for(let i=0;i<42;i++){result++;}result;",
        "let f;for(let i=0;i<2;++i){f=function(){return i;};}f();",
        "let x='40';++x;x++;x;",
    ] {
        compile_script(source, "for.js").unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}

#[test]
fn dynamic_compound_assignment_keeps_javascript_coercion() {
    for source in [
        "let x=40;x+=2;x;",
        "let x='4';x+=2;x;",
        "let x=84n;x/=2n;x;",
    ] {
        compile_script(source, "compound.js").unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}

#[test]
fn templates_convert_each_substitution_before_the_next() {
    for source in ["`answer: ${42}`", r"`\ud800${40+2}`", "`${1}${2}`"] {
        compile_script(source, "template.js").unwrap_or_else(|e| panic!("{source}: {e}"));
    }
}

#[test]
fn named_function_binding_has_production_function_name_kind() {
    let image = compile_script(
        "let f=function self(){'use strict';return self;};f();",
        "self.js",
    )
    .unwrap();
    let qjs_bytecode::image::Constant::Function(function) = &image.function().constants[0] else {
        panic!()
    };
    let self_binding = function
        .locals
        .iter()
        .find(|v| v.name == "self".encode_utf16().collect::<Vec<_>>())
        .unwrap();
    assert_eq!(self_binding.var_kind, 4);
    assert!(self_binding.is_const);
    assert!(!self_binding.is_lexical);
}

#[test]
fn nested_closure_preserves_strict_function_name_capture_flags() {
    let image = compile_script(
        "let f=function self(){'use strict';return function(){return self;};};f()();",
        "self-capture.js",
    )
    .unwrap();
    let qjs_bytecode::image::Constant::Function(function) = &image.function().constants[0] else {
        panic!()
    };
    let qjs_bytecode::image::Constant::Function(child) = &function.constants[0] else {
        panic!()
    };
    let capture = child
        .captures
        .iter()
        .find(|v| v.name == "self".encode_utf16().collect::<Vec<_>>())
        .unwrap();
    assert_eq!(capture.var_kind, 4);
    assert!(capture.is_const);
}

#[test]
fn duplicate_sloppy_parameter_metadata_exposes_only_last_binding() {
    let image = compile_script(
        "function f(x,x){return arguments[1]+x;}f(1,21);",
        "duplicates.js",
    )
    .unwrap();
    let qjs_bytecode::image::Constant::Function(function) = &image.function().constants[0] else {
        panic!()
    };
    assert!(function.locals[0].name.is_empty());
    assert_eq!(function.locals[1].name, vec![b'x' as u16]);
    assert_ne!(
        function.locals[0].var_ref_idx,
        function.locals[1].var_ref_idx
    );
}

/// Fix: a flat script of 998 statements (`o.pN=1;` each) failed to load with
/// "capacity: verification state budget/empty code": every statement took a new
/// local, so the verifier's instructions × (locals + 1) grew with the square of
/// the program's length. Long top-level code (any bundle) must compile, and the
/// number of locals must not grow with the number of statements.
#[test]
fn long_flat_scripts_compile_without_a_local_per_statement() {
    for statement in ["o.p{i}=1;", "o.p{i}+=1;", "o.p{i}||=1;", "o.p{i}++;"] {
        let mut locals = Vec::new();
        for n in [10usize, 100, 5000] {
            let source: String = std::iter::once("var o={};".to_string())
                .chain((0..n).map(|i| statement.replace("{i}", &i.to_string())))
                .collect();
            let image = compile_script(&source, "long.js")
                .unwrap_or_else(|e| panic!("{n} x {statement}: {e}"));
            locals.push(image.function().local_count);
            eprintln!("{n} x {statement}: {} locals", image.function().local_count);
        }
        assert_eq!(locals[0], locals[2], "{statement}: locals per statement count: {locals:?}");
    }
}
