//! Declarations of what a script can reach: the type of every native function,
//! class and module the Runtime, a host or an extension installs.
//!
//! One declaration drives both the installed binding (its arguments are checked
//! against it at the call boundary) and, later, the generated TypeScript types.
//!
//! Decisions:
//! - Plain data, no dependencies. Nested types are `&'static` references rather
//!   than `Box`, so every declaration except a module's item list can be a
//!   `const`: no allocation, and a declaration costs nothing until it is read.
//! - A module's items are a `Vec`, because the Runtime merges the items a host and
//!   its extensions add to one module (`lunarstorm:internal`) at run time.
//! - Type names are spelled as TypeScript spells them (`number`, `Uint8Array`,
//!   `string | undefined`); the same text serves error messages and `.d.ts` output.

use crate::{Kind, Scope, Vm};
use std::fmt;

/// The type of a value crossing the native boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ty {
    /// `undefined`
    Undefined,
    /// `null`
    Null,
    /// A boolean.
    Bool,
    /// A number.
    Number,
    /// A string.
    String,
    /// A `Uint8Array` (a `Buffer` is one).
    Bytes,
    /// Anything; the native inspects it itself.
    Any,
    /// Anything, but a script must narrow it before use.
    Unknown,
    /// An array of one item type.
    Array(&'static Ty),
    /// An object with these fields; an empty list is any object.
    Object(&'static [Field]),
    /// One of several types.
    Union(&'static [Ty]),
    /// A promise of a type.
    Promise(&'static Ty),
    /// A function of a signature.
    Function(&'static Sig),
    /// A type declared elsewhere, by name: a class or an alias.
    Named(&'static str),
    /// The type or `undefined`; a parameter of this type may be left out.
    Optional(&'static Ty),
}

/// One parameter of a [`Sig`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Param {
    /// The name, used in error messages and type output.
    pub name: &'static str,
    /// The type a call must pass.
    pub ty: Ty,
    /// One line of documentation.
    pub about: &'static str,
}

/// A function signature.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sig {
    /// The positional parameters, in order.
    pub params: &'static [Param],
    /// The rest parameter (`...name`), checked against every extra argument.
    pub rest: Option<Param>,
    /// What the function returns. Not checked: the native builds it.
    pub returns: Ty,
    /// What the function does, one line.
    pub about: &'static str,
}

/// A named property: an object type's field, a class accessor or a module value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    /// The property name.
    pub name: &'static str,
    /// Its type.
    pub ty: Ty,
    /// One line of documentation.
    pub about: &'static str,
}

/// A named function: a module export, an `ops` member or a class method.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FnDecl {
    /// The function's name.
    pub name: &'static str,
    /// Its signature; `sig.about` documents it.
    pub sig: Sig,
}

/// A native class.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassDecl {
    /// The class name.
    pub name: &'static str,
    /// The base class, by name: another native class or a builtin (`Uint8Array`).
    pub extends: Option<&'static str>,
    /// The constructor's signature.
    pub constructor: Sig,
    /// Prototype methods.
    pub methods: &'static [FnDecl],
    /// Prototype accessors.
    pub accessors: &'static [Field],
    /// Static methods.
    pub statics: &'static [FnDecl],
    /// What the class is, one line.
    pub about: &'static str,
}

/// One export of a [`ModuleDecl`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    /// A function.
    Function(FnDecl),
    /// A class.
    Class(ClassDecl),
    /// A value that is not called: a constant or an object.
    Value(Field),
}

impl Item {
    /// # Returns
    /// The export's name.
    pub fn name(&self) -> &'static str {
        match self {
            Item::Function(function) => function.name,
            Item::Class(class) => class.name,
            Item::Value(field) => field.name,
        }
    }
}

/// A module scripts (or host modules) import by specifier.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDecl {
    /// What `import` names, e.g. `node:path` or `lunarstorm:internal`.
    pub specifier: &'static str,
    /// The exports, in declaration order.
    pub items: Vec<Item>,
    /// What the module is, one line.
    pub about: &'static str,
}

