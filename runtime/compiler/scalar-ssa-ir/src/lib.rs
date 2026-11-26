//! A bounded, typed scalar SSA IR. Every operation has executable semantics.
//!
//! Numbers use IEEE-754 binary64 arithmetic without JavaScript coercion.
//! The IR deliberately excludes strings, objects, exceptions and side effects.

#[path = "constant_folding.rs"]
mod fold;
#[path = "ir_interpreter.rs"]
mod interpreter;
#[path = "ir_validation.rs"]
mod validation;

pub use fold::fold_constants;
pub use interpreter::execute;
pub use validation::{validate, validate_with_limits, ValidationLimits};

/// Byte offsets into the function's single source, with an exclusive end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    Number,
    Bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scalar {
    Number(f64),
    Bool(bool),
}

impl Scalar {
    pub fn ty(self) -> Type {
        match self {
            Self::Number(_) => Type::Number,
            Self::Bool(_) => Type::Bool,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Negate,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Equal,
    LessThan,
    LessEqual,
    GreaterThan,
    GreaterEqual,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InstructionKind {
    Constant(Scalar),
    Unary {
        op: UnaryOp,
        operand: ValueId,
    },
    Binary {
        op: BinaryOp,
        left: ValueId,
        right: ValueId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Instruction {
    pub result: ValueId,
    pub kind: InstructionKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parameter {
    pub value: ValueId,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub target: BlockId,
    pub arguments: Vec<ValueId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminatorKind {
    Jump(Edge),
    Branch {
        condition: ValueId,
        then_edge: Edge,
        else_edge: Edge,
    },
    Return(ValueId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terminator {
    pub kind: TerminatorKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub id: BlockId,
    pub params: Vec<Parameter>,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub source_len: u32,
    pub entry: BlockId,
    pub return_type: Type,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationErrorKind {
    Capacity,
    InvalidEntry,
    DuplicateBlock,
    DuplicateValue,
    UndefinedValue,
    InvalidTarget,
    ArgumentArity,
    TypeMismatch,
    InvalidSpan,
    UseBeforeDefinition,
    DoesNotDominate,
    UnreachableBlock,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    pub kind: ValidationErrorKind,
    pub block: BlockId,
    pub value: Option<ValueId>,
    pub span: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionError {
    InvalidFunction(Vec<ValidationError>),
    ArgumentArity {
        expected: usize,
        actual: usize,
    },
    ArgumentType {
        index: usize,
        expected: Type,
        actual: Type,
    },
    StepLimit {
        limit: usize,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FoldReport {
    pub folded_instructions: usize,
}

impl InstructionKind {
    /// Result types are fixed by the operation; validation checks operand types.
    pub fn result_type(&self) -> Type {
        match self {
            Self::Constant(value) => value.ty(),
            Self::Unary {
                op: UnaryOp::Negate,
                ..
            } => Type::Number,
            Self::Unary {
                op: UnaryOp::Not, ..
            } => Type::Bool,
            Self::Binary { op, .. } => match op {
                BinaryOp::Add
                | BinaryOp::Subtract
                | BinaryOp::Multiply
                | BinaryOp::Divide
                | BinaryOp::Remainder => Type::Number,
                _ => Type::Bool,
            },
        }
    }
}

impl TerminatorKind {
    pub(crate) fn edges(&self) -> Vec<&Edge> {
        match self {
            Self::Jump(edge) => vec![edge],
            Self::Branch {
                then_edge,
                else_edge,
                ..
            } => vec![then_edge, else_edge],
            Self::Return(_) => vec![],
        }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?} in block {}", self.kind, self.block.0)?;
        if let Some(value) = self.value {
            write!(formatter, " at value {}", value.0)?;
        }
        if let Some(span) = self.span {
            write!(formatter, " (source {}..{})", span.start, span.end)?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

impl std::fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFunction(errors) => write!(
                formatter,
                "invalid function: {} validation error(s)",
                errors.len()
            ),
            Self::ArgumentArity { expected, actual } => write!(
                formatter,
                "expected {expected} entry arguments, got {actual}"
            ),
            Self::ArgumentType {
                index,
                expected,
                actual,
            } => write!(
                formatter,
                "entry argument {index}: expected {expected:?}, got {actual:?}"
            ),
            Self::StepLimit { limit } => {
                write!(formatter, "execution exceeded the {limit}-step limit")
            }
        }
    }
}

impl std::error::Error for ExecutionError {}

/// Owned general JavaScript source semantics with original UTF-16 positions.
#[path = "javascript_source_ir.rs"]
pub mod js;
