//! Typed debugger integration. Native hooks and IDE transports implement these
//! interfaces separately; protocol support never manufactures engine capabilities.
//!
//! Public messages define native backend/coordinator/frontend obligations. The
//! coordinator, source registry, and DAP translator implement host bookkeeping
//! and protocol semantics. A conforming native adapter must still supply actual
//! interruption, breakpoints, stepping, inspection, and frame evaluation.

mod api;
mod rejections;
pub use rejections::*;
mod async_stacks;
pub use async_stacks::*;
mod identities;
mod inspection;
mod lookup;
pub use lookup::*;
mod evaluation;
pub use api::*;
pub use evaluation::*;
pub use identities::*;
pub use inspection::*;

mod source_map;
mod sources;
pub use sources::*;

mod coordinator;
mod transport;
pub use coordinator::*;
pub use transport::{DebugControlReceipt, DebugRequestReceipt, DebugSessionOptions, VmDebugHandle};

mod dap;
pub use dap::*;

mod hit_condition;
pub(crate) mod logpoint;
mod stepping;
pub use hit_condition::HitCondition;

mod targets;
pub use targets::*;
pub mod host;