impl fmt::Display for Ty {
    /// Writes the TypeScript spelling: `number`, `string[]`, `string | undefined`.
    ///
    /// # Arguments
    /// * `f` - the formatter.
    ///
    /// # Errors
    /// The formatter's error.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Undefined => f.write_str("undefined"),
            Ty::Null => f.write_str("null"),
            Ty::Bool => f.write_str("boolean"),
            Ty::Number => f.write_str("number"),
            Ty::String => f.write_str("string"),
            Ty::Bytes => f.write_str("Uint8Array"),
            Ty::Any => f.write_str("any"),
            Ty::Unknown => f.write_str("unknown"),
            Ty::Array(item @ (Ty::Union(_) | Ty::Optional(_) | Ty::Function(_))) => write!(f, "({item})[]"),
            Ty::Array(item) => write!(f, "{item}[]"),
            Ty::Object(_) => f.write_str("object"),
            Ty::Union(types) => {
                for (index, ty) in types.iter().enumerate() {
                    if index > 0 {
                        f.write_str(" | ")?;
                    }
                    write!(f, "{ty}")?;
                }
                Ok(())
            }
            Ty::Promise(ty) => write!(f, "Promise<{ty}>"),
            Ty::Function(_) => f.write_str("function"),
            Ty::Named(name) => f.write_str(name),
            Ty::Optional(ty) => write!(f, "{ty} | undefined"),
        }
    }
}

impl Ty {
    /// Checks a value against this type.
    ///
    /// Decision: the check is shallow. Containers (`Array`, `Object`, `Promise`,
    /// `Named`) only check that the value is an object, and `Function` that it is
    /// callable; their contents are the native's to read. A deep walk would cost
    /// one engine call per element on every call, for shapes only the glue builds.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `value` - the value to check.
    ///
    /// # Returns
    /// `Ok` when the value fits.
    ///
    /// # Errors
    /// This type's name, for the message `<fn>: <param> must be <type>`.
    pub fn check<'s, V: Vm>(&self, scope: &V::Scope<'s>, value: &V::Value<'s>) -> Result<(), String> {
        if self.admits(scope.kind(value)) {
            Ok(())
        } else {
            Err(self.to_string())
        }
    }

    /// # Arguments
    /// * `kind` - the coarse type of a value.
    ///
    /// # Returns
    /// Whether a value of that kind fits this type, under [`Ty::check`]'s rules.
    pub fn admits(&self, kind: Kind) -> bool {
        match self {
            Ty::Undefined => kind == Kind::Undefined,
            Ty::Null => kind == Kind::Null,
            Ty::Bool => kind == Kind::Bool,
            Ty::Number => kind == Kind::Number,
            Ty::String => kind == Kind::String,
            Ty::Bytes => kind == Kind::Bytes,
            Ty::Any | Ty::Unknown => true,
            // A Uint8Array is an object too, and a function may carry properties.
            Ty::Array(_) | Ty::Object(_) | Ty::Promise(_) | Ty::Named(_) => {
                matches!(kind, Kind::Object | Kind::Bytes | Kind::Function)
            }
            Ty::Function(_) => kind == Kind::Function,
            Ty::Union(types) => types.iter().any(|ty| ty.admits(kind)),
            Ty::Optional(ty) => kind == Kind::Undefined || ty.admits(kind),
        }
    }
}

impl Sig {
    /// Checks a call's arguments: a missing argument counts as `undefined`, and
    /// extra arguments are checked against [`Sig::rest`] or else ignored, as JS
    /// ignores them.
    ///
    /// # Arguments
    /// * `scope` - the context handle.
    /// * `args` - the arguments of the call.
    ///
    /// # Returns
    /// `Ok` when every argument fits.
    ///
    /// # Errors
    /// `<param> must be <type>` for the first argument that does not fit.
    pub fn check<'s, V: Vm>(&self, scope: &V::Scope<'s>, args: &[V::Value<'s>]) -> Result<(), String> {
        for (index, param) in self.params.iter().enumerate() {
            let fits = match args.get(index) {
                Some(value) => param.ty.admits(scope.kind(value)),
                None => param.ty.admits(Kind::Undefined),
            };
            if !fits {
                return Err(format!("{} must be {}", param.name, param.ty));
            }
        }
        if let Some(rest) = &self.rest {
            for value in args.iter().skip(self.params.len()) {
                rest.ty.check::<V>(scope, value).map_err(|ty| format!("{} must be {ty}", rest.name))?;
            }
        }
        Ok(())
    }
}
