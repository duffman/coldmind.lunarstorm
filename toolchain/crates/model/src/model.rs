//! The extracted project model and the diagnostics extraction reports.
//!
//! Decisions:
//! - The model owns its data (no AST lifetimes), so the frontend, backend and host
//!   configuration generators can hold it after the sources are dropped.
//! - Props keep source order. Generators emit in that order, so output is stable.
//! - A diagnostic carries a [`Code`], so tests and the dev overlay match on the
//!   kind of error and never on message text.

use std::path::PathBuf;

/// A prop value. The parse rules (spec 2.2) allow nothing else.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A string literal.
    Text(String),
    /// A numeric literal, with an optional sign.
    Number(f64),
    /// A boolean literal; a prop written without a value is `true`.
    Bool(bool),
    /// An array of scalar literals.
    List(Vec<Value>),
    /// `form.<member>`: a handler on the form class, by member name.
    Handler(String),
}

/// A position in a source file, 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    /// Line number.
    pub line: u32,
    /// Column, counted in characters.
    pub column: u32,
}

/// One prop of an element.
#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    /// The prop name as written.
    pub name: String,
    /// Its value.
    pub value: Value,
    /// Where the prop starts.
    pub at: Position,
}

/// One element of a resource: the application, a form, a control or a component.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// The element name; always a tag of the element table.
    pub tag: &'static str,
    /// The `name` prop; empty for the tags that carry none.
    pub name: String,
    /// Every prop except `name`, in source order.
    pub props: Vec<Prop>,
    /// Child elements, in source order.
    pub children: Vec<Element>,
    /// Where the element starts.
    pub at: Position,
}

impl Element {
    /// Finds a prop by name.
    ///
    /// # Arguments
    /// * `name` - the prop name.
    ///
    /// # Returns
    /// The prop's value, or `None` when the element does not set it.
    pub fn prop(&self, name: &str) -> Option<&Value> {
        self.props.iter().find(|prop| prop.name == name).map(|prop| &prop.value)
    }

    /// Visits this element and every descendant, parents first.
    ///
    /// # Arguments
    /// * `visit` - called once per element.
    pub fn walk<'a>(&'a self, visit: &mut impl FnMut(&'a Element)) {
        visit(self);
        for child in &self.children {
            child.walk(visit);
        }
    }
}

/// A form: its view resource paired with its form class.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    /// The form's name; also the name of its class.
    pub name: String,
    /// The view resource, relative to the project root.
    pub view: PathBuf,
    /// The form class file, relative to the project root.
    pub class: PathBuf,
    /// The `<form>` element.
    pub root: Element,
}

/// The whole project.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    /// The `<application>` element of `src/app.tsx`.
    pub application: Element,
    /// Every form, ordered by view resource path.
    pub forms: Vec<Form>,
    /// The native API methods the project's code references, as `namespace.method`,
    /// sorted. The native permissions are derived from these (spec 2.6).
    pub natives: Vec<String>,
}

/// The kind of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// A required file is missing or unreadable.
    File,
    /// The file is not valid TypeScript/JSX.
    Syntax,
    /// A resource holds a statement other than an import, a declaration or its
    /// default export.
    Statement,
    /// The default export is missing or is not the expected root element.
    Root,
    /// An element name is not a tag of the model (a component, a fragment, a
    /// member or namespaced name, or an unknown lower-case name).
    Tag,
    /// A `name` is missing, not a string literal or not an identifier.
    Name,
    /// A name is used twice within one resource, or two forms share a name.
    Duplicate,
    /// An element sits where it may not, or a leaf has children.
    Nesting,
    /// A prop value is not a literal or `form.<member>`.
    Value,
    /// A prop is set twice on one element.
    Repeated,
    /// An event prop does not name a handler, or a plain prop does.
    Event,
    /// A handler does not exist on the form class, or is not public.
    Handler,
    /// The form class file does not export a class with the form's name that
    /// extends another class.
    Class,
    /// `mainForm` is missing or names no form.
    MainForm,
    /// An element has no such prop, or the prop is runtime-only data.
    Prop,
    /// A prop value has the wrong type or is not one of the allowed values.
    Type,
    /// A prop names an element that does not exist or is of the wrong kind.
    Reference,
    /// A `this.find<T>('Name')` in a form class names no control of the form's
    /// view resource, or `T` is not that control's type (spec 4: resolved at
    /// compile time).
    Find,
}

/// One compile error.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// What kind of error it is.
    pub code: Code,
    /// The file, relative to the project root.
    pub file: PathBuf,
    /// Where in the file.
    pub at: Position,
    /// The message for the developer.
    pub message: String,
}

/// The result of extraction.
#[derive(Debug, Clone, PartialEq)]
pub struct Extraction {
    /// The model. Present only when there is no diagnostic: a model with errors
    /// must never reach a generator.
    pub project: Option<Project>,
    /// Every compile error, ordered by file, then by position.
    pub diagnostics: Vec<Diagnostic>,
}
