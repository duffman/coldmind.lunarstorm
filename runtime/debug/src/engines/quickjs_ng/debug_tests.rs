//! Native hook regression through the production Rust compiler (not the C parser).
use super::*;
use std::{ffi::CString, rc::Weak};

struct Probe {
    hits: usize,
    markers: usize,
    failed: bool,
}
unsafe extern "C" fn marker(opaque: *mut c_void, _: usize, _: u32, _: i32, _: i32, _: i32) {
    // SAFETY: test retains the probe for this synchronous enumeration.
    unsafe { &mut *opaque.cast::<Probe>() }.markers += 1;
}
unsafe extern "C" fn hook(opaque: *mut c_void, context: *mut ffi::Context, event: i32) -> i32 {
    if event == -1 {
        return 0;
    }
    // SAFETY: owner-thread callback borrows the test's live probe/context.
    let probe = unsafe { &mut *opaque.cast::<Probe>() };
    probe.hits += 1;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let text: Vec<u16> = "x = 99".encode_utf16().collect();
        let origin = CString::new("debugger-eval.js").unwrap();
        // SAFETY: live selected frame; compiler callback consumes the lease synchronously.
        let mut value = std::mem::MaybeUninit::uninit();
        // SAFETY: out remains writable and the native call initializes it.
        let status = unsafe {
            ffi::jbq_debug_eval_verified(
                context,
                0,
                text.as_ptr(),
                text.len(),
                origin.as_ptr(),
                value.as_mut_ptr(),
            )
        };
        // SAFETY: native initializes out and transfers its evaluator-owned value;
        // the restored application exception slot must not be consumed here.
        unsafe { ffi::jbq_free(context, value.assume_init()) };
        let exception = status != 0;
        !exception
    }));
    probe.failed |= !matches!(result, Ok(true));
    0
}

#[test]
fn production_rust_compiler_debug_hook_mutates_shadowed_live_binding() {
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    let state = engine.context(id).unwrap();
    let ptr = state.raw.ptr().unwrap();
    let mut probe = Probe {
        hits: 0,
        markers: 0,
        failed: false,
    };
    let opaque = (&mut probe as *mut Probe).cast();
    let text: Vec<u16> = "function f(){let x=10;{let x=20;debugger;globalThis.inner=x;}globalThis.outer=x;}f();inner+outer".encode_utf16().collect();
    let origin = CString::new("production-debug.js").unwrap();
    // SAFETY: all calls run on the live context's owner thread; probe outlives the hook.
    unsafe {
        ffi::jbq_debug_install(ptr, Some(hook), opaque);
        ffi::jbq_debug_flags(ptr, 1);
        let code = ffi::jbq_compile(ptr, text.as_ptr(), text.len(), origin.as_ptr());
        assert_eq!(ffi::jbq_is_exception(code), 0);
        assert_eq!(ffi::jbq_debug_markers(ptr, code, marker, opaque), 0);
        let result = ffi::jbq_eval_compiled(ptr, code);
        assert_eq!(ffi::jbq_is_exception(result), 0);
        let mut number = 0.;
        assert_eq!(ffi::jbq_to_number(ptr, result, &mut number), 0);
        assert_eq!(number, 109.);
        ffi::jbq_free(ptr, result);
        ffi::jbq_free(ptr, code);
        ffi::jbq_debug_install(ptr, None, std::ptr::null_mut());
    }
    assert_eq!(probe.hits, 1);
    assert!(probe.markers > 0);
    assert!(!probe.failed);
    engine.close_context(id).unwrap();
}

struct ForceCollection(Weak<QuickJsNgEngine>, Rc<Cell<usize>>);

struct StopOnDrop(
    Arc<
        dyn Fn() -> Result<crate::debug::DebugControlReceipt, crate::debug::DebugError>
            + Send
            + Sync,
    >,
);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = (self.0)();
    }
}
struct AbortEvaluationGate(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);
impl HostFunction for AbortEvaluationGate {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.send(()).unwrap();
        self.1
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        call.import_data(JsData::Undefined)
    }
}

