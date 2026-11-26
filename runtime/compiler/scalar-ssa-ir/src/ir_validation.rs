//! Validation of scalar SSA functions: definitions before uses, dominance, spans and
//! tighten-only work limits.

use std::collections::{BTreeMap, BTreeSet};

use crate::*;

/// Tighten-only admission and work limits for scalar SSA validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValidationLimits {
    /// Function, blocks, parameters, instructions, terminators, edges and arguments.
    pub max_nodes: usize,
    /// Reachable block count squared, before dominator set construction.
    pub max_state_cells: usize,
    /// Structural visits and dominator scan, clone and comparison entries.
    pub max_work: usize,
}
impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_nodes: 1_000_000,
            max_state_cells: 8_000_000,
            max_work: 64_000_000,
        }
    }
}
impl ValidationLimits {
    pub fn validate(&self) -> Result<(), &'static str> {
        let defaults = Self::default();
        if self.max_nodes > defaults.max_nodes
            || self.max_state_cells > defaults.max_state_cells
            || self.max_work > defaults.max_work
        {
            Err("scalar validation limits may only tighten defaults")
        } else {
            Ok(())
        }
    }
}

fn capacity(function: &Function) -> Vec<ValidationError> {
    vec![ValidationError {
        kind: ValidationErrorKind::Capacity,
        block: function.entry,
        value: None,
        span: None,
    }]
}
struct Work<'a> {
    remaining: usize,
    function: &'a Function,
}
impl Work<'_> {
    fn charge(&mut self, amount: usize) -> Result<(), Vec<ValidationError>> {
        self.remaining = self
            .remaining
            .checked_sub(amount)
            .ok_or_else(|| capacity(self.function))?;
        Ok(())
    }
}
// Count caller-owned structure before building identifier maps or CFG state.
// Nodes include the function, blocks, parameters, instructions, terminators,
// edges and edge arguments. Fixed-width instruction operands need no allocation.
fn admission<'a>(
    function: &'a Function,
    limits: &ValidationLimits,
) -> Result<Work<'a>, Vec<ValidationError>> {
    limits.validate().map_err(|_| capacity(function))?;
    let mut work = Work {
        remaining: limits.max_work,
        function,
    };
    let mut nodes = 0usize;
    let mut add = |n: usize| -> Result<(), Vec<ValidationError>> {
        nodes = nodes
            .checked_add(n)
            .filter(|n| *n <= limits.max_nodes)
            .ok_or_else(|| capacity(function))?;
        work.charge(n)
    };
    add(1)?;
    for block in &function.blocks {
        add(2)?;
        add(block.params.len())?;
        add(block.instructions.len())?;
        for edge in block.terminator.kind.edges() {
            add(1)?;
            add(edge.arguments.len())?;
        }
    }
    Ok(work)
}

#[derive(Clone, Copy)]
struct Definition {
    block: BlockId,
    instruction: Option<usize>,
    ty: Type,
}

struct UseSite {
    block: BlockId,
    instruction: usize,
    span: Span,
}

fn error(
    errors: &mut Vec<ValidationError>,
    kind: ValidationErrorKind,
    block: BlockId,
    value: Option<ValueId>,
    span: Option<Span>,
) {
    errors.push(ValidationError {
        kind,
        block,
        value,
        span,
    });
}

fn check_span(function: &Function, block: BlockId, span: Span, errors: &mut Vec<ValidationError>) {
    if span.start > span.end || span.end > function.source_len {
        error(
            errors,
            ValidationErrorKind::InvalidSpan,
            block,
            None,
            Some(span),
        );
    }
}

fn define(
    definitions: &mut BTreeMap<ValueId, Definition>,
    value: ValueId,
    definition: Definition,
    span: Span,
    errors: &mut Vec<ValidationError>,
) {
    if let std::collections::btree_map::Entry::Vacant(entry) = definitions.entry(value) {
        entry.insert(definition);
    } else {
        error(
            errors,
            ValidationErrorKind::DuplicateValue,
            definition.block,
            Some(value),
            Some(span),
        );
    }
}

