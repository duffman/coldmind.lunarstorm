//! Extensions at the runtime level: functionality a host installs in each of its
//! Runtimes, selected by the host's manifest.
//!
//! An extension contributes a [`Bindings`] set, the mechanism hosts already use:
//! natives on `ops` and source modules. Its modules are host modules, so they may
//! import `lunarstorm:internal`; a program's own files may not.
//!
//! Decisions:
//! - A host lists its extensions as a type ([`crate::Host::Extensions`]), a tuple of
//!   [`Extension`]s, so registration and every call are resolved at compile time,
//!   like every Runtime/VM/Host call.
//! - [`crate::Runtime::start`] asks the host for a fresh set and calls
//!   [`Extension::bindings`] there. A worker's Runtime is created from its own
//!   host, so each Runtime, the main one and every worker, holds its own state.
//! - An extension serves only its own namespace, `lunarstorm:<NAME>` and
//!   `lunarstorm:<NAME>/...`; names are unique per host, so two extensions cannot
//!   claim one specifier.

use lunarstorm_engine_contract::Vm;

use crate::bindings::Bindings;

pub use lunar_extension::{check, valid_name, Cardinality, Entry, Manifest, Offer, PluginHost, Requirement, Use, RESERVED};

/// A named package of functionality a host installs in each of its Runtimes.
pub trait Extension<V: Vm>: 'static {
    /// The extension's name, `[a-z][a-z0-9]{0,31}` and not one of [`RESERVED`].
    /// Its modules are `lunarstorm:<NAME>` and `lunarstorm:<NAME>/...`.
    const NAME: &'static str;

    /// # Returns
    /// The plugin hosts it opens, each with the plugins this build offers there.
    fn plugin_hosts(&self) -> Vec<PluginHost> {
        Vec::new()
    }

    /// Builds what the extension installs in one Runtime. Called at
    /// [`crate::Runtime::start`] of every Runtime whose host's manifest uses it,
    /// after the manifest passed [`check`].
    ///
    /// # Arguments
    /// * `using` - the active plugins, in manifest order, and the configuration.
    ///
    /// # Returns
    /// Its natives and modules. State for this Runtime is captured by the
    /// natives' closures.
    ///
    /// # Errors
    /// A message; `start` fails with `extension <NAME>: <message>`.
    fn bindings(&self, using: &Use<'_>) -> Result<Bindings<V>, String>;
}

/// A host's extensions as one static type: `()`, `(A,)`, `(A, B)`, ... up to 8.
/// The tuple's order is the order their bindings are installed in.
pub trait ExtensionSet<V: Vm>: 'static {
    /// # Returns
    /// Every member as the manifest check sees it.
    fn offers(&self) -> Vec<Offer>;

    /// Adds the bindings of every member the manifest uses.
    ///
    /// # Arguments
    /// * `manifest` - the host's manifest, already checked.
    /// * `into` - the Runtime's bindings: the host's own, then each extension's.
    ///
    /// # Errors
    /// The first failing `bindings` or merge, prefixed with the extension's name.
    fn install(&self, manifest: &Manifest, into: &mut Bindings<V>) -> Result<(), String>;
}

impl<V: Vm> ExtensionSet<V> for () {
    /// # Returns
    /// Nothing: a host without extensions.
    fn offers(&self) -> Vec<Offer> {
        Vec::new()
    }

    /// Installs nothing.
    ///
    /// # Arguments
    /// * `_manifest` - unused.
    /// * `_into` - unused.
    fn install(&self, _manifest: &Manifest, _into: &mut Bindings<V>) -> Result<(), String> {
        Ok(())
    }
}

/// Implements [`ExtensionSet`] for a tuple of extensions.
macro_rules! extension_set {
    ($($member:ident $index:tt),+) => {
        impl<V: Vm, $($member: Extension<V>),+> ExtensionSet<V> for ($($member,)+) {
            /// # Returns
            /// One offer per member, in tuple order.
            fn offers(&self) -> Vec<Offer> {
                vec![$(Offer { name: $member::NAME, plugin_hosts: self.$index.plugin_hosts() }),+]
            }

            /// Installs each used member in tuple order.
            ///
            /// # Arguments
            /// * `manifest` - the host's manifest, already checked.
            /// * `into` - the Runtime's bindings.
            ///
            /// # Errors
            /// The first failing member's message, prefixed with its name.
            fn install(&self, manifest: &Manifest, into: &mut Bindings<V>) -> Result<(), String> {
                $(
                    if let Some(using) = manifest.using($member::NAME) {
                        let bindings = self.$index.bindings(&using).map_err(|e| format!("extension {}: {e}", $member::NAME))?;
                        into.merge($member::NAME, bindings).map_err(|e| format!("extension {}: {e}", $member::NAME))?;
                    }
                )+
                Ok(())
            }
        }
    };
}

extension_set!(A 0);
extension_set!(A 0, B 1);
extension_set!(A 0, B 1, C 2);
extension_set!(A 0, B 1, C 2, D 3);
extension_set!(A 0, B 1, C 2, D 3, E 4);
extension_set!(A 0, B 1, C 2, D 3, E 4, F 5);
extension_set!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
extension_set!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);