#[test]
fn raw_native_abort_exits_paused_evaluator_without_transport_stop_directive() {
    use crate::debug::*;
    use std::{
        num::NonZeroU64,
        sync::mpsc,
        time::{Duration, Instant},
    };
    for detach in [false, true] {
        let engine = QuickJsNgEngine::new().unwrap();
        let id = engine.create_context(ContextLimits::default()).unwrap();
        let interrupt = engine.interrupt_handle(id).unwrap();
        let coordinator = DebugCoordinator::new(
            engine.debug_backend(id).unwrap().unwrap(),
            id,
            DebugLimits::default(),
            Arc::new(|| Ok(())),
            NonZeroU64::new(1).unwrap(),
        )
        .unwrap();
        let (tx, entered) = mpsc::channel();
        let (release, rx) = mpsc::channel();
        let gate = Rc::new(AbortEvaluationGate(tx, rx));
        engine
            .with_scope(
                id,
                &crate::vm::contract_fixture::NoLoop,
                &mut |scope: &dyn JsScope| {
                    let function = scope
                        .host_function(&JsString::from_utf8("gate"), 0, gate.clone())
                        .map_err(|e| scope.detach(e))?;
                    let global = scope.global().map_err(|e| scope.detach(e))?;
                    scope
                        .set(
                            global,
                            PropertyKey::String(&JsString::from_utf8("gate")),
                            function,
                        )
                        .map_err(|e| scope.detach(e))
                },
            )
            .unwrap();
        let debug = coordinator.attach(DebugSessionOptions::default()).unwrap();
        let stop = coordinator.stop_signal();
        let worker = std::thread::spawn(move || {
            let _rescue = StopOnDrop(stop.clone());
            let next = |debug: &VmDebugHandle| {
                let until = Instant::now() + Duration::from_secs(10);
                loop {
                    if let Some(update) = debug.try_recv() {
                        break update;
                    }
                    assert!(Instant::now() < until);
                    std::thread::yield_now();
                }
            };
            let pause = loop {
                if let DebugUpdate::Paused { pause, .. } = next(&debug) {
                    break pause;
                }
            };
            let send = |request| match debug.try_request(request).unwrap() {
                DebugRequestReceipt::Accepted { id }
                | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
            };
            let stack = send(DebugRequest::Stack {
                pause,
                page: Page { start: 0, count: 1 },
            });
            let frame = loop {
                if let DebugUpdate::Reply {
                    id,
                    result: Ok(DebugReply::Stack { frames, .. }),
                } = next(&debug)
                    && id == stack
                {
                    break frames[0].id;
                }
            };
            let evaluation = send(DebugRequest::EvaluateWithOptions {
                frame,
                expression: JsString::from_utf8("gate();while(true){}"),
                options: EvaluationOptions::default(),
            });
            entered.recv_timeout(Duration::from_secs(10)).unwrap();
            interrupt.request_abort();
            debug.cancel_evaluation(evaluation).unwrap();
            if detach {
                debug.detach().unwrap();
            }
            release.send(()).unwrap();
            let until = Instant::now() + Duration::from_secs(2);
            let mut rescued = false;
            let mut terminal = 0;
            let mut closed = false;
            while !closed || terminal == 0 {
                if let Some(update) = debug.try_recv() {
                    match update {
                        DebugUpdate::Reply { id, result } if id == evaluation => {
                            terminal += 1;
                            assert_eq!(result.unwrap_err(), DebugError::Closed);
                        }
                        DebugUpdate::SessionClosed { .. } => closed = true,
                        _ => {}
                    }
                } else if Instant::now() >= until && !rescued {
                    // Rescue a defective paused pump so this regression fails
                    // without leaving the owner blocked indefinitely.
                    rescued = true;
                    stop().unwrap();
                } else {
                    std::thread::yield_now();
                }
            }
            assert_eq!(terminal, 1);
            assert!(
                !rescued,
                "raw abort needed a transport stop to exit its paused pump"
            );
        });
        let result = engine.with_scope(
            id,
            &crate::vm::contract_fixture::NoLoop,
            &mut |scope: &dyn JsScope| {
                scope
                    .source()
                    .unwrap()
                    .evaluate(&ScriptSource::new(
                        JsString::from_utf8("debugger;throw 'application resumed'"),
                        "file:///raw-abort.js".into(),
                    ))
                    .map_err(|e| scope.detach(e))?;
                Ok(())
            },
        );
        worker.join().unwrap();
        assert_eq!(result.unwrap_err().code, FailureCode::Interrupted);
        coordinator.shutdown().unwrap();
        engine.close_context(id).unwrap();
    }
}