fn check_use(
    value: ValueId,
    expected: Option<Type>,
    site: &UseSite,
    definitions: &BTreeMap<ValueId, Definition>,
    dominators: &BTreeMap<BlockId, BTreeSet<BlockId>>,
    errors: &mut Vec<ValidationError>,
) {
    let Some(definition) = definitions.get(&value) else {
        error(
            errors,
            ValidationErrorKind::UndefinedValue,
            site.block,
            Some(value),
            Some(site.span),
        );
        return;
    };
    if expected.is_some_and(|ty| ty != definition.ty) {
        error(
            errors,
            ValidationErrorKind::TypeMismatch,
            site.block,
            Some(value),
            Some(site.span),
        );
    }
    if definition.block == site.block {
        if definition
            .instruction
            .is_some_and(|index| index >= site.instruction)
        {
            error(
                errors,
                ValidationErrorKind::UseBeforeDefinition,
                site.block,
                Some(value),
                Some(site.span),
            );
        }
    } else if let Some(block_dominators) = dominators.get(&site.block) {
        if !block_dominators.contains(&definition.block) {
            error(
                errors,
                ValidationErrorKind::DoesNotDominate,
                site.block,
                Some(value),
                Some(site.span),
            );
        }
    }
}

fn compute_dominators(
    function: &Function,
    blocks: &BTreeMap<BlockId, usize>,
    errors: &mut Vec<ValidationError>,
    limits: &ValidationLimits,
    work: &mut Work<'_>,
) -> Result<BTreeMap<BlockId, BTreeSet<BlockId>>, Vec<ValidationError>> {
    let mut reachable = BTreeSet::new();
    work.charge(1)?;
    let mut pending = vec![function.entry];
    while let Some(id) = pending.pop() {
        work.charge(1)?;
        if !reachable.insert(id) {
            continue;
        }
        for edge in function.blocks[blocks[&id]].terminator.kind.edges() {
            work.charge(1)?;
            if blocks.contains_key(&edge.target) {
                pending.push(edge.target);
            }
        }
    }
    // Worst-case dominator membership is quadratic. Reject before any such
    // sets are cloned, independently of identifier density or storage order.
    let cells = reachable
        .len()
        .checked_mul(reachable.len())
        .ok_or_else(|| capacity(function))?;
    if cells > limits.max_state_cells {
        return Err(capacity(function));
    }
    for block in &function.blocks {
        work.charge(1)?;
        if !reachable.contains(&block.id) {
            error(
                errors,
                ValidationErrorKind::UnreachableBlock,
                block.id,
                None,
                Some(block.span),
            );
        }
    }
    work.charge(reachable.len())?;
    let mut predecessors: BTreeMap<BlockId, BTreeSet<BlockId>> =
        reachable.iter().map(|id| (*id, BTreeSet::new())).collect();
    for block in &function.blocks {
        work.charge(1)?;
        if !reachable.contains(&block.id) {
            continue;
        }
        for edge in block.terminator.kind.edges() {
            work.charge(1)?;
            if let Some(incoming) = predecessors.get_mut(&edge.target) {
                incoming.insert(block.id);
            }
        }
    }
    let mut dominators = BTreeMap::new();
    for id in &reachable {
        // Charge clone/collection entries before allocating them.
        work.charge(if *id == function.entry {
            1
        } else {
            reachable.len()
        })?;
        dominators.insert(
            *id,
            if *id == function.entry {
                BTreeSet::from([*id])
            } else {
                reachable.clone()
            },
        );
    }
    loop {
        let mut changed = false;
        for id in &reachable {
            work.charge(1)?;
            if *id == function.entry {
                continue;
            }
            work.charge(reachable.len())?;
            let mut intersection = reachable.clone();
            for predecessor in &predecessors[id] {
                work.charge(1)?;
                // retain scans each surviving entry; debit the entire scan
                // before it starts so exhaustion cannot continue the traversal.
                work.charge(intersection.len())?;
                intersection.retain(|candidate| dominators[predecessor].contains(candidate));
            }
            work.charge(1)?;
            intersection.insert(*id);
            work.charge(if dominators[id].len() == intersection.len() {
                intersection.len()
            } else {
                1
            })?;
            if dominators[id] != intersection {
                dominators.insert(*id, intersection);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    Ok(dominators)
}

/// Check structure, source ranges, fixed scalar types and SSA dominance.
///
/// IDs need not be dense or follow storage order. All blocks must be reachable
/// from `entry`; parameters define values at block entry. Edge operands are uses
/// at the predecessor's terminator, before simultaneous parameter assignment.
pub fn validate(function: &Function) -> Result<(), Vec<ValidationError>> {
    validate_with_limits(function, &ValidationLimits::default())
}

pub fn validate_with_limits(
    function: &Function,
    limits: &ValidationLimits,
) -> Result<(), Vec<ValidationError>> {
    let mut work = admission(function, limits)?;
    let mut errors = Vec::new();
    let mut blocks = BTreeMap::new();
    let mut definitions = BTreeMap::new();
    for (index, block) in function.blocks.iter().enumerate() {
        work.charge(2)?;
        if blocks.insert(block.id, index).is_some() {
            error(
                &mut errors,
                ValidationErrorKind::DuplicateBlock,
                block.id,
                None,
                Some(block.span),
            );
        }
        check_span(function, block.id, block.span, &mut errors);
        for parameter in &block.params {
            work.charge(1)?;
            check_span(function, block.id, parameter.span, &mut errors);
            define(
                &mut definitions,
                parameter.value,
                Definition {
                    block: block.id,
                    instruction: None,
                    ty: parameter.ty,
                },
                parameter.span,
                &mut errors,
            );
        }
        for (index, instruction) in block.instructions.iter().enumerate() {
            work.charge(1)?;
            check_span(function, block.id, instruction.span, &mut errors);
            define(
                &mut definitions,
                instruction.result,
                Definition {
                    block: block.id,
                    instruction: Some(index),
                    ty: instruction.kind.result_type(),
                },
                instruction.span,
                &mut errors,
            );
        }
        check_span(function, block.id, block.terminator.span, &mut errors);
    }
    if !blocks.contains_key(&function.entry) {
        error(
            &mut errors,
            ValidationErrorKind::InvalidEntry,
            function.entry,
            None,
            None,
        );
    }
    // Duplicate identifiers make subsequent definition and CFG analysis ambiguous.
    work.charge(errors.len())?;
    if errors.iter().any(|e| {
        matches!(
            e.kind,
            ValidationErrorKind::InvalidEntry
                | ValidationErrorKind::DuplicateBlock
                | ValidationErrorKind::DuplicateValue
        )
    }) {
        return Err(errors);
    }
    for block in &function.blocks {
        work.charge(1)?;
        for edge in block.terminator.kind.edges() {
            work.charge(1)?;
            if !blocks.contains_key(&edge.target) {
                error(
                    &mut errors,
                    ValidationErrorKind::InvalidTarget,
                    block.id,
                    None,
                    Some(block.terminator.span),
                );
            }
        }
    }
    let dominators = compute_dominators(function, &blocks, &mut errors, limits, &mut work)?;
    for block in &function.blocks {
        work.charge(1)?;
        for (index, instruction) in block.instructions.iter().enumerate() {
            work.charge(1)?;
            let site = UseSite {
                block: block.id,
                instruction: index,
                span: instruction.span,
            };
            match instruction.kind {
                InstructionKind::Constant(_) => {}
                InstructionKind::Unary { op, operand } => {
                    let expected = match op {
                        UnaryOp::Negate => Type::Number,
                        UnaryOp::Not => Type::Bool,
                    };
                    check_use(
                        operand,
                        Some(expected),
                        &site,
                        &definitions,
                        &dominators,
                        &mut errors,
                    );
                }
                InstructionKind::Binary { op, left, right } => {
                    let expected = if op == BinaryOp::Equal {
                        None
                    } else {
                        Some(Type::Number)
                    };
                    check_use(
                        left,
                        expected,
                        &site,
                        &definitions,
                        &dominators,
                        &mut errors,
                    );
                    let right_type =
                        expected.or_else(|| definitions.get(&left).map(|definition| definition.ty));
                    check_use(
                        right,
                        right_type,
                        &site,
                        &definitions,
                        &dominators,
                        &mut errors,
                    );
                }
            }
        }
        let site = UseSite {
            block: block.id,
            instruction: block.instructions.len(),
            span: block.terminator.span,
        };
        match block.terminator.kind {
            TerminatorKind::Branch { condition, .. } => check_use(
                condition,
                Some(Type::Bool),
                &site,
                &definitions,
                &dominators,
                &mut errors,
            ),
            TerminatorKind::Return(value) => check_use(
                value,
                Some(function.return_type),
                &site,
                &definitions,
                &dominators,
                &mut errors,
            ),
            TerminatorKind::Jump(_) => {}
        }
        for edge in block.terminator.kind.edges() {
            work.charge(1)?;
            let parameters = blocks
                .get(&edge.target)
                .map(|index| &function.blocks[*index].params);
            if parameters.is_some_and(|params| params.len() != edge.arguments.len()) {
                error(
                    &mut errors,
                    ValidationErrorKind::ArgumentArity,
                    block.id,
                    None,
                    Some(block.terminator.span),
                );
            }
            for (index, argument) in edge.arguments.iter().enumerate() {
                work.charge(1)?;
                let expected =
                    parameters.and_then(|params| params.get(index).map(|parameter| parameter.ty));
                check_use(
                    *argument,
                    expected,
                    &site,
                    &definitions,
                    &dominators,
                    &mut errors,
                );
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
