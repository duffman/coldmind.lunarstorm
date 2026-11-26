//! Typed debugger integration. Native hooks and IDE transports implement these
//! interfaces separately; protocol support never manufactures engine capabilities.
//!
//! Public messages define native backend/coordinator/frontend obligations. The
//! coordinator, source registry, and DAP translator implement host bookkeeping
//! and protocol semantics. A conforming native adapter must still supply actual
//! interruption, breakpoints, stepping, inspection, and frame evaluation.

#[path = "debug_session/session_api.rs"]
mod api;
#[path = "debug_session/rejection_identities.rs"]
mod rejections;
pub use rejections::*;
#[path = "debug_session/async_stack_ancestry.rs"]
mod async_stacks;
pub use async_stacks::*;
#[path = "debug_session/object_identities.rs"]
mod identities;
#[path = "debug_session/inspection_contracts.rs"]
mod inspection;
#[path = "debug_session/variable_path_parsing.rs"]
mod lookup;
pub use lookup::*;
#[path = "debug_session/expression_evaluation_policy.rs"]
mod evaluation;
pub use api::*;
pub use evaluation::*;
pub use identities::*;
pub use inspection::*;

#[path = "debug_session/source_map_decoding.rs"]
mod source_map;
#[path = "debug_session/loaded_sources.rs"]
mod sources;
pub use sources::*;

#[path = "debug_session/pause_coordinator.rs"]
mod coordinator;
#[path = "debug_session/dap_message_transport.rs"]
mod transport;
pub use coordinator::*;
pub use transport::{DebugControlReceipt, DebugRequestReceipt, DebugSessionOptions, VmDebugHandle};

#[path = "debug_session/dap_protocol_translation.rs"]
mod dap;
pub use dap::*;

#[path = "debug_session/breakpoint_hit_conditions.rs"]
mod hit_condition;
#[path = "debug_session/logpoint_templates.rs"]
pub(crate) mod logpoint;
#[path = "debug_session/step_skip_globs.rs"]
mod stepping;
pub use hit_condition::HitCondition;

#[path = "debug_session/worker_target_registry.rs"]
mod targets;
pub use targets::*;
#[path = "debug_session/stdio_dap_transport.rs"]
pub mod host;
