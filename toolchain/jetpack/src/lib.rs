//! # jetpack
//!
//! An npm registry package manager implemented in Rust. It resolves a pinned
//! nested `node_modules` tree, validates SHA package integrity, and installs
//! required npm peers at the nearest viable host scope.
//!
//! Peer dependencies have different placement rules from ordinary dependencies:
//! for `node_modules/host/node_modules/plugin`, the peer `react` belongs at
//! `node_modules/host/node_modules/react` (or at a compatible ancestor), **not**
//! inside `plugin/node_modules/react`. Peer conflicts fail closed.
//!
//! ## Library example
//!
//! jetpack owns no HTTP stack: the embedder passes a [`transport::Transport`].
//!
//! ```no_run
//! use jetpack::{manifest::Manifest, registry::Registry, resolve, transport::Transport};
//! use std::sync::Arc;
//! fn lock(transport: Arc<dyn Transport>) -> anyhow::Result<()> {
//!     let manifest = Manifest::open(std::path::Path::new("package.json"))?;
//!     let mut registry = Registry::new("https://registry.npmjs.org", transport)?;
//!     let lock = resolve::resolve(&manifest, &mut registry)?;
//!     lock.validate()?;
//!     Ok(())
//! }
//! ```
//!
//! See `docs/PEER_DEPENDENCIES.md` and `docs/LOCKFILE.md` for supported
//! semantics and known differences from npm Arborist.

#![warn(rustdoc::broken_intra_doc_links)]

pub mod config;
pub mod install;
pub mod integrity;
pub mod lockfile;
pub mod manifest;
pub mod peer;
pub mod registry;
pub mod resolve;
pub mod transport;
