//! Phase 6 uses real Rust contract call graphs, never simulated JavaScript parsing.
mod support;
use coldmind_runtime::{debug::DebugRequirements, engine::*, scheduler::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use support::{Inline, RustFixture};
fn requirements() -> EngineRequirements {
    EngineRequirements {
        entry: EntryRequirement::ExistingCallables,
        debug: DebugRequirements::disabled(),
        ..Default::default()
    }
}
fn bridge(handler: Option<Rc<dyn HostMessageHandler>>) -> EngineBridge<RustFixture<Inline>> {
    EngineBridge::new(
        RustFixture::new("bridge-fixture"),
        ContextLimits::default(),
        requirements(),
        handler,
        8,
    )
    .unwrap()
}
struct Observe(Rc<RefCell<Vec<JsData>>>);
impl HostFunction for Observe {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        assert_eq!(call.kind(call.receiver())?, ValueKind::Undefined);
        self.0.borrow_mut().extend(
            call.arguments()
                .iter()
                .map(|&arg| call.export_data(arg, ExportLimits::default()).unwrap()),
        );
        call.import_data(JsData::Undefined)
    }
}
fn callback(
    runtime: &mut EventLoop<EngineBridge<RustFixture<Inline>>>,
    observed: Rc<RefCell<Vec<JsData>>>,
) -> RootedValue {
    let output = Rc::new(RefCell::new(None));
    let result = output.clone();
    let task = runtime
        .host()
        .post(EngineJob::Work(Box::new(move |scope: &dyn JsScope| {
            let function = scope
                .host_function(
                    &JsString::from_utf8("observe"),
                    2,
                    Rc::new(Observe(observed.clone())),
                )
                .map_err(|e| scope.detach(e))?;
            *result.borrow_mut() = Some(scope.retain(function)?);
            Ok(())
        })))
        .unwrap();
    runtime.run_until(task).unwrap();
    output.borrow_mut().take().unwrap()
}
#[test]
fn ordinary_callback_jobs_localize_existing_roots_without_data_export() {
    let observed = Rc::new(RefCell::new(Vec::new()));
    let mut runtime = EventLoop::new(bridge(None), SchedulerConfig::default(), None).unwrap();
    let function = callback(&mut runtime, observed.clone());
    runtime
        .host()
        .post(EngineJob::Callback(CallbackJob {
            function,
            receiver: None,
            arguments: vec![],
        }))
        .unwrap();
    runtime.tick(false).unwrap();
    assert!(observed.borrow().is_empty());
}
struct Start {
    callback: RefCell<Option<RootedValue>>,
    ticket: Rc<RefCell<Option<CompletionTicket>>>,
    cancel: Option<Rc<dyn CancelNative>>,
}
impl HostFunction for Start {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        let reservation = call.host().reserve_operation(4096)?;
        let ticket = call.host().bind_callback(
            reservation,
            self.callback.borrow_mut().take().unwrap(),
            self.cancel.clone(),
        )?;
        *self.ticket.borrow_mut() = Some(ticket);
        call.import_data(JsData::Undefined)
    }
}
fn begin(
    runtime: &mut EventLoop<EngineBridge<RustFixture<Inline>>>,
    callback: RootedValue,
    cancel: Option<Rc<dyn CancelNative>>,
) -> CompletionTicket {
    let ticket = Rc::new(RefCell::new(None));
    let captured = ticket.clone();
    let mut callback = Some(callback);
    let task = runtime
        .host()
        .post(EngineJob::Work(Box::new(move |scope: &dyn JsScope| {
            let function = scope
                .host_function(
                    &JsString::from_utf8("begin"),
                    0,
                    Rc::new(Start {
                        callback: RefCell::new(callback.take()),
                        ticket: captured.clone(),
                        cancel: cancel.clone(),
                    }),
                )
                .map_err(|e| scope.detach(e))?;
            scope
                .call(function, None, &[])
                .map_err(|e| scope.detach(e))?;
            Ok(())
        })))
        .unwrap();
    runtime.run_until(task).unwrap();
    ticket.borrow_mut().take().unwrap()
}
struct Cancel(Rc<Cell<usize>>);
impl CancelNative for Cancel {
    fn cancel(&self) -> Result<(), OperationFailure> {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
}
#[test]
fn successful_reserved_callback_completion_uses_null_error_and_does_not_cancel_native() {
    let observed = Rc::new(RefCell::new(Vec::new()));
    let canceled = Rc::new(Cell::new(0));
    let mut runtime = EventLoop::new(bridge(None), SchedulerConfig::default(), None).unwrap();
    let cb = callback(&mut runtime, observed.clone());
    let ticket = begin(&mut runtime, cb, Some(Rc::new(Cancel(canceled.clone()))));
    assert!(matches!(
        ticket.complete(CompletionPayload::Success(JsData::Number(42.0))),
        PublishOutcome::Published
    ));
    runtime.tick(false).unwrap();
    assert_eq!(*observed.borrow(), [JsData::Null, JsData::Number(42.0)]);
    assert_eq!(canceled.get(), 0);
}
#[test]
fn canceled_and_abandoned_bindings_invoke_native_cancel_once() {
    for abandon in [false, true] {
        let observed = Rc::new(RefCell::new(Vec::new()));
        let canceled = Rc::new(Cell::new(0));
        let mut runtime = EventLoop::new(bridge(None), SchedulerConfig::default(), None).unwrap();
        let cb = callback(&mut runtime, observed.clone());
        let ticket = begin(&mut runtime, cb, Some(Rc::new(Cancel(canceled.clone()))));
        if !abandon {
            assert_eq!(
                runtime.host().cancel_operation(ticket.id()),
                CancelOutcome::Canceled
            );
        }
        drop(ticket);
        runtime.tick(false).unwrap();
        assert_eq!(canceled.get(), 1);
        assert!(
            matches!(&observed.borrow()[0],JsData::Record(fields) if fields.iter().any(|(k,v)|k.to_utf8().unwrap()=="code"&&matches!(v,JsData::String(_))))
        );
        assert_eq!(observed.borrow()[1], JsData::Undefined);
    }
}
struct Messages(Rc<RefCell<Vec<JsData>>>);
impl HostMessageHandler for Messages {
    fn deliver<'s>(
        &self,
        _: &'s dyn JsScope,
        _: &dyn HostServices,
        payload: JsData,
    ) -> JsResult<'s, ()> {
        self.0.borrow_mut().push(payload);
        Ok(())
    }
}
#[test]
fn fixed_message_handler_admits_only_bounded_data_and_unsupported_without_handler() {
    let mut absent = EventLoop::new(bridge(None), SchedulerConfig::default(), None).unwrap();
    assert_eq!(
        absent
            .handle()
            .try_send(JsData::Number(1.0))
            .unwrap_err()
            .reason,
        RejectionReason::Unsupported
    );
    absent.tick(false).unwrap();
    let observed = Rc::new(RefCell::new(Vec::new()));
    let mut runtime = EventLoop::new(
        bridge(Some(Rc::new(Messages(observed.clone())))),
        SchedulerConfig::default(),
        None,
    )
    .unwrap();
    runtime.handle().try_send(JsData::Number(7.0)).unwrap();
    runtime.tick(false).unwrap();
    assert_eq!(*observed.borrow(), [JsData::Number(7.0)]);
}
