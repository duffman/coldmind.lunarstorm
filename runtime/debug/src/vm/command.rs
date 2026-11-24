//! Single owner-local command and capture transaction shared by all VM methods.
use crate::engine::*;
use std::{cell::RefCell, rc::Rc};
#[allow(
    clippy::large_enum_variant,
    reason = "One owner-local capture is allocated at VM construction and reused; boxing artifacts would add an allocation per compilation."
)]
pub(crate) enum Captured {
    Empty,
    Value(RootedValue),
    Artifact(CompiledArtifact),
    Unit,
}
pub(crate) type Capture = Rc<RefCell<Captured>>;
pub(crate) enum Kind {
    Bind {
        name: JsString,
        arity: u16,
        function: Rc<dyn HostFunction>,
    },
    Evaluate(ScriptSource),
    Call {
        function: RootedValue,
        receiver: Option<RootedValue>,
        arguments: Vec<RootedValue>,
    },
    Compile(ScriptSource, CompileOptions),
    Artifact(CompiledArtifact),
    Work(Box<dyn ScopeCall>),
}
/// Opaque one-shot VM command. The VM creates its private capture transaction;
/// applications use the public facade instead of constructing result channels.
pub struct EngineCommand {
    kind: Option<Kind>,
    capture: Capture,
}
impl EngineCommand {
    pub(crate) fn new(kind: Kind, capture: Capture) -> Self {
        Self {
            kind: Some(kind),
            capture,
        }
    }
    pub(crate) fn run(&mut self, scope: &dyn JsScope) -> Result<(), FailureReport> {
        let unsupported = || {
            FailureReport::fault(
                FailureCode::Unsupported,
                "requested engine interface unavailable",
            )
        };
        let output = match self.kind.take().ok_or_else(|| {
            FailureReport::fault(FailureCode::InvalidHandle, "command already entered")
        })? {
            Kind::Bind {
                name,
                arity,
                function,
            } => {
                let value = scope
                    .host_function(&name, arity, function)
                    .map_err(|e| scope.detach(e))?;
                let global = scope.global().map_err(|e| scope.detach(e))?;
                scope
                    .set(global, PropertyKey::String(&name), value)
                    .map_err(|e| scope.detach(e))?;
                Captured::Value(scope.retain(value)?)
            }
            Kind::Evaluate(source) => {
                let value = scope
                    .source()
                    .ok_or_else(unsupported)?
                    .evaluate(&source)
                    .map_err(|e| scope.detach(e))?;
                Captured::Value(scope.retain(value)?)
            }
            Kind::Call {
                function,
                receiver,
                arguments,
            } => {
                let function = scope.local(&function).map_err(|e| scope.detach(e))?;
                let receiver = receiver
                    .as_ref()
                    .map(|root| scope.local(root))
                    .transpose()
                    .map_err(|e| scope.detach(e))?;
                let arguments = arguments
                    .iter()
                    .map(|root| scope.local(root))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| scope.detach(e))?;
                let value = scope
                    .call(function, receiver, &arguments)
                    .map_err(|e| scope.detach(e))?;
                Captured::Value(scope.retain(value)?)
            }
            Kind::Compile(source, options) => Captured::Artifact(
                scope
                    .compiler()
                    .ok_or_else(unsupported)?
                    .compile(&source, options)?,
            ),
            Kind::Artifact(artifact) => {
                let value = scope
                    .artifacts()
                    .ok_or_else(unsupported)?
                    .execute(&artifact)
                    .map_err(|e| scope.detach(e))?;
                Captured::Value(scope.retain(value)?)
            }
            Kind::Work(mut work) => {
                work.run(scope)?;
                Captured::Unit
            }
        };
        *self.capture.borrow_mut() = output;
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/vm_command.rs"]
mod tests;
