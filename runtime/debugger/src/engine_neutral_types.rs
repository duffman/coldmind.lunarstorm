//! Engine-neutral types the debugger speaks in, from jetbunny `src/engine`.

#[path = "engine_neutral_types/engine_api.rs"]
mod api;
#[path = "engine_neutral_types/host_data.rs"]
mod data;
#[path = "engine_neutral_types/engine_errors.rs"]
mod errors;
#[path = "engine_neutral_types/engine_extensions.rs"]
mod extensions;
#[path = "engine_neutral_types/value_identity.rs"]
mod identity;
#[path = "engine_neutral_types/distribution_images.rs"]
mod images;
#[path = "engine_neutral_types/engine_scope.rs"]
mod scope;
#[path = "engine_neutral_types/engine_values.rs"]
mod values;
// lunarstorm patch: jetbunny's engine bridge (the VM-side call path) and its command
// type, compiled with jetbunny's VM into the unit tests only; see `crate::vm`.
#[cfg(test)]
#[path = "engine_neutral_types/owner_thread_bridge.rs"]
mod bridge;
#[cfg(test)]
pub use crate::vm::command::EngineCommand;
#[cfg(test)]
pub use bridge::*;

pub use api::*;
pub use data::*;
pub use errors::*;
pub use extensions::*;
pub use identity::*;
pub use images::*;
pub use scope::*;
pub use values::*;
