//! What every lunar-extension shares: names, the manifest, plugin hosts and the
//! load-time checks.
//!
//! A host offers extensions; each extension may open plugin hosts, and each plugin
//! host offers plugins. The application's [`Manifest`] selects which of them are
//! active. [`check`] compares the two before any script runs and reports every
//! mismatch at once.
//!
//! The crate has no VM dependency: the runtime level (`lunarstorm-runtime`) and the
//! platform level apply the same rules and the same messages.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Core names an extension may not take: they are the Runtime's own
/// `lunarstorm:` modules (`lunarstorm:ipc`, `lunarstorm:worker`,
/// `lunarstorm:internal`).
pub const RESERVED: &[&str] = &["ipc", "worker", "internal"];

/// Whether `name` is a valid extension, plugin host or plugin name.
///
/// Decision: `[a-z][a-z0-9]{0,31}`. The name appears in module specifiers
/// (`lunarstorm:<extension>/<plugin>`) and in permission strings
/// (`<extension>.<command>`), so it carries no separator, no case and no
/// punctuation.
///
/// # Arguments
/// * `name` - the name to test.
///
/// # Returns
/// `true` when the name follows the rule.
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    matches!(bytes.first(), Some(b'a'..=b'z')) && bytes.len() <= 32 && bytes.iter().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9'))
}

/// How many plugins a plugin host takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cardinality {
    /// At most one active plugin.
    One,
    /// Any number of active plugins, in manifest order.
    Many,
}

/// Whether a plugin host needs a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    /// The extension cannot work without an active plugin here.
    Required,
    /// The extension works with none.
    Optional,
}

/// A named slot an extension opens for plugins, with the plugins this build offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginHost {
    /// Unique within the extension.
    pub name: &'static str,
    /// How many plugins may be active.
    pub cardinality: Cardinality,
    /// Whether one must be active.
    pub requirement: Requirement,
    /// The plugins compiled into this host for the slot. A plugin name is unique
    /// within its extension, across all of the extension's plugin hosts.
    pub plugins: Vec<&'static str>,
}

/// An extension as a host offers it: its name and its plugin hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// The extension's name.
    pub name: &'static str,
    /// Its plugin hosts; empty for an extension that takes no plugins.
    pub plugin_hosts: Vec<PluginHost>,
}

/// The declaration of which extensions and plugins an application uses.
///
/// ```json
/// { "extensions": { "hostapi": { "plugins": ["ui"] } } }
/// ```
///
/// Decision: unknown keys are refused, so a key this version does not implement
/// (for example a permission form of a later version) is an error, never silently
/// ignored.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The extensions in use, by name.
    #[serde(default)]
    pub extensions: BTreeMap<String, Entry>,
}

/// One extension's entry in the manifest.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The active plugins, in the order the extension sees them.
    #[serde(default)]
    pub plugins: Vec<String>,
    /// The extension's own configuration; its meaning is the extension's.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// The manifest of a host that uses no extensions.
static EMPTY: Manifest = Manifest { extensions: BTreeMap::new() };

impl Manifest {
    /// # Returns
    /// The manifest that uses nothing: every extension inert.
    pub fn empty() -> &'static Manifest {
        &EMPTY
    }

    /// Parses a manifest from JSON.
    ///
    /// # Arguments
    /// * `text` - the manifest's JSON text.
    ///
    /// # Returns
    /// The manifest.
    ///
    /// # Errors
    /// `manifest: <reason>` when the text is not a manifest.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        serde_json::from_str(text).map_err(|e| format!("manifest: {e}"))
    }

    /// The use of one extension.
    ///
    /// # Arguments
    /// * `name` - the extension's name.
    ///
    /// # Returns
    /// Its entry as a [`Use`], or `None` when the manifest does not use it.
    pub fn using(&self, name: &str) -> Option<Use<'_>> {
        self.extensions.get(name).map(|entry| Use { entry })
    }
}

/// What the manifest gives one used extension: its active plugins and its
/// configuration.
#[derive(Clone, Copy, Debug)]
pub struct Use<'a> {
    /// The extension's manifest entry.
    entry: &'a Entry,
}

impl<'a> Use<'a> {
    /// # Returns
    /// The active plugins, in manifest order.
    pub fn plugins(&self) -> &'a [String] {
        &self.entry.plugins
    }

    /// Whether one plugin is active.
    ///
    /// # Arguments
    /// * `plugin` - the plugin's name.
    ///
    /// # Returns
    /// `true` when the manifest lists it.
    pub fn has(&self, plugin: &str) -> bool {
        self.entry.plugins.iter().any(|p| p == plugin)
    }

    /// # Returns
    /// The extension's configuration, `None` when the manifest gives none.
    pub fn config(&self) -> Option<&'a serde_json::Value> {
        self.entry.config.as_ref()
    }
}

/// Compares a manifest with what a host offers.
///
/// # Arguments
/// * `manifest` - the application's manifest.
/// * `offers` - the host's extensions.
///
/// # Returns
/// `Ok` when every extension and plugin the manifest uses exists and every plugin
/// host's rule holds.
///
/// # Errors
/// Every mismatch, one message each, in a stable order (offers first, then the
/// manifest's extensions by name).
pub fn check(manifest: &Manifest, offers: &[Offer]) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    // The host's own offers first: a wrong one is a fault of the host's build,
    // reported with the `host:` prefix so it is not mistaken for the manifest's.
    for (index, offer) in offers.iter().enumerate() {
        if RESERVED.contains(&offer.name) {
            errors.push(format!("host: extension name {} is reserved", offer.name));
        } else if !valid_name(offer.name) {
            errors.push(format!("host: extension name {} is invalid", offer.name));
        } else if offers[..index].iter().any(|o| o.name == offer.name) {
            errors.push(format!("host: extension {} is offered twice", offer.name));
        }
        let mut seen: Vec<&str> = Vec::new();
        for slot in &offer.plugin_hosts {
            for plugin in &slot.plugins {
                if !valid_name(plugin) {
                    errors.push(format!("host: extension {}: plugin name {plugin} is invalid", offer.name));
                } else if seen.contains(plugin) {
                    errors.push(format!("host: extension {} offers plugin {plugin} twice", offer.name));
                }
                seen.push(plugin);
            }
        }
    }
    for (name, entry) in &manifest.extensions {
        let Some(offer) = offers.iter().find(|o| o.name == name) else {
            errors.push(format!("manifest: this host has no extension {name}"));
            continue;
        };
        for (index, plugin) in entry.plugins.iter().enumerate() {
            if entry.plugins[..index].contains(plugin) {
                errors.push(format!("manifest: extension {name} lists plugin {plugin} twice"));
            } else if !offer.plugin_hosts.iter().any(|slot| slot.plugins.contains(&plugin.as_str())) {
                errors.push(format!("manifest: extension {name} has no plugin {plugin}"));
            }
        }
        for slot in &offer.plugin_hosts {
            // Distinct active plugins of this slot; a duplicate is reported above.
            let mut active: Vec<&str> = Vec::new();
            for plugin in entry.plugins.iter().map(String::as_str).filter(|p| slot.plugins.contains(p)) {
                if !active.contains(&plugin) {
                    active.push(plugin);
                }
            }
            if active.is_empty() && slot.requirement == Requirement::Required {
                errors.push(format!("manifest: extension {name}: plugin host {} requires a plugin", slot.name));
            }
            if active.len() > 1 && slot.cardinality == Cardinality::One {
                errors.push(format!("manifest: extension {name}: plugin host {} takes one plugin", slot.name));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
