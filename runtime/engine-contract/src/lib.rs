//! The VM interface: the contract between the Runtime and a JavaScript engine.
//!
//! ```text
//! RUNTIME        generic over the VM, bound at compile time
//!    | owns VM (calls down)            ^ implements VM callbacks (calls up)
//!    v                                 |
//! VM INTERFACE                       VM CALLBACKS
//!   Instance: heap, GC, jobs,          module resolve/load,
//!     limits, interrupt                unhandled rejection,
//!   Context: one realm                 interrupt check
//!   Handles: scoped + persistent       each receives a context handle,
//!   Native function signature          never the Runtime
//!   Host objects with finalizers
//!   Capabilities query
//!   Conformance suite
//!    | conforms to (static dispatch)
//!    v
//! VM ADAPTER    wraps one engine
//! ```
//!
//! Design decisions:
//! - **Static dispatch.** [`Vm`] is a marker type with associated types; the Runtime
//!   is generic over it and every call is monomorphized. There is no `dyn` anywhere
//!   in the contract, so an adapter whose methods inline costs nothing.
//! - **Two kinds of handle.** A scoped handle ([`Vm::Value`]) borrows the scope it
//!   was made in and cannot outlive it; a persistent handle ([`Vm::Persistent`]) is
//!   `'static` and is turned back into a scoped one with [`Scope::local`].
//! - **Exceptions stay in the engine.** A failing operation returns [`Thrown`], a
//!   zero-sized marker meaning "an exception is pending in the context"; the value
//!   is fetched with [`Scope::exception`] only when someone wants it. Nothing is
//!   allocated or converted on the error path until then.
//! - **Borrowed data.** [`Scope::with_str`] and [`Scope::with_bytes`] lend the
//!   engine's own memory to a closure instead of copying it out.
//! - **Callbacks take `&self`.** A callback may run while another is active (a
//!   rejection reported during a module load), so implementations use interior
//!   mutability rather than `&mut self`.
//! - **Isolation.** A [`Context`] is a realm. Limits, the interrupt and the job
//!   queue belong to the [`Instance`]; isolation and reload use a new Instance.

pub mod engine_conformance_suite;
pub mod native_declarations;
mod engine_descriptor_and_limits;
mod engine_traits;
mod module_source_and_image;
mod native_call_and_class_spec;
mod value_and_error_kinds;

pub use engine_descriptor_and_limits::*;
pub use engine_traits::*;
pub use module_source_and_image::*;
pub use native_call_and_class_spec::*;
pub use native_declarations::{ClassDecl, Field, FnDecl, Item, ModuleDecl, Param, Sig, Ty};
pub use value_and_error_kinds::*;

/// The debugger crate, whose protocol types [`Instance::debug`] hands out.
pub use lunarstorm_debugger;


/// Version of this contract. An adapter reports the version it implements in its
/// [`Descriptor`]; the conformance suite rejects a mismatch.
pub const CONTRACT_VERSION: u32 = 1;

/// Marker: the operation failed and an exception is pending in the context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thrown;

/// Result of an operation that can throw.
pub type Outcome<T> = Result<T, Thrown>;
