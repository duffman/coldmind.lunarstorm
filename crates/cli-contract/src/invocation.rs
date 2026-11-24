//! What a handler receives: the command path, the parsed values and the context.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::Failure;

/// A parsed command line, handed to every `enter` handler on the path and then to the
/// leaf's `run`.
pub struct Invocation {
    /// The command names from the top level down, e.g. `["build", "types"]`. A default
    /// command the user did not type is still named here.
    pub path: Vec<String>,
    /// The values of every argument on the path.
    pub values: Values,
    /// Typed values produced by the `enter` handlers above, and anything the host put
    /// there before dispatch.
    pub context: Context,
}

/// Argument values by argument name.
///
/// Decision: one map for the whole path. Names are unique along a path (the host
/// rejects a command that redeclares a name an ancestor already uses), so a child
/// reads its parent's options and the globals with the same getters as its own.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Values {
    /// Every value given or defaulted, in command-line order; a present flag has an
    /// empty list.
    map: HashMap<String, Vec<String>>,
}

impl Values {
    /// Records a flag as present.
    ///
    /// # Arguments
    /// * `name` - the flag's name.
    pub fn set_flag(&mut self, name: &str) {
        self.map.entry(name.to_owned()).or_default();
    }

    /// Appends a value.
    ///
    /// # Arguments
    /// * `name` - the argument's name.
    /// * `value` - the value, already checked against the argument's type.
    pub fn push(&mut self, name: &str, value: String) {
        self.map.entry(name.to_owned()).or_default().push(value);
    }

    /// Tells whether an argument was given, defaulted, or read from its env variable.
    ///
    /// # Arguments
    /// * `name` - the argument's name.
    ///
    /// # Returns
    /// `true` when present.
    pub fn has(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    /// Reads a flag.
    ///
    /// # Arguments
    /// * `name` - the flag's name.
    ///
    /// # Returns
    /// `true` when the flag was given.
    pub fn flag(&self, name: &str) -> bool {
        self.has(name)
    }

    /// Reads a single value.
    ///
    /// # Arguments
    /// * `name` - the argument's name.
    ///
    /// # Returns
    /// The last value given, or `None`.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.map.get(name)?.last().map(String::as_str)
    }

    /// Reads a single value as a path.
    ///
    /// # Arguments
    /// * `name` - the argument's name.
    ///
    /// # Returns
    /// The last value given, or `None`.
    pub fn path(&self, name: &str) -> Option<PathBuf> {
        self.text(name).map(PathBuf::from)
    }

    /// Reads a single value as an integer.
    ///
    /// # Arguments
    /// * `name` - the argument's name; declared with `ValueType::Int`.
    ///
    /// # Returns
    /// The last value given, or `None` when absent or not declared as an integer.
    pub fn int(&self, name: &str) -> Option<i64> {
        self.text(name)?.parse().ok()
    }

    /// Reads every value of a repeated option or a rest argument.
    ///
    /// # Arguments
    /// * `name` - the argument's name.
    ///
    /// # Returns
    /// The values in command-line order; empty when absent.
    pub fn all(&self, name: &str) -> &[String] {
        self.map.get(name).map_or(&[], Vec::as_slice)
    }
}

/// Values keyed by their type: how one command level hands state to the levels below
/// it.
///
/// Decision: keyed by type, not by name, so a producer and its consumers agree through
/// a Rust type both can see (usually a struct the package defines next to its
/// commands) and a lookup can never return the wrong type. One value per type; a
/// deeper `enter` that inserts the same type replaces the outer one.
#[derive(Default)]
pub struct Context {
    /// The values, boxed by type.
    items: HashMap<TypeId, Box<dyn Any>>,
}

impl Context {
    /// Stores a value, replacing one of the same type.
    ///
    /// # Arguments
    /// * `value` - the value.
    pub fn insert<T: 'static>(&mut self, value: T) {
        self.items.insert(TypeId::of::<T>(), Box::new(value));
    }

    /// Looks a value up.
    ///
    /// # Returns
    /// The value of type `T`, or `None`.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.items.get(&TypeId::of::<T>())?.downcast_ref()
    }

    /// Looks up a value a parent level must have produced.
    ///
    /// # Returns
    /// The value of type `T`.
    ///
    /// # Errors
    /// [`Failure::Failed`] naming the type when it is missing: a package wired a
    /// command below a level that does not produce what it needs.
    pub fn require<T: 'static>(&self) -> Result<&T, Failure> {
        self.get().ok_or_else(|| Failure::failed(format!("internal: no {} in the command context", std::any::type_name::<T>())))
    }
}
