//! The Lunarstorm Runtime.
//!
//! ```text
//! HOST            owns the Runtime (calls down)      implements host services (calls up)
//!                 start, load, stop                  stdio, args, exit,
//!                 call(export, args) -> promise      fs/net policy,
//!                 post(task) from any thread         clock, entropy, environment,
//!                 poll() one loop step               log sink
//!                 register(bindings)
//! RUNTIME         event loop, timers, module loader, fs and net under host policy,
//!                 plus the bindings the host registered; generic over the VM
//! VM              `lunarstorm-engine-contract`: the contract an engine adapter implements
//! ```
//!
//! The rule at both boundaries: this crate depends only on contracts. It names no
//! host type and no engine type; [`Runtime<V, H>`] is generic over a
//! [`lunarstorm_engine_contract::Vm`] and a [`Host`], both bound at compile time, so every call
//! across either boundary is a direct (and usually inlined) call.
//!
//! | module                          | role                                                         |
//! |---------------------------------|--------------------------------------------------------------|
//! | `runtime_instance`              | the [`Runtime`] a host owns: start, load, call, poll, stop    |
//! | `runtime_public_types`          | values crossing the host boundary: `Source`, `Arg`, `Poster` |
//! | `event_loop`                    | one loop step: timers, readiness, TCP, HTTP, posted tasks    |
//! | `host_services_trait`           | the host services contract ([`Host`])                        |
//! | `host_bindings_registry`        | host-supplied native functions and modules ([`Bindings`])    |
//! | `extension_manifest_check`      | extension manifests and their requirement check              |
//! | `internal_natives`              | the Runtime's own natives (`lunarstorm:internal` ops)        |
//! | `node_builtins`                 | Node's builtin modules and globals (`node:path`, `console`)  |
//! | `module_loader_callbacks`       | module resolver and loader; the VM callbacks                 |
//! | `reactor_io_state`              | loop-owned state: sockets, write buffers, timers, deferred   |
//! | `blocking_work_pool`            | bounded worker pool for blocking work (fs, DNS)              |
//! | `blocking_filesystem_operations`| filesystem operations, run inline or on the pool             |
//! | `cross_thread_message_channel`  | bounded byte queue between threads: `Inbox`, the host's side |
//! | `ipc_module`                    | `lunarstorm:ipc`, the program's byte channel to its host     |
//! | `worker_threads_module`         | `lunarstorm:worker`, programs on their own threads and VMs   |
//!
//! HTTP/1.1 request framing and response-head serialization is the separate crate
//! `lunarstorm-http1` (`crates/http1`): it has no sockets and no JS, so the rest of
//! the monorepo can use it without the Runtime.

mod host_bindings_registry;
mod cross_thread_message_channel;
mod extension_manifest_check;
mod blocking_filesystem_operations;
mod host_services_trait;
mod reactor_io_state;
mod ipc_module;
mod module_loader_callbacks;
mod node_builtins;
mod internal_natives;
mod blocking_work_pool;
mod event_loop;
mod runtime_instance;
mod runtime_public_types;
mod worker_threads_module;

pub use host_bindings_registry::{declared, AccessorImpl, Bindings, ClassImpl, Constructor, Exports, Native};
pub use cross_thread_message_channel::{Inbox, MAX_BYTES, MAX_MESSAGES, MAX_MESSAGE_BYTES};
pub use extension_manifest_check::{check, valid_name, Cardinality, Entry, Extension, ExtensionSet, Manifest, Offer, PluginHost, Requirement, Use, RESERVED};
pub use host_services_trait::{Access, Host, Log, Stream};
pub use lunarstorm_engine_contract as vm;
pub use node_builtins::{declarations as builtin_declarations, GLOBAL};
pub use runtime_instance::{glue_compiles, Runtime};
pub use runtime_public_types::{Arg, CallId, Poster, Source, State, Wait};
