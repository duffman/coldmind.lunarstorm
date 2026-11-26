//! A stop requested from another thread ends JavaScript that never yields. A stop that the
//! running code requests on its own thread lets that code return normally.
#![cfg(feature = "quickjs-ng")]

use coldmind_runtime::{
    debug::DebugRequirements, engine::*, engines::quickjs_ng::QuickJsNgEngine, vm::*,
};
use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

fn vm() -> VirtualMachine<QuickJsNgEngine> {
    let config = VmConfig {
        requirements: EngineRequirements {
            debug: DebugRequirements::disabled(),
            ..Default::default()
        },
        ..Default::default()
    };
    VirtualMachine::new(QuickJsNgEngine::new().unwrap(), config).unwrap()
}
fn source(text: &str) -> ScriptSource {
    ScriptSource::new(JsString::from_utf8(text), "file:///busy-stop.js".into())
}

#[test]
fn stop_from_another_thread_aborts_a_loop_that_never_yields() {
    let mut vm = vm();
    let handle = vm.handle();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        handle.request_stop()
    });
    let start = Instant::now();
    let result = vm.evaluate(source("for (;;) {}"));
    assert!(result.is_err(), "busy loop must not complete");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "abort took {:?}",
        start.elapsed()
    );
    stopper.join().unwrap().unwrap();
}

#[test]
fn abort_is_not_catchable_by_the_aborted_code() {
    let mut vm = vm();
    let handle = vm.handle();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        handle.request_stop()
    });
    let result = vm.evaluate(source(
        "for (;;) { try { for (;;) {} } catch (e) { /* must not swallow the abort */ } }",
    ));
    assert!(result.is_err());
    stopper.join().unwrap().unwrap();
}

struct Stop;
impl HostFunction for Stop {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        call.host().request_stop()?;
        call.import_data(JsData::Undefined)
    }
}
struct Mark(Rc<Cell<f64>>);
impl HostFunction for Mark {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        if let JsData::Number(n) = call.export_data(call.arguments()[0], ExportLimits::default())? {
            self.0.set(n);
        }
        call.import_data(JsData::Undefined)
    }
}

#[test]
fn stop_on_the_owner_thread_does_not_abort_the_calling_code() {
    let mut vm = vm();
    let reached = Rc::new(Cell::new(0.0));
    let stop = vm
        .bind_function(JsString::from_utf8("stop"), 0, Rc::new(Stop))
        .unwrap();
    let mark = vm
        .bind_function(
            JsString::from_utf8("mark"),
            1,
            Rc::new(Mark(reached.clone())),
        )
        .unwrap();
    let body = vm
        .evaluate(source(
            "(stop, mark) => { stop(); let x = 0; for (let i = 0; i < 200000; i++) x += 1; mark(x); }",
        ))
        .unwrap();
    let stop_local = vm.call(&body, None, &[stop, mark]);
    // The VM reports that it is stopping, but the code after stop() ran to its end.
    assert!(
        matches!(stop_local, Ok(_) | Err(VmError::Stopped)),
        "{stop_local:?}"
    );
    assert_eq!(reached.get(), 200000.0);
}
