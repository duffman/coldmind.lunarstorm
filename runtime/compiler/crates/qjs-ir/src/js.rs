//! Owned JavaScript source IR. Values and effects retain JavaScript semantics;
//! scalar SSA functions elsewhere in this crate remain unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclarationKind {
    Var,
    Let,
    Const,
    Parameter,
    Function,
    FunctionName,
    Import,
    /// Simple catch parameter: scoped native JS_VAR_CATCH, not a let TDZ slot.
    Catch,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub symbol: u32,
    pub scope: u32,
    pub name: String,
    pub kind: DeclarationKind,
    pub span: Span,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub symbol: Option<u32>,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    BigInt(Vec<u8>),
    Number(f64),
    Bool(bool),
    String(Vec<u16>),
    Null,
    Undefined,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unary {
    Negate,
    Plus,
    Not,
    BitNot,
    Typeof,
    Void,
    Delete,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binary {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Exponent,
    Equal,
    NotEqual,
    StrictEqual,
    StrictNotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
    ShiftUnsigned,
    In,
    Instanceof,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Logical {
    And,
    Or,
    Nullish,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignmentOp {
    Assign,
    Compound(Binary),
    Logical(Logical),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassMethodKind {
    Method,
    Get,
    Set,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivateKind {
    Field,
    Method,
    Accessor,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PrivateName {
    pub id: u32,
    pub name: Vec<u16>,
    pub is_static: bool,
    pub kind: PrivateKind,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ClassKey {
    Public(ObjectKey),
    Private(u32),
}
#[derive(Clone, Debug, PartialEq)]
pub enum ClassElement {
    Method {
        key: ClassKey,
        function: Function,
        kind: ClassMethodKind,
        is_static: bool,
    },
    Field {
        key: ClassKey,
        initializer: Option<Function>,
        is_static: bool,
    },
    StaticBlock(Function),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Class {
    pub name: Option<String>,
    pub self_binding: Option<Binding>,
    pub heritage: Option<Box<Expression>>,
    pub constructor: Option<Box<Function>>,
    pub elements: Vec<ClassElement>,
    pub private_names: Vec<PrivateName>,
    pub instance_initializer_binding: Binding,
    pub scope: u32,
    pub span: Span,
}
#[derive(Clone, Debug, PartialEq)]
pub enum CallArgument {
    Value(Expression),
    Spread(Expression),
}
/// Property syntax distinguishes static names from evaluated keys. Computed
/// keys are converted once before the property's value/function is evaluated.
#[derive(Clone, Debug, PartialEq)]
pub enum ObjectKey {
    Static(Vec<u16>),
    Computed(Expression),
}
#[derive(Clone, Debug, PartialEq)]
pub enum ObjectEntry {
    Data {
        key: ObjectKey,
        value: Expression,
        method: bool,
        infer_name: bool,
    },
    Getter {
        key: ObjectKey,
        value: Expression,
    },
    Setter {
        key: ObjectKey,
        value: Expression,
    },
    /// Only noncomputed colon-form __proto__ is prototype syntax.
    Prototype {
        value: Expression,
    },
    /// Copy enumerable own properties; this does not use iterator semantics.
    Spread {
        value: Expression,
    },
}
/// One contiguous optional-chain boundary. Parenthesized inner chains remain
/// nested expressions, rather than sharing their outer short-circuit target.
#[derive(Clone, Debug, PartialEq)]
pub enum ChainSegment {
    PrivateMember {
        id: u32,
        optional: bool,
        span: Span,
    },
    Member {
        key: Expression,
        optional: bool,
        span: Span,
    },
    Call {
        arguments: Vec<CallArgument>,
        optional: bool,
        span: Span,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub enum ExpressionKind {
    NamedEvaluation {
        name: Box<Expression>,
        value: Box<Expression>,
    },
    Class(Box<Class>),
    SuperMember {
        key: Box<Expression>,
    },
    AssignSuper {
        key: Box<Expression>,
        value: Box<Expression>,
        op: AssignmentOp,
    },
    UpdateSuper {
        key: Box<Expression>,
        increment: bool,
        prefix: bool,
    },
    SuperCall {
        arguments: Vec<CallArgument>,
    },
    PrivateMember {
        object: Box<Expression>,
        id: u32,
    },
    AssignPrivate {
        object: Box<Expression>,
        id: u32,
        value: Box<Expression>,
        op: AssignmentOp,
    },
    UpdatePrivate {
        object: Box<Expression>,
        id: u32,
        increment: bool,
        prefix: bool,
    },
    PrivateIn {
        object: Box<Expression>,
        id: u32,
    },
    Literal(Literal),
    Sequence(Vec<Expression>),
    Template {
        parts: Vec<TemplatePart>,
        substitutions: Vec<Expression>,
    },
    Conditional {
        test: Box<Expression>,
        consequent: Box<Expression>,
        alternate: Box<Expression>,
    },
    Object(Vec<ObjectEntry>),
    Array(Vec<Expression>),
    Member {
        object: Box<Expression>,
        key: Box<Expression>,
    },
    /// Can yield a member reference to an enclosing call/delete; converting
    /// every chain result to a value would lose the method receiver.
    Chain {
        base: Box<Expression>,
        segments: Vec<ChainSegment>,
    },
    Read(Reference),
    /// Annex B: a call used as a sloppy-mode assignment target. The call runs, then a
    /// ReferenceError is thrown; no value is produced.
    InvalidCallTarget(Box<Expression>),
    This,
    NewTarget,
    Unary {
        op: Unary,
        argument: Box<Expression>,
    },
    Binary {
        op: Binary,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    Logical {
        op: Logical,
        left: Box<Expression>,
        right: Box<Expression>,
    },
    DestructureAssign {
        pattern: Box<Pattern>,
        value: Box<Expression>,
    },
    Assign {
        target: Reference,
        value: Box<Expression>,
    },
    Update {
        target: Reference,
        increment: bool,
        prefix: bool,
    },
    /// Preserve a binding reference across the short-circuit test and RHS.
    LogicalAssign {
        target: Reference,
        op: Logical,
        value: Box<Expression>,
    },
    /// Member operations evaluate object and key once, converting the key to
    /// a property key before GetValue or RHS effects, and retain that reference.
    UpdateMember {
        object: Box<Expression>,
        key: Box<Expression>,
        increment: bool,
        prefix: bool,
    },
    CompoundAssignMember {
        object: Box<Expression>,
        key: Box<Expression>,
        op: Binary,
        value: Box<Expression>,
    },
    LogicalAssignMember {
        object: Box<Expression>,
        key: Box<Expression>,
        op: Logical,
        value: Box<Expression>,
    },
    AssignMember {
        object: Box<Expression>,
        key: Box<Expression>,
        value: Box<Expression>,
    },
    Construct {
        callee: Box<Expression>,
        arguments: Vec<CallArgument>,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<CallArgument>,
    },
    Function(Box<Function>),
    /// An array literal containing spread elements or holes.
    SpreadArray(Vec<ArrayElement>),
    /// A regular expression literal: original pattern and flags units.
    RegExp {
        pattern: Vec<u16>,
        flags: Vec<u16>,
    },
    /// `import.meta` of the enclosing module.
    ImportMeta,
    /// `eval(...)`: a direct eval when the callee is %eval% at run time,
    /// otherwise an ordinary call with an undefined receiver.
    DirectEval {
        callee: Box<Expression>,
        arguments: Vec<CallArgument>,
    },
    /// The per-site frozen template object of a tagged template: cooked
    /// strings (None for an invalid escape) and the frozen `raw` array.
    TemplateObject {
        cooked: Vec<Option<Vec<u16>>>,
        raw: Vec<Vec<u16>>,
    },
    /// Dynamic `import(specifier, options)`.
    ImportCall {
        specifier: Box<Expression>,
        options: Option<Box<Expression>>,
    },
    /// `await value`: suspends an async function or async module body.
    Await(Box<Expression>),
    /// `yield value` or `yield* iterable` inside a generator.
    Yield {
        argument: Option<Box<Expression>>,
        delegate: bool,
    },
}
/// One element of an array literal with spread elements or holes.
#[derive(Clone, Debug, PartialEq)]
pub enum ArrayElement {
    Value(Expression),
    Spread(Expression),
    Hole,
}
/// Native function kind: suspension protocol of the function body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FunctionKind {
    #[default]
    Normal,
    Generator,
    Async,
    AsyncGenerator,
}
impl FunctionKind {
    /// The native `func_kind` bits: generator 1, async 2.
    pub fn native(self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Generator => 1,
            Self::Async => 2,
            Self::AsyncGenerator => 3,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Declarator {
    pub pattern: Pattern,
    pub initializer: Option<Expression>,
    /// `using` (`Some(false)`) or `await using` (`Some(true)`): a const binding whose value is
    /// disposed when its scope ends.
    pub using: Option<bool>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}
/// Shared binding/assignment syntax. The consumer controls whether identifiers
/// are initialized, assigned, or declared; source expressions remain explicit.
#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    SuperMember {
        key: Box<Expression>,
    },
    PrivateMember {
        object: Box<Expression>,
        id: u32,
    },
    Binding {
        binding: Binding,
        initialization_target: Option<Reference>,
    },
    Target(Reference),
    Member {
        object: Box<Expression>,
        key: Box<Expression>,
    },
    Default {
        pattern: Box<Pattern>,
        value: Expression,
    },
    Array {
        elements: Vec<Option<Pattern>>,
        rest: Option<Box<Pattern>>,
    },
    Object {
        properties: Vec<PatternProperty>,
        rest: Option<Box<Pattern>>,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct PatternProperty {
    pub key: ObjectKey,
    pub pattern: Pattern,
}
impl Pattern {
    pub fn for_each_binding(&self, visit: &mut impl FnMut(&Binding)) {
        match self {
            Self::Binding { binding, .. } => visit(binding),
            Self::Default { pattern, .. } => pattern.for_each_binding(visit),
            Self::Array { elements, rest } => {
                for element in elements.iter().flatten() {
                    element.for_each_binding(visit);
                }
                if let Some(rest) = rest {
                    rest.for_each_binding(visit);
                }
            }
            Self::Object { properties, rest } => {
                for property in properties {
                    property.pattern.for_each_binding(visit);
                }
                if let Some(rest) = rest {
                    rest.for_each_binding(visit);
                }
            }
            Self::Target(_)
            | Self::Member { .. }
            | Self::SuperMember { .. }
            | Self::PrivateMember { .. } => {}
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct BodyParameterCopy {
    pub parameter: Reference,
    pub binding: Binding,
}
/// A loop declaration instantiates its binding separately from assignment of
/// each produced value. Annex B catch/var targets use initialization_target.
#[derive(Clone, Debug, PartialEq)]
pub enum ForTarget {
    Declaration(Declarator),
    Assignment(Pattern),
}
#[derive(Clone, Debug, PartialEq)]
pub struct SwitchCase {
    pub test: Option<Expression>,
    pub consequent: Vec<Statement>,
    pub span: Span,
}
#[derive(Clone, Debug, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "statements are built and matched by value across the compiler; boxing one variant would touch every site"
)]
pub enum StatementKind {
    Class {
        binding: Binding,
        class: Class,
    },
    Empty,
    /// `debugger;`: flags the statement's debug marker as a breakpoint.
    Debugger,
    Expression(Expression),
    Declaration(Vec<Declarator>),
    Block(Vec<Statement>),
    If {
        test: Expression,
        consequent: Box<Statement>,
        alternate: Option<Box<Statement>>,
    },
    /// `with (object) body`; `scope` is the with statement's own scope (contains the body).
    With {
        object: Expression,
        body: Box<Statement>,
        scope: u32,
    },
    While {
        test: Expression,
        body: Box<Statement>,
    },
    For {
        init: Option<Box<Statement>>,
        test: Option<Expression>,
        update: Option<Expression>,
        body: Box<Statement>,
    },
    DoWhile {
        test: Expression,
        body: Box<Statement>,
    },
    ForOf {
        left: ForTarget,
        right: Expression,
        body: Box<Statement>,
        scope: u32,
        /// `for await (... of ...)`.
        is_await: bool,
    },
    ForIn {
        left: ForTarget,
        right: Expression,
        body: Box<Statement>,
        scope: u32,
    },
    Break,
    Continue,
    BreakLabel(String),
    ContinueLabel(String),
    Label {
        label: String,
        body: Box<Statement>,
    },
    Switch {
        discriminant: Expression,
        cases: Vec<SwitchCase>,
        scope: u32,
    },
    Return(Option<Expression>),
    Throw(Expression),
    TryCatch {
        body: Vec<Statement>,
        binding: Option<Pattern>,
        handler: Vec<Statement>,
        body_scope: u32,
        catch_scope: u32,
        handler_scope: u32,
    },
    /// A synthetic body wrapping TryCatch has no additional lexical scope.
    /// The finalizer runs for every normal or abrupt completion of the body.
    TryFinally {
        body: Vec<Statement>,
        finalizer: Vec<Statement>,
        body_scope: Option<u32>,
        finalizer_scope: u32,
    },
    Function {
        binding: Binding,
        function: Function,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub initializer_name_binding: Option<Binding>,
    pub arguments_allowed: bool,
    pub is_class_constructor: bool,
    pub derived_constructor: bool,
    pub home_object_binding: Option<Binding>,
    pub uses_home_object: bool,
    pub super_constructor_binding: Option<Binding>,
    pub field_initializer_binding: Option<Binding>,
    pub super_call_allowed: bool,
    pub is_arrow: bool,
    pub new_target_binding: Option<Binding>,
    pub uses_new_target: bool,
    pub this_binding: Option<Binding>,
    pub uses_this: bool,
    pub arguments_binding: Option<Binding>,
    pub is_method: bool,
    pub self_binding: Option<Binding>,
    pub name: Option<String>,
    pub scope: u32,
    pub parameters: Vec<Pattern>,
    pub has_parameter_expressions: bool,
    pub defined_arg_count: u32,
    pub body_parameter_copies: Vec<BodyParameterCopy>,
    /// Initialized after regular parameters; never contributes to function length.
    pub rest_parameter: Option<Pattern>,
    pub body: Vec<Statement>,
    pub strict: bool,
    /// The function body (arrows included, nested functions excluded) calls `eval` directly.
    pub has_direct_eval: bool,
    pub span: Span,
    pub kind: FunctionKind,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Scope {
    pub id: u32,
    pub parent: Option<u32>,
    pub strict: bool,
    pub function: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub this_binding: Option<Binding>,
    pub uses_this: bool,
    pub module: Option<Module>,
    pub scopes: Vec<Scope>,
    pub filename: String,
    pub source: Vec<u16>,
    pub body: Vec<Statement>,
    pub strict: bool,
    pub scope: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TemplatePart {
    pub raw: Vec<u16>,
    pub cooked: Vec<u16>,
}

/// Static module linkage; all names retain their UTF-16 units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Module {
    pub requests: Vec<Vec<u16>>,
    /// lunarstorm patch: the import attributes (`with { key: "value" }`) of
    /// each request, parallel to `requests`, in source order; empty for none.
    /// One request per specifier; a second, different attribute set for the
    /// same specifier is rejected by the front end.
    pub request_attributes: Vec<Vec<(Vec<u16>, Vec<u16>)>>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub binding: Binding,
    pub request: u32,
    pub name: Vec<u16>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Export {
    Local {
        symbol: u32,
        name: Vec<u16>,
    },
    Indirect {
        request: u32,
        import: Vec<u16>,
        name: Vec<u16>,
    },
    Star {
        request: u32,
    },
}
