//! Explicit owner-local engine construction without adapter dependencies.
use crate::engine::{FailureCode, FailureReport, JsEngine};
use std::{collections::BTreeMap, rc::Rc};

/// Native resource ceilings an embedder sets on an engine. `None` keeps the adapter default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineLimits {
    /// Native heap ceiling; an allocation past it fails with out-of-memory.
    pub heap_bytes: Option<usize>,
    /// Native stack budget for the engine's stack-overflow checks.
    pub stack_bytes: Option<usize>,
    /// Queued promise jobs; one past it fails the program with a capacity fault.
    pub pending_jobs: Option<usize>,
}

/// Constructs a fresh engine on the calling owner thread.
pub trait EngineFactory {
    /// Stable adapter identity, equal to every constructed engine's descriptor ID.
    fn id(&self) -> &str;
    /// Constructs an independent engine, propagating native admission failures.
    fn create(&self) -> Result<Box<dyn JsEngine>, FailureReport>;
    /// Constructs an engine that honors every set limit. An adapter that cannot honor one
    /// fails with `Unsupported`; it never ignores it.
    fn create_with(&self, limits: &EngineLimits) -> Result<Box<dyn JsEngine>, FailureReport> {
        if *limits != EngineLimits::default() {
            return Err(FailureReport::fault(
                FailureCode::Unsupported,
                &format!("engine {} cannot honor engine limits", self.id()),
            ));
        }
        self.create()
    }
}

/// Owner-local registry. Registration never constructs an engine or replaces a factory.
#[derive(Default)]
pub struct EngineRegistry {
    factories: BTreeMap<String, Rc<dyn EngineFactory>>,
}
impl EngineRegistry {
    /// Creates an empty registry with no implicit engine selection.
    pub fn new() -> Self {
        Self::default()
    }
    /// Registers a nonempty identity. Duplicates fail without replacing prior state.
    pub fn register(&mut self, factory: Rc<dyn EngineFactory>) -> Result<(), FailureReport> {
        let id = factory.id();
        if id.is_empty() || self.factories.contains_key(id) {
            return Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "empty or duplicate engine identity",
            ));
        }
        self.factories.insert(id.to_owned(), factory);
        Ok(())
    }
    /// Lists registered identities in deterministic lexical order without construction.
    pub fn identifiers(&self) -> impl ExactSizeIterator<Item = &str> {
        self.factories.keys().map(String::as_str)
    }
    /// Constructs the selected engine. Omission is allowed only for a single factory.
    /// Unknown, ambiguous, and descriptor-mismatched selections never fall back.
    pub fn create_selected(
        &self,
        selection: Option<&str>,
    ) -> Result<Box<dyn JsEngine>, FailureReport> {
        self.create_limited(selection, &EngineLimits::default())
    }
    /// [`Self::create_selected`] with native limits the engine must honor.
    pub fn create_limited(
        &self,
        selection: Option<&str>,
        limits: &EngineLimits,
    ) -> Result<Box<dyn JsEngine>, FailureReport> {
        let factory = match selection {
            Some(id) => self.factories.get(id),
            None if self.factories.len() == 1 => self.factories.values().next(),
            None => None,
        }
        .ok_or_else(|| {
            FailureReport::fault(
                FailureCode::Unsupported,
                "explicit registered engine selection required",
            )
        })?;
        let engine = factory.create_with(limits)?;
        if engine.descriptor().adapter_id != factory.id() {
            return Err(FailureReport::fault(
                FailureCode::InvalidHandle,
                "engine factory descriptor identity mismatch",
            ));
        }
        Ok(engine)
    }
}
