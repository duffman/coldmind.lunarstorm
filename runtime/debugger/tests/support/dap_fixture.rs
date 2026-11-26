#![allow(dead_code)]
//! DAP framing/control tests use a Rust metadata backend, not a JavaScript debugger.
use coldmind_runtime::{debug::*, engine::*};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    num::NonZeroU64,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
pub(crate) struct Signal(pub(crate) AtomicBool);
impl DebugSignal for Signal {
    fn wake(&self) -> Result<(), DebugError> {
        if self.0.load(Ordering::Acquire) {
            Err(DebugError::WakeFailed(FailureReport::fault(
                FailureCode::EngineFailure,
                "fixture signal failed",
            )))
        } else {
            Ok(())
        }
    }
}
pub(crate) struct Backend {
    pub(crate) descriptor: DebugDescriptor,
    pub(crate) port: RefCell<Option<Rc<dyn DebugPort>>>,
    pub(crate) session: Cell<Option<DebugSessionId>>,
    pub(crate) continued: Cell<usize>,
    pub(crate) async_eval: Cell<bool>,
    pub(crate) evaluation: Cell<Option<DebugRequestId>>,
    pub(crate) scopes: Cell<usize>,
    pub(crate) signal: Arc<Signal>,
    pub(crate) actions: RefCell<Vec<DebugContinue>>,
    pub(crate) policies: RefCell<Vec<ExceptionPolicy>>,
    pub(crate) last_breakpoints: RefCell<Vec<BreakpointSpec>>,
    pub(crate) last_expression: RefCell<Option<JsString>>,
    pub(crate) variable_count: Cell<usize>,
    pub(crate) frame_count: Cell<usize>,
    pub(crate) evaluation_throws: Cell<bool>,
    pub(crate) frame_location: RefCell<Option<DebugLocation>>,
    pub(crate) breakpoint_mode: Cell<u8>,
}
impl Backend {
    pub(crate) fn new() -> Self {
        Self {
            descriptor: DebugDescriptor {
                api_version: 1,
                capabilities: DebugCapabilities::baseline(),
                native_requirements: vec!["Rust DAP fixture only".to_owned()],
                interrupt_mechanism: "fixture".to_owned(),
            },
            port: RefCell::new(None),
            session: Cell::new(None),
            continued: Cell::new(0),
            async_eval: Cell::new(false),
            evaluation: Cell::new(None),
            scopes: Cell::new(0),
            signal: Arc::new(Signal(AtomicBool::new(false))),
            actions: RefCell::new(vec![]),
            policies: RefCell::new(vec![]),
            last_breakpoints: RefCell::new(vec![]),
            last_expression: RefCell::new(None),
            variable_count: Cell::new(1),
            frame_count: Cell::new(1),
            evaluation_throws: Cell::new(false),
            frame_location: RefCell::new(None),
            breakpoint_mode: Cell::new(0),
        }
    }
    pub(crate) fn port(&self) -> Rc<dyn DebugPort> {
        self.port.borrow().as_ref().unwrap().clone()
    }
    pub(crate) fn pause(&self, reason: PauseReason) -> PauseId {
        let p = self.port();
        let pause = p
            .identities()
            .begin_pause(self.session.get().unwrap())
            .unwrap();
        p.publish(DebugUpdate::Paused {
            sequence: p.identities().next_update_sequence().unwrap(),
            pause,
            reason,
            location: self.frame_location.borrow().clone(),
            original: None,
            breakpoint_ids: vec![],
        })
        .unwrap();
        pause
    }
    pub(crate) fn finish(&self) {
        let id = self.evaluation.take().unwrap();
        self.port()
            .publish(DebugUpdate::Reply {
                id,
                result: Ok(DebugReply::Evaluation {
                    value: DebugValue {
                        preview: JsString::from_utf8("async fixture"),
                        object: None,
                        accessor: false,
                        unavailable: None,
                    },
                    exception: false,
                }),
            })
            .unwrap();
    }
}
impl DebugBackend for Backend {
    fn descriptor(&self) -> &DebugDescriptor {
        &self.descriptor
    }
    fn initialize(&self, port: Rc<dyn DebugPort>) -> Result<(), DebugError> {
        *self.port.borrow_mut() = Some(port);
        Ok(())
    }
    fn attach(&self, options: DebugAttachOptions) -> Result<Arc<dyn DebugSignal>, DebugError> {
        self.session.set(Some(options.session));
        Ok(self.signal.clone())
    }
    fn service(&self) -> Result<DebugServiceReport, DebugError> {
        let p = self.port();
        let mut progress = false;
        while let Some(directive) = p.next_directive(false) {
            progress = true;
            match directive {
                DebugDirective::Continue { pause, action } => {
                    self.actions.borrow_mut().push(action);
                    self.continued.set(self.continued.get() + 1);
                    p.identities().end_pause(pause)?;
                    p.publish(DebugUpdate::Resumed {
                        sequence: p.identities().next_update_sequence()?,
                        previous: pause,
                    })?;
                }
                DebugDirective::Request(command) => {
                    let result = match command.request {
                        DebugRequest::SetBreakpoints {
                            selector,
                            breakpoints,
                        } => {
                            *self.last_breakpoints.borrow_mut() = breakpoints.clone();
                            DebugReply::Breakpoints(
                                breakpoints
                                    .into_iter()
                                    .map(|breakpoint| {
                                        let state = match self.breakpoint_mode.get() {
                                            1 => BreakpointState::Verified(
                                                p.resolve_breakpoint(
                                                    &selector,
                                                    breakpoint.line,
                                                    breakpoint.column,
                                                )
                                                .unwrap(),
                                            ),
                                            2 => BreakpointState::Rejected(
                                                "fixture condition rejected".to_owned(),
                                            ),
                                            _ => BreakpointState::Pending,
                                        };
                                        BreakpointInfo {
                                            client_id: breakpoint.client_id,
                                            state,
                                        }
                                    })
                                    .collect(),
                            )
                        }
                        DebugRequest::Stack { pause, page } => {
                            let total = self.frame_count.get();
                            let end = page.start.saturating_add(page.count).min(total);
                            let mut frames = Vec::new();
                            for index in page.start..end {
                                let frame = match p.identities().register(
                                    pause,
                                    ReferenceKind::Frame,
                                    NonZeroU64::new(index as u64 + 100).unwrap(),
                                )? {
                                    DebugReference::Frame(frame) => frame,
                                    _ => unreachable!(),
                                };
                                frames.push(DebugFrame {
                                    id: frame,
                                    name: JsString::from_utf8("Rust fixture frame"),
                                    generated: self.frame_location.borrow().clone(),
                                    original: None,
                                });
                            }
                            DebugReply::Stack { frames, total }
                        }
                        DebugRequest::Scopes { frame } => {
                            self.scopes.set(self.scopes.get() + 1);
                            let scope = match p.identities().register(
                                DebugReference::Frame(frame).pause(),
                                ReferenceKind::Scope,
                                NonZeroU64::new(2).unwrap(),
                            )? {
                                DebugReference::Scope(scope) => scope,
                                _ => unreachable!(),
                            };
                            DebugReply::Scopes(vec![DebugScope {
                                id: scope,
                                name: JsString::from_utf8("Rust locals"),
                                expensive: false,
                            }])
                        }
                        DebugRequest::ReleaseCursor(cursor) => {
                            p.identities().release_cursor(cursor)?;
                            DebugReply::CursorReleased
                        }
                        DebugRequest::Variables { parent, page } => {
                            let pause = match parent {
                                VariableParent::Scope(scope) => {
                                    DebugReference::Scope(scope).pause()
                                }
                                VariableParent::Object(object) => {
                                    DebugReference::Object(object).pause()
                                }
                            };
                            let object = match p.identities().register(
                                pause,
                                ReferenceKind::Object,
                                NonZeroU64::new(3).unwrap(),
                            )? {
                                DebugReference::Object(object) => object,
                                _ => unreachable!(),
                            };
                            let total = self.variable_count.get();
                            let end = page.start.saturating_add(page.count).min(total);
                            let variables = (page.start..end)
                                .map(|index| DebugVariable {
                                    name: JsString::from_utf8(&format!("item{index}")),
                                    value: DebugValue {
                                        preview: JsString::from_utf8(&index.to_string()),
                                        object: (index == 0).then_some(object),
                                        accessor: index == 0,
                                        unavailable: None,
                                    },
                                })
                                .collect();
                            DebugReply::Variables { variables, total }
                        }
                        DebugRequest::Evaluate { expression, .. } => {
                            *self.last_expression.borrow_mut() = Some(expression);
                            if self.async_eval.get() {
                                self.evaluation.set(Some(command.id));
                                continue;
                            }
                            DebugReply::Evaluation {
                                value: DebugValue {
                                    preview: JsString::from_utf8(if self.evaluation_throws.get() {
                                        "fixture throw"
                                    } else {
                                        "fixture result"
                                    }),
                                    object: None,
                                    accessor: false,
                                    unavailable: None,
                                },
                                exception: self.evaluation_throws.get(),
                            }
                        }
                        DebugRequest::ExceptionPolicy(policy) => {
                            self.policies.borrow_mut().push(policy);
                            DebugReply::ExceptionPolicy(policy)
                        }
                        _ => {
                            p.publish(DebugUpdate::Reply {
                                id: command.id,
                                result: Err(DebugError::Unsupported),
                            })?;
                            continue;
                        }
                    };
                    p.publish(DebugUpdate::Reply {
                        id: command.id,
                        result: Ok(result),
                    })?;
                }
                DebugDirective::Detach | DebugDirective::Stop => self.detach()?,
                DebugDirective::Pause => {
                    self.pause(PauseReason::Requested);
                }
            }
        }
        Ok(DebugServiceReport {
            made_progress: progress,
            state: p.identities().state(),
        })
    }
    fn detach(&self) -> Result<(), DebugError> {
        if let Some(session) = self.session.take() {
            let p = self.port();
            if let DebugExecutionState::Paused(pause) = p.identities().state() {
                p.identities().end_pause(pause)?;
            }
            p.identities().close_session(session)?;
            p.publish(DebugUpdate::SessionClosed {
                session,
                reason: None,
            })?;
        }
        Ok(())
    }
}
pub(crate) fn fixture() -> (
    ValueStore<()>,
    Rc<Backend>,
    Rc<DebugCoordinator>,
    DapAdapter,
) {
    fixture_with_limits(DebugLimits::default())
}
pub(crate) fn fixture_with_limits(
    limits: DebugLimits,
) -> (
    ValueStore<()>,
    Rc<Backend>,
    Rc<DebugCoordinator>,
    DapAdapter,
) {
    fixture_with_capabilities(limits, DebugCapabilities::baseline())
}
pub(crate) fn fixture_with_capabilities(
    limits: DebugLimits,
    capabilities: DebugCapabilities,
) -> (
    ValueStore<()>,
    Rc<Backend>,
    Rc<DebugCoordinator>,
    DapAdapter,
) {
    let id = EngineIdentity::new().unwrap();
    let store = ValueStore::new(&id, ContextLimits::default()).unwrap();
    let mut backend = Backend::new();
    backend.descriptor.capabilities = capabilities;
    let backend = Rc::new(backend);
    let coordinator = DebugCoordinator::new(
        backend.clone(),
        store.context(),
        limits,
        Arc::new(|| Ok(())),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap();
    let handle = coordinator
        .attach(DebugSessionOptions {
            break_on_start: true,
        })
        .unwrap();
    let dap = DapAdapter::new(handle).unwrap();
    (store, backend, coordinator, dap)
}
pub(crate) fn wire(value: Value) -> Vec<u8> {
    let body = serde_json::to_vec(&value).unwrap();
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend(body);
    frame
}
pub(crate) fn decode(frame: &[u8]) -> Value {
    let end = frame.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
    let header = std::str::from_utf8(&frame[..end]).unwrap();
    let size = header
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert_eq!(size, frame.len() - end);
    serde_json::from_slice(&frame[end..]).unwrap()
}
pub(crate) fn request(dap: &mut DapAdapter, seq: u64, command: &str, args: Value) -> Vec<Value> {
    dap.receive(&wire(
        json!({"seq":seq,"type":"request","command":command,"arguments":args}),
    ))
    .unwrap()
    .iter()
    .map(|f| decode(f))
    .collect()
}
pub(crate) fn drive(coordinator: &DebugCoordinator, dap: &mut DapAdapter) -> Vec<Value> {
    coordinator.service().unwrap();
    dap.poll().unwrap().iter().map(|f| decode(f)).collect()
}