#[test]
fn raw_native_abort_exits_condition_evaluator_without_transport_stop_directive() {
    use crate::debug::*;
    use std::{
        num::NonZeroU64,
        sync::mpsc,
        time::{Duration, Instant},
    };
    let engine = QuickJsNgEngine::new().unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    let interrupt = engine.interrupt_handle(id).unwrap();
    let coordinator = DebugCoordinator::new(
        engine.debug_backend(id).unwrap().unwrap(),
        id,
        DebugLimits::default(),
        Arc::new(|| Ok(())),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap();
    let (tx, entered) = mpsc::channel();
    let (release, rx) = mpsc::channel();
    let gate = Rc::new(AbortEvaluationGate(tx, rx));
    engine
        .with_scope(
            id,
            &crate::vm::contract_fixture::NoLoop,
            &mut |scope: &dyn JsScope| {
                let function = scope
                    .host_function(&JsString::from_utf8("gate"), 0, gate.clone())
                    .map_err(|e| scope.detach(e))?;
                let global = scope.global().map_err(|e| scope.detach(e))?;
                scope
                    .set(
                        global,
                        PropertyKey::String(&JsString::from_utf8("gate")),
                        function,
                    )
                    .map_err(|e| scope.detach(e))
            },
        )
        .unwrap();
    let debug = coordinator.attach(DebugSessionOptions::default()).unwrap();
    debug
        .try_request(DebugRequest::SetBreakpoints {
            selector: ScriptSelector {
                origin: "file:///condition-abort.js".into(),
                expected_digest: None,
            },
            breakpoints: vec![BreakpointSpec {
                client_id: 1,
                line: 1,
                column: None,
                condition: Some(JsString::from_utf8("gate();while(true){}")),
                enabled: true,
                hit_condition: None,
                log_message: None,
            }],
        })
        .unwrap();
    let stop = coordinator.stop_signal();
    let worker = std::thread::spawn(move || {
        let _rescue = StopOnDrop(stop.clone());
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        interrupt.request_abort();
        release.send(()).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        let mut rescued = false;
        loop {
            match debug.try_recv() {
                Some(DebugUpdate::SessionClosed { .. }) => break,
                Some(DebugUpdate::Paused { .. }) => panic!("raw abort must not pause"),
                Some(_) => {}
                None if Instant::now() >= until && !rescued => {
                    rescued = true;
                    stop().unwrap();
                }
                None => std::thread::yield_now(),
            }
        }
        assert!(
            !rescued,
            "condition abort needed a transport stop directive"
        );
    });
    let result = engine.with_scope(
        id,
        &crate::vm::contract_fixture::NoLoop,
        &mut |scope: &dyn JsScope| {
            scope
                .source()
                .unwrap()
                .evaluate(&ScriptSource::new(
                    JsString::from_utf8("globalThis.ran=0;\nran++;"),
                    "file:///condition-abort.js".into(),
                ))
                .map_err(|e| scope.detach(e))?;
            Ok(())
        },
    );
    worker.join().unwrap();
    assert_eq!(result.unwrap_err().code, FailureCode::Interrupted);
    assert!(engine.context(id).unwrap().raw.take_fault().is_none());
    coordinator.shutdown().unwrap();
    engine.close_context(id).unwrap();
}
impl HostFunction for ForceCollection {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.upgrade().unwrap().collect_garbage(call.context())?;
        self.1.set(self.1.get() + 1);
        call.import_data(JsData::Undefined)
    }
}

