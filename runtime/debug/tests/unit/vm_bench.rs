//! Paired owner orchestration benchmark, not JavaScript/native-engine throughput.
//! Both paths use the same real Rust fixture, typed command, capture, root leases,
//! invocation, and checkpoint. Only the public facade's preflight/state work differs.
use super::*;
use contract_fixture::{Inline, RustFixture};
use std::{hint::black_box, time::Instant};

type Runner = EventLoop<EngineBridge<RustFixture<Inline>>>;
type VmSetup = (
    VirtualMachine<RustFixture<Inline>>,
    RootedValue,
    Vec<RootedValue>,
    Rc<Cell<usize>>,
);
type RunnerSetup = (
    Runner,
    Capture,
    RootedValue,
    Vec<RootedValue>,
    Rc<Cell<usize>>,
);
struct Identity(Rc<Cell<usize>>);
impl HostFunction for Identity {
    fn invoke<'s>(&self, call: &'s dyn HostCall) -> JsResult<'s, ScopedValue<'s>> {
        self.0.set(self.0.get() + 1);
        Ok(call.arguments()[0])
    }
}
fn config() -> VmConfig {
    VmConfig {
        requirements: EngineRequirements {
            entry: EntryRequirement::ExistingCallables,
            debug: crate::debug::DebugRequirements::disabled(),
            ..Default::default()
        },
        ..Default::default()
    }
}
fn captured(runner: &mut Runner, capture: &Capture, kind: Kind) -> Captured {
    *capture.borrow_mut() = Captured::Empty;
    let id = runner
        .host()
        .post(EngineJob::Command(EngineCommand::new(
            kind,
            capture.clone(),
        )))
        .unwrap();
    assert_eq!(runner.run_until(id).unwrap(), TurnOutcome::Completed);
    std::mem::replace(&mut *capture.borrow_mut(), Captured::Empty)
}
fn stage(
    runner: &Runner,
    capture: &Capture,
    function: &RootedValue,
    arguments: &[RootedValue],
) -> TaskId {
    *capture.borrow_mut() = Captured::Empty;
    let kind = Kind::Call {
        function: function.local_lease(),
        receiver: Some(arguments[0].local_lease()),
        arguments: arguments.iter().map(RootedValue::local_lease).collect(),
    };
    runner
        .host()
        .post(EngineJob::Command(EngineCommand::new(
            kind,
            capture.clone(),
        )))
        .unwrap()
}
fn finish(runner: &mut Runner, capture: &Capture, task: TaskId) -> RootedValue {
    assert_eq!(runner.run_until(task).unwrap(), TurnOutcome::Completed);
    match std::mem::replace(&mut *capture.borrow_mut(), Captured::Empty) {
        Captured::Value(root) => root,
        _ => panic!("direct command must retain its actual return value"),
    }
}
fn direct_call(
    runner: &mut Runner,
    capture: &Capture,
    function: &RootedValue,
    arguments: &[RootedValue],
) -> RootedValue {
    let task = stage(runner, capture, function, arguments);
    finish(runner, capture, task)
}
fn vm_setup() -> VmSetup {
    let calls = Rc::new(Cell::new(0));
    let mut vm = VirtualMachine::new(RustFixture::new("vm-paired-benchmark"), config()).unwrap();
    let function = vm
        .bind_function(
            JsString::from_utf8("identity"),
            1,
            Rc::new(Identity(calls.clone())),
        )
        .unwrap();
    let result = Rc::new(RefCell::new(None));
    let output = result.clone();
    vm.execute(Box::new(move |scope: &dyn JsScope| {
        let value = scope
            .import_data(JsData::Number(42.0))
            .map_err(|error| scope.detach(error))?;
        *output.borrow_mut() = Some(scope.retain(value)?);
        Ok(())
    }))
    .unwrap();
    let argument = result.borrow_mut().take().unwrap();
    (vm, function, vec![argument], calls)
}
fn runner_setup() -> RunnerSetup {
    let cfg = config();
    let calls = Rc::new(Cell::new(0));
    let bridge = EngineBridge::new(
        RustFixture::new("vm-paired-benchmark"),
        cfg.context,
        cfg.requirements,
        None,
        cfg.scheduler.max_operations,
    )
    .unwrap()
    .with_debug_limits(cfg.debug_limits);
    let mut runner =
        EventLoop::with_policy(bridge, cfg.scheduler, None, cfg.script_error_policy).unwrap();
    let capture = Rc::new(RefCell::new(Captured::Empty));
    let function = match captured(
        &mut runner,
        &capture,
        Kind::Bind {
            name: JsString::from_utf8("identity"),
            arity: 1,
            function: Rc::new(Identity(calls.clone())),
        },
    ) {
        Captured::Value(root) => root,
        _ => panic!("bind returns callable root"),
    };
    let output = Rc::new(RefCell::new(None));
    let result = output.clone();
    assert!(matches!(
        captured(
            &mut runner,
            &capture,
            Kind::Work(Box::new(move |scope: &dyn JsScope| {
                let value = scope
                    .import_data(JsData::Number(42.0))
                    .map_err(|error| scope.detach(error))?;
                *result.borrow_mut() = Some(scope.retain(value)?);
                Ok(())
            }))
        ),
        Captured::Unit
    ));
    let argument = output.borrow_mut().take().unwrap();
    (runner, capture, function, vec![argument], calls)
}
fn timed(mut call: impl FnMut() -> RootedValue, iterations: usize) -> f64 {
    let start = Instant::now();
    for _ in 0..iterations {
        drop(black_box(call()));
    }
    start.elapsed().as_secs_f64() * 1e9 / iterations as f64
}
fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

