// Copyright (c) 2026 Coldmind AB <https://coldmind.com>
// SPDX-License-Identifier: MIT

//! The promise job queue belongs to jetbunny: QuickJS hands every job it creates to the
//! adapter, which holds, bounds, runs and releases it.
#![cfg(feature = "quickjs-ng")]

use coldmind_runtime::{
    engine::*,
    engines::quickjs_ng::{QuickJsNgConfig, QuickJsNgEngine},
};
use std::{cell::Cell, num::NonZeroUsize, rc::Rc};
mod support;

/// An engine whose context has the host's `queueMicrotask`, as the global builtin installs it.
fn engine(max_pending_jobs: usize) -> (QuickJsNgEngine, ContextId) {
    let engine = QuickJsNgEngine::with_config(QuickJsNgConfig {
        max_pending_jobs,
        ..Default::default()
    })
    .unwrap();
    let id = engine.create_context(ContextLimits::default()).unwrap();
    install(&engine, id, "queueMicrotask", Rc::new(QueueMicrotask));
    (engine, id)
}

fn install(engine: &QuickJsNgEngine, id: ContextId, name: &str, function: Rc<dyn HostFunction>) {
    engine
        .with_scope(id, &support::NoLoop, &mut |scope: &dyn JsScope| {
            let detach = |e| scope.detach(e);
            let key = JsString::from_utf8(name);
            let value = scope
                .host_function(&key, 1, function.clone())
                .map_err(detach)?;
            let global = scope.global().map_err(detach)?;
            scope
                .set(global, PropertyKey::String(&key), value)
                .map_err(detach)
        })
        .unwrap();
}

/// `queueMicrotask(callback)` through [`JsScope::enqueue_microtask`].
struct QueueMicrotask;
impl HostFunction for QueueMicrotask {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        call.enqueue_microtask(call.arguments()[0])?;
        call.import_data(JsData::Undefined)
    }
}

/// Evaluates `program` as a script and copies its completion value out.
fn evaluate(
    engine: &QuickJsNgEngine,
    id: ContextId,
    program: &str,
) -> Result<JsData, FailureReport> {
    let source = ScriptSource::new(JsString::from_utf8(program), "file:///jobs.js".into());
    let mut out = None;
    engine.with_scope(id, &support::NoLoop, &mut |scope: &dyn JsScope| {
        let value = scope
            .source()
            .unwrap()
            .evaluate(&source)
            .map_err(|e| scope.detach(e))?;
        out = Some(
            scope
                .export_data(value, scope.data_limits())
                .map_err(|e| scope.detach(e))?,
        );
        Ok(())
    })?;
    Ok(out.expect("the script ran"))
}

/// Runs at most `budget` queued jobs.
fn drain(engine: &QuickJsNgEngine, id: ContextId, budget: usize) -> CheckpointReport {
    let mut report = None;
    engine
        .with_scope(id, &support::NoLoop, &mut |scope: &dyn JsScope| {
            report = Some(scope.drain_microtasks(NonZeroUsize::new(budget).unwrap())?);
            Ok(())
        })
        .unwrap();
    report.unwrap()
}

fn numbers(data: JsData) -> Vec<f64> {
    let JsData::Array(ref items) = data else {
        panic!("not an array: {data:?}")
    };
    items
        .iter()
        .map(|item| match item {
            JsData::Number(n) => *n,
            other => panic!("not a number: {other:?}"),
        })
        .collect()
}

#[test]
fn a_promise_storm_past_the_job_capacity_fails_with_a_capacity_fault() {
    let (engine, id) = engine(64);
    evaluate(
        &engine,
        id,
        "for (let i = 0; i < 64; i++) Promise.resolve().then(() => {}); 0",
    )
    .unwrap();
    let Err(report) = evaluate(&engine, id, "Promise.resolve().then(() => {}); 0") else {
        panic!("a 65th job was accepted past a capacity of 64");
    };
    assert_eq!(
        (report.class, report.code),
        (FailureClass::Engine, FailureCode::Capacity),
        "{report:?}"
    );
    let message = report.message.as_ref().map(|m| m.to_utf8().unwrap());
    assert_eq!(message.as_deref(), Some("promise job queue full (64 jobs)"));
    // A lost job breaks the program's continuations: the context runs no more JavaScript.
    let after = evaluate(&engine, id, "1").unwrap_err();
    assert_eq!(after.class, FailureClass::Engine, "{after:?}");
    engine.close_context(id).unwrap();
}

#[test]
fn jobs_run_in_fifo_order_and_a_budget_leaves_the_rest_pending() {
    let (engine, id) = engine(64);
    evaluate(
        &engine,
        id,
        "globalThis.order = [];
         Promise.resolve(1).then(v => order.push(v));
         queueMicrotask(() => order.push(2));
         Promise.resolve(3).then(v => { order.push(v); queueMicrotask(() => order.push(6)); });
         Promise.resolve(4).then(v => order.push(v));
         queueMicrotask(() => order.push(5));
         0",
    )
    .unwrap();
    let first = drain(&engine, id, 2);
    assert_eq!(first.state, CheckpointState::Pending);
    assert_eq!(numbers(evaluate(&engine, id, "order").unwrap()), [1., 2.]);
    let rest = drain(&engine, id, 64);
    assert_eq!(rest.state, CheckpointState::Empty);
    assert!(rest.failures.is_empty(), "{:?}", rest.failures);
    assert_eq!(
        numbers(evaluate(&engine, id, "order").unwrap()),
        [1., 2., 3., 4., 5., 6.]
    );
    engine.close_context(id).unwrap();
}

/// Counts its own drop, which happens when the engine releases the function object.
struct Probe(Rc<Cell<u32>>);
impl HostFunction for Probe {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        call.import_data(JsData::Undefined)
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn closing_a_context_releases_its_queued_jobs() {
    let (engine, id) = engine(64);
    let dropped = Rc::new(Cell::new(0));
    install(&engine, id, "probe", Rc::new(Probe(dropped.clone())));
    evaluate(
        &engine,
        id,
        "Promise.resolve().then(probe); queueMicrotask(probe); delete globalThis.probe; 0",
    )
    .unwrap();
    engine.collect_garbage(id).unwrap();
    assert_eq!(dropped.get(), 0, "queued jobs keep the function alive");
    engine.close_context(id).unwrap();
    assert_eq!(
        dropped.get(),
        1,
        "closing the context released the queued jobs"
    );
}
