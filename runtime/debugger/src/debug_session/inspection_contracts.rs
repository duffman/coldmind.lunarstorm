//! Typed inspection contracts; all native targets remain owner-local table keys.
use super::{BindingRef, CursorRef, DebugVariable, Page, PropertyRef, VariableParent};
use crate::engine::JsString;

/// Exact writable identity, never a display name or executable path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteTarget {
    /// A verified lexical binding.
    Binding(BindingRef),
    /// A verified writable own data property.
    Property(PropertyRef),
}
/// Bounded children request, optionally continuing a previous native scan.
#[derive(Clone, Copy, Debug)]
pub struct InspectionRequest {
    /// Current-pause scope or retained object.
    pub parent: VariableParent,
    /// Maximum requested items and initial offset.
    pub page: Page,
    /// Owner-issued continuation; invalidated by evaluation or editing.
    pub cursor: Option<CursorRef>,
}
/// Why an otherwise mutable member currently has no issued target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteUnavailable {
    /// Release abandoned write targets and refresh this view to retry issuance.
    Capacity,
}
/// Safe variable preview with optional exact write identity.
#[derive(Clone, Debug)]
pub struct InspectionEntry {
    /// Display name and non-executing value preview.
    pub variable: DebugVariable,
    /// Safely obtained native type name.
    pub type_name: JsString,
    /// Exact writable target, absent for read-only or unavailable members.
    pub write: Option<WriteTarget>,
    /// Temporary issuance failure; the readable value remains available. None
    /// with no write target denotes a read-only or unsupported member.
    pub write_unavailable: Option<WriteUnavailable>,
}
/// One bounded native scan; an empty page need not mean completion.
#[derive(Clone, Debug)]
pub struct InspectionPage {
    /// Entries emitted within the requested item and owned-byte budgets.
    pub items: Vec<InspectionEntry>,
    /// Continuation for unfinished scans.
    pub next: Option<CursorRef>,
    /// Whether the scan reached its end.
    pub complete: bool,
    /// Native storage slots examined for this page.
    pub scanned_slots: usize,
    /// Total children, only when known without unbounded work.
    pub total: Option<usize>,
}