#[test]
#[ignore = "explicit paired VM versus identical direct command measurement"]
fn paired_vm_vs_direct_command() {
    const WARM: usize = 1000;
    const ITERATIONS: usize = 20_000;
    const SAMPLES: usize = 7;
    const ALLOCATION_CALLS: usize = 100;
    let (mut vm, vm_function, vm_arguments, vm_calls) = vm_setup();
    let (mut runner, capture, function, arguments, direct_calls) = runner_setup();
    // An existing receiver avoids adding a fresh Undefined native wrapper to
    // either fixture's heap on every call. The returned value aliases the argument.
    for _ in 0..WARM {
        drop(
            vm.call(&vm_function, Some(&vm_arguments[0]), &vm_arguments)
                .unwrap(),
        );
        drop(direct_call(&mut runner, &capture, &function, &arguments));
    }
    let vm_wakes = vm.runner.handle().stats().wake_calls;
    let direct_wakes = runner.handle().stats().wake_calls;
    let (_, vm_allocations) = test_allocator::count_allocations(|| {
        for _ in 0..ALLOCATION_CALLS {
            drop(
                vm.call(&vm_function, Some(&vm_arguments[0]), &vm_arguments)
                    .unwrap(),
            );
        }
    });
    let (_, direct_allocations) = test_allocator::count_allocations(|| {
        for _ in 0..ALLOCATION_CALLS {
            drop(direct_call(&mut runner, &capture, &function, &arguments));
        }
    });
    let (task, staging_allocations) =
        test_allocator::count_allocations(|| stage(&runner, &capture, &function, &arguments));
    let (root, pumping_allocations) =
        test_allocator::count_allocations(|| finish(&mut runner, &capture, task));
    drop(root);
    assert_eq!(
        vm_allocations, direct_allocations,
        "the warmed facade introduces no additional allocation requests"
    );
    assert_eq!(
        direct_allocations,
        (staging_allocations + pumping_allocations) * ALLOCATION_CALLS
    );
    let before_vm = vm_calls.get();
    let before_direct = direct_calls.get();
    let mut vm_samples = Vec::with_capacity(SAMPLES);
    let mut direct_samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        if sample % 2 == 0 {
            vm_samples.push(timed(
                || {
                    vm.call(&vm_function, Some(&vm_arguments[0]), &vm_arguments)
                        .unwrap()
                },
                ITERATIONS,
            ));
            direct_samples.push(timed(
                || direct_call(&mut runner, &capture, &function, &arguments),
                ITERATIONS,
            ));
        } else {
            direct_samples.push(timed(
                || direct_call(&mut runner, &capture, &function, &arguments),
                ITERATIONS,
            ));
            vm_samples.push(timed(
                || {
                    vm.call(&vm_function, Some(&vm_arguments[0]), &vm_arguments)
                        .unwrap()
                },
                ITERATIONS,
            ));
        }
    }
    assert_eq!(vm_calls.get() - before_vm, SAMPLES * ITERATIONS);
    assert_eq!(direct_calls.get() - before_direct, SAMPLES * ITERATIONS);
    assert_eq!(vm.runner.handle().stats().wake_calls, vm_wakes);
    assert_eq!(runner.handle().stats().wake_calls, direct_wakes);
    let vm_ns = median(vm_samples);
    let direct_ns = median(direct_samples);
    eprintln!(
        "{{\"profile\":\"Disabled Rust fixture, owner orchestration only\",\"samples\":{SAMPLES},\"iterations_per_sample\":{ITERATIONS},\"vm_ns_per_call\":{vm_ns},\"direct_command_ns_per_call\":{direct_ns},\"vm_over_direct_ratio\":{},\"vm_allocations_per_call\":{},\"direct_allocations_per_call\":{},\"allocation_categories\":{{\"command_capture_and_leases\":{staging_allocations},\"driver_invocation_checkpoint_and_result_root\":{pumping_allocations},\"facade_additional\":0}},\"additional_wakes\":0}}",
        vm_ns / direct_ns,
        vm_allocations / ALLOCATION_CALLS,
        direct_allocations / ALLOCATION_CALLS
    );
    vm.shutdown().unwrap();
    runner.shutdown().unwrap();
}