#[test]
fn retained_debugger_objects_and_lexical_scopes_survive_native_collection() {
    use crate::debug::*;
    use std::{
        num::NonZeroU64,
        time::{Duration, Instant},
    };
    fn update(debug: &VmDebugHandle) -> DebugUpdate {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(update) = debug.try_recv() {
                return update;
            }
            assert!(Instant::now() < deadline, "debugger update timed out");
            std::thread::yield_now();
        }
    }
    fn ask(debug: &VmDebugHandle, request: DebugRequest) -> DebugReply {
        let id = match debug.try_request(request).unwrap() {
            DebugRequestReceipt::Accepted { id }
            | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
        };
        loop {
            if let DebugUpdate::Reply { id: actual, result } = update(debug)
                && actual == id
            {
                return result.unwrap();
            }
        }
    }
    fn variables(debug: &VmDebugHandle, parent: VariableParent) -> Vec<DebugVariable> {
        let mut cursor = None;
        let mut variables = Vec::new();
        loop {
            let DebugReply::Inspection(page) = ask(
                debug,
                DebugRequest::Inspect(InspectionRequest {
                    parent,
                    page: Page {
                        start: 0,
                        count: 64,
                    },
                    cursor,
                }),
            ) else {
                panic!("expected native inspection page")
            };
            assert!(page.scanned_slots <= 4096);
            variables.extend(page.items.into_iter().map(|v| v.variable));
            cursor = page.next;
            if page.complete {
                return variables;
            }
        }
    }
    fn named<'a>(variables: &'a [DebugVariable], name: &str) -> &'a DebugValue {
        &variables
            .iter()
            .find(|v| v.name.to_utf8().unwrap() == name)
            .unwrap_or_else(|| panic!("missing {name}"))
            .value
    }
    let engine = Rc::new(QuickJsNgEngine::new().unwrap());
    let id = engine.create_context(ContextLimits::default()).unwrap();
    let coordinator = DebugCoordinator::new(
        engine.debug_backend(id).unwrap().unwrap(),
        id,
        DebugLimits::default(),
        Arc::new(|| Ok(())),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap();
    let collections = Rc::new(Cell::new(0));
    let callback = Rc::new(ForceCollection(Rc::downgrade(&engine), collections.clone()));
    engine
        .with_scope(
            id,
            &crate::vm::contract_fixture::NoLoop,
            &mut |scope: &dyn JsScope| {
                let f = scope
                    .host_function(&JsString::from_utf8("forceGC"), 0, callback.clone())
                    .map_err(|e| scope.detach(e))?;
                let global = scope.global().map_err(|e| scope.detach(e))?;
                scope
                    .set(
                        global,
                        PropertyKey::String(&JsString::from_utf8("forceGC")),
                        f,
                    )
                    .map_err(|e| scope.detach(e))
            },
        )
        .unwrap();
    let debug = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let worker = std::thread::spawn(move || {
        let pause = loop {
            if let DebugUpdate::Paused { pause, .. } = update(&debug) {
                break pause;
            }
        };
        let frame = match ask(
            &debug,
            DebugRequest::Stack {
                pause,
                page: Page { start: 0, count: 1 },
            },
        ) {
            DebugReply::Stack { frames, .. } => frames[0].id,
            other => panic!("{other:?}"),
        };
        let scopes = match ask(&debug, DebugRequest::Scopes { frame }) {
            DebugReply::Scopes(scopes) => scopes,
            other => panic!("{other:?}"),
        };
        let mut all = Vec::new();
        for scope in scopes {
            all.extend(variables(&debug, VariableParent::Scope(scope.id)));
        }
        assert_eq!(named(&all, "argument").preview.to_utf8().unwrap(), "11");
        assert_eq!(named(&all, "captured").preview.to_utf8().unwrap(), "23");
        let object = named(&all, "object").object.unwrap();
        let members = variables(&debug, VariableParent::Object(object));
        let nested = named(&members, "nested").object.unwrap();
        let DebugReply::Inspection(before_gc) = ask(
            &debug,
            DebugRequest::Inspect(InspectionRequest {
                parent: VariableParent::Object(nested),
                page: Page {
                    start: 0,
                    count: 64,
                },
                cursor: None,
            }),
        ) else {
            panic!("expected retained child page")
        };
        let old_cursor = before_gc
            .next
            .expect("retained child must have another page");

        match ask(
            &debug,
            DebugRequest::Evaluate {
                frame,
                expression: JsString::from_utf8("object=null;forceGC();42"),
                allow_side_effects: true,
            },
        ) {
            DebugReply::Evaluation { value, exception } => {
                assert!(!exception);
                assert_eq!(value.preview.to_utf8().unwrap(), "42");
            }
            other => panic!("{other:?}"),
        }
        let id = match debug
            .try_request(DebugRequest::Inspect(InspectionRequest {
                parent: VariableParent::Object(nested),
                page: Page {
                    start: 0,
                    count: 64,
                },
                cursor: Some(old_cursor),
            }))
            .unwrap()
        {
            DebugRequestReceipt::Accepted { id }
            | DebugRequestReceipt::AcceptedWakeFailed { id, .. } => id,
        };
        loop {
            if let DebugUpdate::Reply { id: actual, result } = update(&debug)
                && actual == id
            {
                assert_eq!(result.unwrap_err(), DebugError::StaleInspection);
                break;
            }
        }
        let after = variables(&debug, VariableParent::Object(nested));
        assert_eq!(
            after.len(),
            131,
            "fresh retained child pages survive collection"
        );
        assert_eq!(named(&after, "answer").preview.to_utf8().unwrap(), "37");
        // Re-read frame scopes as well as references retained before collection.
        let mut after_locals = Vec::new();
        let DebugReply::Scopes(scopes) = ask(&debug, DebugRequest::Scopes { frame }) else {
            panic!()
        };
        for scope in scopes {
            after_locals.extend(variables(&debug, VariableParent::Scope(scope.id)));
        }
        assert_eq!(
            named(&after_locals, "argument").preview.to_utf8().unwrap(),
            "11"
        );
        assert_eq!(
            named(&after_locals, "captured").preview.to_utf8().unwrap(),
            "23"
        );
        assert_eq!(
            named(&after_locals, "object").preview.to_utf8().unwrap(),
            "null"
        );
        debug
            .continue_execution(pause, DebugContinue::Resume)
            .unwrap();
        debug
    });
    engine.with_scope(id, &crate::vm::contract_fixture::NoLoop, &mut |scope: &dyn JsScope| {
        scope.source().unwrap().evaluate(&ScriptSource::new(
            JsString::from_utf8("(function outer(){let captured=23;return function inner(argument){let object={nested:{answer:37}};for(let i=0;i<130;i++)object.nested['p'+i]=i;debugger;return captured+argument;};})()(11)"),
            "file:///gc-debugger.js".into(),
        )).map_err(|e| scope.detach(e))?;
        Ok(())
    }).unwrap();
    let _debug = worker.join().unwrap();
    assert_eq!(collections.get(), 1, "the real native collector must run");
    coordinator.shutdown().unwrap();
    engine.close_context(id).unwrap();
}

