use super::*;
use crate::{
    engine::JsEngine,
    vm::contract_fixture::{Inline, NoLoop, RustFixture},
};
use std::cell::Cell;

struct Counter(Rc<Cell<usize>>);
impl ScopeCall for Counter {
    fn run(&mut self, scope: &dyn JsScope) -> Result<(), FailureReport> {
        self.0.set(self.0.get() + 1);
        scope
            .import_data(JsData::Number(42.0))
            .map_err(|error| scope.detach(error))?;
        Ok(())
    }
}
struct Function(Rc<Cell<usize>>);
impl HostFunction for Function {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.set(self.0.get() + 1);
        call.import_data(JsData::Undefined)
    }
}
fn root(
    engine: &RustFixture<Inline>,
    context: ContextId,
    function: Option<Rc<dyn HostFunction>>,
) -> RootedValue {
    let mut result = None;
    engine
        .with_scope(context, &NoLoop, &mut |scope: &dyn JsScope| {
            let value = match &function {
                Some(function) => {
                    scope.host_function(&JsString::from_utf8("typed-command"), 0, function.clone())
                }
                None => scope.import_data(JsData::Null),
            }
            .map_err(|error| scope.detach(error))?;
            result = Some(scope.retain(value)?);
            Ok(())
        })
        .unwrap();
    result.unwrap()
}
fn artifact(source: ScriptSource) -> CompiledArtifact {
    let metadata = ArtifactMetadata {
        adapter_id: "unsupported-command".into(),
        native_version: "none".into(),
        format_version: 1,
        target_fingerprint: "rust-test".into(),
        build_fingerprint: "rust-test".into(),
        options: CompileOptions {
            mode: CodeMode::Script,
            debug_info: DebugInfo::Required,
            fingerprint: "rust-program-v1".into(),
        },
        source_digest: SourceDigest::of_utf16(&source.text),
    };
    // SAFETY: Trusted empty metadata fixture; the absent ArtifactApi rejects
    // the command before any native loader or deserializer could be reached.
    unsafe { CompiledArtifact::from_trusted_compiler_output(metadata, Vec::new(), source) }.unwrap()
}

#[test]
fn a_consumed_typed_command_never_reenters_its_real_body() {
    let engine = RustFixture::<Inline>::new("consumed-command");
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let calls = Rc::new(Cell::new(0));
    let capture = Rc::new(RefCell::new(Captured::Empty));
    let mut command = EngineCommand::new(
        Kind::Work(Box::new(Counter(calls.clone()))),
        capture.clone(),
    );
    engine
        .with_scope(context, &NoLoop, &mut |scope: &dyn JsScope| {
            command.run(scope)?;
            assert!(matches!(*capture.borrow(), Captured::Unit));
            assert_eq!(
                command.run(scope).unwrap_err().code,
                FailureCode::InvalidHandle
            );
            assert_eq!(calls.get(), 1);
            Ok(())
        })
        .unwrap();
    engine.close_context(context).unwrap();
    assert_eq!(
        format!("{:?}", EngineJob::Command(command)),
        "Command(owner capture)",
        "consumed commands expose no scoped/captured value in diagnostics"
    );
}

#[test]
fn typed_optional_commands_reject_missing_interfaces_without_fabricating_values() {
    let engine = RustFixture::<Inline>::new("unsupported-command");
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let source = ScriptSource::new(
        JsString::from_utf8("named-rust-program"),
        "rust://unsupported".into(),
    );
    let options = CompileOptions {
        mode: CodeMode::Script,
        debug_info: DebugInfo::Required,
        fingerprint: "rust-program-v1".into(),
    };
    for kind in [
        Kind::Evaluate(source.clone()),
        Kind::Compile(source.clone(), options),
        Kind::Artifact(artifact(source)),
    ] {
        let capture = Rc::new(RefCell::new(Captured::Empty));
        let mut command = EngineCommand::new(kind, capture.clone());
        engine
            .with_scope(context, &NoLoop, &mut |scope: &dyn JsScope| {
                assert_eq!(
                    command.run(scope).unwrap_err().code,
                    FailureCode::Unsupported
                );
                assert!(matches!(*capture.borrow(), Captured::Empty));
                Ok(())
            })
            .unwrap();
    }
    engine.close_context(context).unwrap();
}

#[test]
fn typed_call_still_validates_function_receiver_and_argument_contexts() {
    let engine = RustFixture::<Inline>::new("command-owner");
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let foreign = RustFixture::<Inline>::new("foreign-command-root");
    let foreign_context = foreign.create_context(ContextLimits::default()).unwrap();
    let calls = Rc::new(Cell::new(0));
    let function = root(&engine, context, Some(Rc::new(Function(calls.clone()))));
    let foreign_root = root(&foreign, foreign_context, None);
    for invalid in 0..3 {
        let kind = Kind::Call {
            function: if invalid == 0 {
                foreign_root.local_lease()
            } else {
                function.local_lease()
            },
            receiver: (invalid == 1).then(|| foreign_root.local_lease()),
            arguments: if invalid == 2 {
                vec![foreign_root.local_lease()]
            } else {
                Vec::new()
            },
        };
        let mut command = EngineCommand::new(kind, Rc::new(RefCell::new(Captured::Empty)));
        engine
            .with_scope(context, &NoLoop, &mut |scope: &dyn JsScope| {
                assert_eq!(
                    command.run(scope).unwrap_err().code,
                    FailureCode::WrongContext
                );
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(calls.get(), 0);
    let capture = Rc::new(RefCell::new(Captured::Empty));
    let mut valid = EngineCommand::new(
        Kind::Call {
            function: function.local_lease(),
            receiver: None,
            arguments: Vec::new(),
        },
        capture.clone(),
    );
    engine
        .with_scope(context, &NoLoop, &mut |scope: &dyn JsScope| {
            valid.run(scope)
        })
        .unwrap();
    assert_eq!(calls.get(), 1);
    assert!(matches!(*capture.borrow(), Captured::Value(_)));
    engine.close_context(context).unwrap();
    foreign.close_context(foreign_context).unwrap();
}
