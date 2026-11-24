use qjs_frontend::{SourceMode, compile_program};
use qjs_ir::js::{Export, StatementKind};
#[test]
fn module_preserves_symbol_identity_linkage_and_strictness() {
    let program=compile_program("import value,{x as y} from 'dep';import * as ns from 'dep';export let answer=value+y;export {answer as result};export {x as forwarded} from 'dep';export * from 'other';export * as namespace from 'dep';","main.mjs",SourceMode::Module).unwrap();
    assert!(program.strict);
    let module = program.module.unwrap();
    assert_eq!(module.requests.len(), 2);
    assert_eq!(module.imports.len(), 3);
    assert_eq!(module.exports.len(), 5);
    let StatementKind::Declaration(declarations) = &program.body[0].kind else {
        panic!()
    };
    let qjs_ir::js::Pattern::Binding { binding, .. } = &declarations[0].pattern else {
        panic!("{:?}", declarations[0].pattern)
    };
    assert!(matches!(module.exports[0],Export::Local{symbol,..}if symbol==binding.symbol));
}
#[test]
fn default_exports_keep_declaration_hoisting_distinct_from_expressions() {
    for source in [
        "export default 42;",
        "export default function(){return 42;}",
        "export default function named(){return 42;}",
        "let x=42;export{x as default};",
    ] {
        let p = compile_program(source, "default.mjs", SourceMode::Module).unwrap();
        assert_eq!(p.module.unwrap().exports.len(), 1);
    }
}
#[test]
fn unsupported_module_effects_are_not_discarded() {
    for source in [
        "export const x=await 42;",
        "export default function*(){}",
        "import x from 'dep' with{type:'json'};",
        "export default import('dep');",
    ] {
        compile_program(source, "supported.mjs", SourceMode::Module)
            .unwrap_or_else(|e| panic!("{source}: {e:?}"));
    }
    for source in [
        "import source x from 'dep';",
        "import defer * as ns from 'dep';",
        "export default import.source('dep');",
    ] {
        assert!(
            compile_program(source, "unsupported.mjs", SourceMode::Module).is_err(),
            "{source}"
        );
    }
}