#[test]
fn lookup_rejection_and_stale_frame_never_enter_native_or_compile() {
    use crate::debug::*;
    use std::{
        num::NonZeroU64,
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    let engine = QuickJsNgEngine::new().unwrap();
    let context = engine.create_context(ContextLimits::default()).unwrap();
    let observer = engine
        .context(context)
        .unwrap()
        .debug
        .lookup_observation
        .clone();
    let coordinator = DebugCoordinator::new(
        engine.debug_backend(context).unwrap().unwrap(),
        context,
        DebugLimits::default(),
        Arc::new(|| Ok(())),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap();
    let debug = coordinator.attach(DebugSessionOptions::default()).unwrap();
    let stop = coordinator.stop_signal();
    struct StopOnPanic<F: Fn()>(F);
    impl<F: Fn()> Drop for StopOnPanic<F> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                (self.0)();
            }
        }
    }
    let worker = std::thread::spawn(move || {
        let _rescue = StopOnPanic(|| {
            let _ = stop();
        });
        let update = || {
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(update) = debug.try_recv() {
                    return update;
                }
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
        };
        let paused = || loop {
            if let DebugUpdate::Paused { pause, .. } = update() {
                break pause;
            }
        };
        let request = |request| {
            let id = match debug.try_request(request) {
                Ok(
                    DebugRequestReceipt::Accepted { id }
                    | DebugRequestReceipt::AcceptedWakeFailed { id, .. },
                ) => id,
                Err(rejected) => {
                    return match rejected.reason {
                        RejectionReason::Debug(error) => Err(error),
                        other => panic!("{other:?}"),
                    };
                }
            };
            loop {
                if let DebugUpdate::Reply { id: actual, result } = update()
                    && actual == id
                {
                    break result;
                }
            }
        };
        let frame = |pause| match request(DebugRequest::Stack {
            pause,
            page: Page { start: 0, count: 1 },
        })
        .unwrap()
        {
            DebugReply::Stack { frames, .. } => frames[0].id,
            other => panic!("{other:?}"),
        };
        let snapshot = || {
            (
                observer.native_entries.load(Ordering::SeqCst),
                observer.compilations.load(Ordering::SeqCst),
            )
        };
        let first = paused();
        let old = frame(first);
        let before = snapshot();
        assert!(
            before.1 > 0,
            "production compiler observer must see application compilation"
        );
        for text in [
            "touch()",
            "object.answer=9",
            "object[touch()]",
            "object+1",
            "object?.answer",
        ] {
            assert_eq!(
                request(DebugRequest::LookupPath {
                    frame: old,
                    path: JsString::from_utf8(text)
                })
                .unwrap_err(),
                DebugError::InvalidRequest
            );
            assert_eq!(
                snapshot(),
                before,
                "rejected syntax reached FFI/compiler: {text}"
            );
        }
        let DebugReply::Lookup(value) = request(DebugRequest::LookupPath {
            frame: old,
            path: JsString::from_utf8("object.answer"),
        })
        .unwrap() else {
            panic!()
        };
        assert_eq!(value.preview.to_utf8().unwrap(), "42");
        assert_eq!(
            snapshot(),
            (before.0 + 1, before.1),
            "valid lookup reaches native and never compiler"
        );
        // Positive compiler control: explicit evaluation must increment observer.
        let evaluated = request(DebugRequest::Evaluate {
            frame: old,
            expression: JsString::from_utf8("40+2"),
            allow_side_effects: true,
        })
        .unwrap();
        assert!(matches!(
            evaluated,
            DebugReply::Evaluation {
                exception: false,
                ..
            }
        ));
        assert!(snapshot().1 > before.1);
        debug
            .continue_execution(first, DebugContinue::Resume)
            .unwrap();
        let second = paused();
        let fresh = frame(second);
        let before = snapshot();
        assert_eq!(
            request(DebugRequest::LookupPath {
                frame: old,
                path: JsString::from_utf8("object.answer")
            })
            .unwrap_err(),
            DebugError::StalePause
        );
        assert_eq!(snapshot(), before, "stale frame reached native/compiler");
        assert!(matches!(
            request(DebugRequest::LookupPath {
                frame: fresh,
                path: JsString::from_utf8("object.answer")
            }),
            Ok(DebugReply::Lookup(_))
        ));
        assert_eq!(snapshot(), (before.0 + 1, before.1));
        debug
            .continue_execution(second, DebugContinue::Resume)
            .unwrap();
    });
    engine
        .with_scope(
            context,
            &crate::vm::contract_fixture::NoLoop,
            &mut |scope: &dyn JsScope| {
                scope.source().unwrap().evaluate(&ScriptSource::new(JsString::from_utf8(
            "let object={answer:42};function touch(){throw 99}debugger;object.answer=43;debugger;"),
            "file:///lookup-observation.js".into())).map_err(|error|scope.detach(error))?;
                Ok(())
            },
        )
        .unwrap();
    worker.join().unwrap();
    coordinator.shutdown().unwrap();
    engine.close_context(context).unwrap();
}
