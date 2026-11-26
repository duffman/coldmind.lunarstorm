//! Tighten-only compiler admission limits. These do not replace image verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmissionLimits {
    pub max_ir_nodes: usize,
    pub max_nesting: usize,
    pub max_functions: usize,
    /// Per-function argument/local slots, including compiler temporaries.
    pub max_locals: usize,
    pub max_atoms: usize,
    pub max_atom_units: usize,
    pub max_constants: usize,
    pub max_instructions: usize,
    pub max_code_bytes: usize,
    /// Logical instruction locations (each has a native and protected-span row).
    pub max_positions: usize,
    pub max_source_units: usize,
    pub max_work: usize,
}
impl Default for EmissionLimits {
    fn default() -> Self {
        Self {
            // Program-wide caps sized for real bundles (multi-megabyte
            // sources); per-function native tables stay at 16-bit bounds.
            max_ir_nodes: 32_000_000,
            max_nesting: 256,
            max_functions: 1_000_000,
            max_locals: 65_534,
            max_atoms: 65_535,
            max_atom_units: 64 * 1024 * 1024,
            max_constants: 65_535,
            max_instructions: 32_000_000,
            max_code_bytes: 256 * 1024 * 1024,
            max_positions: 32_000_000,
            max_source_units: 256 * 1024 * 1024,
            max_work: 2_000_000_000,
        }
    }
}
impl EmissionLimits {
    pub fn validate(&self) -> Result<(), String> {
        let hard = Self::default();
        macro_rules! bound {($($field:ident),*)=>{$(if self.$field>hard.$field{return Err(concat!("capacity: limits cannot increase ",stringify!($field)).into())})*}}
        bound!(
            max_ir_nodes,
            max_nesting,
            max_functions,
            max_locals,
            max_atoms,
            max_atom_units,
            max_constants,
            max_instructions,
            max_code_bytes,
            max_positions,
            max_source_units,
            max_work
        );
        Ok(())
    }
}

use std::{cell::RefCell, rc::Rc};
#[derive(Clone, Copy)]
pub(crate) enum Counter {
    Functions,
    Atoms,
    AtomUnits,
    Constants,
    Instructions,
    Code,
    Positions,
    Work,
}
#[derive(Clone)]
pub(crate) struct Budget {
    pub limits: EmissionLimits,
    state: Rc<RefCell<State>>,
}
#[derive(Default)]
struct State {
    used: [usize; 8],
    error: Option<String>,
}
impl Budget {
    pub fn new(limits: EmissionLimits) -> Result<Self, String> {
        limits.validate()?;
        Ok(Self {
            limits,
            state: Rc::new(RefCell::new(State::default())),
        })
    }
    pub fn charge(&self, c: Counter, n: usize) -> Result<(), String> {
        self.check()?;
        let (index, max, name) = match c {
            Counter::Functions => (0, self.limits.max_functions, "functions"),
            Counter::Atoms => (1, self.limits.max_atoms, "atoms"),
            Counter::AtomUnits => (2, self.limits.max_atom_units, "atom units"),
            Counter::Constants => (3, self.limits.max_constants, "constants"),
            Counter::Instructions => (4, self.limits.max_instructions, "instructions"),
            Counter::Code => (5, self.limits.max_code_bytes, "code bytes"),
            Counter::Positions => (6, self.limits.max_positions, "positions"),
            Counter::Work => (7, self.limits.max_work, "work"),
        };
        let mut state = self.state.borrow_mut();
        let next = state.used[index].checked_add(n);
        if next.is_none_or(|v| v > max) {
            let e = format!("capacity: emission {name} budget exceeded");
            state.error = Some(e.clone());
            return Err(e);
        }
        state.used[index] = next.unwrap();
        Ok(())
    }
    pub fn check(&self) -> Result<(), String> {
        self.state.borrow().error.clone().map_or(Ok(()), Err)
    }
    pub fn slots(&self, n: usize) -> Result<(), String> {
        if n > self.limits.max_locals {
            return Err("capacity: emission locals budget exceeded".into());
        }
        Ok(())
    }
    pub fn text(&self, units: usize) -> Result<(), String> {
        self.charge(Counter::AtomUnits, units)
    }
}
/// Iterative admission of caller-owned IR before recursive emission and cloning.
pub(crate) fn program(p: &qjs_scalar_ssa_ir::js::Program, l: &EmissionLimits) -> Result<(), String> {
    use qjs_scalar_ssa_ir::js::{
        CallArgument, ClassElement, ClassKey, ExpressionKind as E, Literal, StatementKind as S,
    };
    l.validate()?;
    if l.max_functions == 0 || l.max_ir_nodes == 0 || l.max_locals == 0 {
        return Err("capacity: script entry representation budget exceeded".into());
    }
    if p.source.len().saturating_add(p.scopes.len()) > l.max_work {
        return Err("capacity: emission work budget exceeded".into());
    }
    if p.source.len() > l.max_source_units {
        return Err("capacity: emission source units budget exceeded".into());
    }
    if p.scopes
        .iter()
        .any(|s| s.id > 65533 || s.parent.is_some_and(|p| p > 65533))
    {
        return Err("capacity: emission scope index budget exceeded".into());
    }
    if p.scopes.len() > l.max_ir_nodes {
        return Err("capacity: emission scope table budget exceeded".into());
    }
    if let Some(m) = &p.module
        && m.requests
            .len()
            .saturating_add(m.imports.len())
            .saturating_add(m.exports.len())
            > l.max_ir_nodes
    {
        return Err("capacity: emission module tables budget exceeded".into());
    }
    enum Node<'a> {
        Statement(&'a qjs_scalar_ssa_ir::js::Statement),
        Expression(&'a qjs_scalar_ssa_ir::js::Expression),
        Function(&'a qjs_scalar_ssa_ir::js::Function),
        Pattern(&'a qjs_scalar_ssa_ir::js::Pattern),
        Class(&'a qjs_scalar_ssa_ir::js::Class),
        Key(&'a qjs_scalar_ssa_ir::js::ObjectKey),
        Argument(&'a CallArgument),
        Binding(&'a qjs_scalar_ssa_ir::js::Binding),
        Reference(&'a qjs_scalar_ssa_ir::js::Reference),
    }
    let mut pending = Vec::new();
    let mut nodes = p
        .scopes
        .len()
        .checked_add(1)
        .ok_or("capacity: scope node overflow")?;
    let mut functions = 1usize;
    let mut constants = 0usize;
    let mut work = p
        .source
        .len()
        .checked_add(p.scopes.len())
        .ok_or("capacity: scope work overflow")?;
    macro_rules! charge_work {
        ($amount:expr) => {{
            work = work
                .checked_add($amount)
                .ok_or("capacity: emission work overflow")?;
            if work > l.max_work {
                return Err("capacity: emission work budget exceeded".into());
            }
        }};
    }
    macro_rules! charge_nodes {
        ($amount:expr) => {{
            nodes = nodes
                .checked_add($amount)
                .ok_or("capacity: emission IR nodes overflow")?;
            if nodes
                .checked_add(pending.len())
                .is_none_or(|n| n > l.max_ir_nodes)
            {
                return Err("capacity: emission IR nodes budget exceeded".into());
            }
        }};
    }
    macro_rules! atom_text {
        ($text:expr) => {{
            let text: &str = $text;
            charge_work!(text.len());
            if text.len() > l.max_atom_units.saturating_mul(3)
                || text.encode_utf16().count() > l.max_atom_units
            {
                return Err("capacity: emission atom units budget exceeded".into());
            }
        }};
    }
    macro_rules! atom_units {
        ($text:expr) => {{
            let text: &[u16] = $text;
            charge_work!(text.len());
            if text.len() > l.max_atom_units {
                return Err("capacity: emission atom units budget exceeded".into());
            }
        }};
    }
    macro_rules! push {
        ($node:expr,$depth:expr) => {{
            if pending
                .len()
                .checked_add(nodes)
                .is_none_or(|n| n >= l.max_ir_nodes)
            {
                return Err("capacity: emission IR nodes budget exceeded".into());
            }
            charge_work!(1usize);
            pending.push(($node, $depth));
        }};
    }
    macro_rules! add_functions {
        ($amount:expr) => {{
            let amount = $amount;
            functions = functions
                .checked_add(amount)
                .ok_or("capacity: emission functions overflow")?;
            constants = constants
                .checked_add(amount)
                .ok_or("capacity: emission constants overflow")?;
            if functions > l.max_functions {
                return Err("capacity: emission functions budget exceeded".into());
            }
            if constants > l.max_constants {
                return Err("capacity: emission constants budget exceeded".into());
            }
        }};
    }
    macro_rules! push_function {
        ($function:expr,$depth:expr) => {{
            add_functions!(1usize);
            push!(Node::Function($function), $depth);
        }};
    }
    charge_nodes!(0usize);
    atom_text!(&p.filename);
    if let Some(binding) = &p.this_binding {
        push!(Node::Binding(binding), 1usize);
    }
    if let Some(module) = &p.module {
        let entries = module
            .requests
            .len()
            .checked_add(module.imports.len())
            .and_then(|n| n.checked_add(module.exports.len()))
            .ok_or("capacity: module table overflow")?;
        charge_nodes!(entries);
        charge_work!(entries);
        for request in &module.requests {
            atom_units!(request);
        }
        for import in &module.imports {
            atom_units!(&import.name);
            push!(Node::Binding(&import.binding), 1usize);
        }
        for export in &module.exports {
            match export {
                qjs_scalar_ssa_ir::js::Export::Local { name, .. } => atom_units!(name),
                qjs_scalar_ssa_ir::js::Export::Indirect { import, name, .. } => {
                    atom_units!(import);
                    atom_units!(name);
                }
                qjs_scalar_ssa_ir::js::Export::Star { .. } => {}
            }
        }
    }
    for s in p.body.iter().rev() {
        push!(Node::Statement(s), 1usize);
    }
    while let Some((node, depth)) = pending.pop() {
        if depth > l.max_nesting {
            return Err("capacity: emission nesting budget exceeded".into());
        }
        nodes += 1;
        if nodes > l.max_ir_nodes {
            return Err("capacity: emission IR nodes budget exceeded".into());
        }
        work = work
            .checked_add(1)
            .ok_or("capacity: emission work overflow")?;

        if work > l.max_work {
            return Err("capacity: emission work budget exceeded".into());
        }
        match node {
            Node::Function(f) => {
                if f.span.start > f.span.end || f.span.end as usize > p.source.len() {
                    return Err("function source span outside original source".into());
                }
                if f.parameters
                    .len()
                    .saturating_add(usize::from(f.rest_parameter.is_some()))
                    > l.max_locals
                {
                    return Err("capacity: emission locals budget exceeded".into());
                }
                atom_text!(f.name.as_deref().unwrap_or(""));
                if f.scope > 65533 {
                    return Err("capacity: function scope representation budget exceeded".into());
                }
                for binding in [
                    &f.self_binding,
                    &f.arguments_binding,
                    &f.this_binding,
                    &f.new_target_binding,
                    &f.home_object_binding,
                    &f.super_constructor_binding,
                    &f.field_initializer_binding,
                    &f.initializer_name_binding,
                ]
                .into_iter()
                .flatten()
                {
                    push!(Node::Binding(binding), depth + 1);
                }
                charge_nodes!(f.body_parameter_copies.len());
                charge_work!(f.body_parameter_copies.len());
                for copy in &f.body_parameter_copies {
                    push!(Node::Binding(&copy.binding), depth + 1);
                    push!(Node::Reference(&copy.parameter), depth + 1);
                }
                for pattern in f.parameters.iter().chain(f.rest_parameter.iter()) {
                    push!(Node::Pattern(pattern), depth + 1)
                }
                for s in f.body.iter().rev() {
                    push!(Node::Statement(s), depth + 1)
                }
            }
            Node::Binding(binding) => {
                atom_text!(&binding.name);
                if binding.scope > 65533 {
                    return Err("capacity: binding scope representation budget exceeded".into());
                }
            }
            Node::Reference(reference) => {
                atom_text!(&reference.name);
            }
            Node::Argument(argument) => match argument {
                CallArgument::Value(expression) | CallArgument::Spread(expression) => {
                    // The wrapper is data, not another recursive expression layer.
                    push!(Node::Expression(expression), depth);
                }
            },
            Node::Key(key) => match key {
                qjs_scalar_ssa_ir::js::ObjectKey::Static(name) => {
                    atom_units!(name);
                }
                qjs_scalar_ssa_ir::js::ObjectKey::Computed(expression) => {
                    push!(Node::Expression(expression), depth + 1)
                }
            },
            Node::Class(class) => {
                if class.span.start > class.span.end || class.span.end as usize > p.source.len() {
                    return Err("class source span outside original source".into());
                }
                if class.scope > 65533 {
                    return Err("capacity: class scope representation budget exceeded".into());
                }
                if let Some(name) = &class.name {
                    atom_text!(name);
                }
                let entries = class
                    .elements
                    .len()
                    .checked_add(class.private_names.len())
                    .ok_or("capacity: class table size overflow")?;
                charge_nodes!(entries);
                charge_work!(entries);
                // Reserve all directly owned function images before enqueueing
                // elements or creating a backend private-name/field plan.
                let mut explicit = usize::from(class.constructor.is_some());
                let mut instance_work = false;
                for element in &class.elements {
                    match element {
                        ClassElement::Method { key, is_static, .. } => {
                            explicit = explicit
                                .checked_add(1)
                                .ok_or("capacity: class functions overflow")?;
                            instance_work |= !is_static && matches!(key, ClassKey::Private(_));
                        }
                        ClassElement::Field {
                            initializer,
                            is_static,
                            ..
                        } => {
                            explicit = explicit
                                .checked_add(usize::from(initializer.is_some()))
                                .ok_or("capacity: class functions overflow")?;
                            instance_work |= !is_static;
                        }
                        ClassElement::StaticBlock(_) => {
                            explicit = explicit
                                .checked_add(1)
                                .ok_or("capacity: class functions overflow")?;
                        }
                    }
                }
                let implicit =
                    usize::from(class.constructor.is_none()) + usize::from(instance_work);
                let direct = explicit
                    .checked_add(implicit)
                    .ok_or("capacity: class functions overflow")?;
                if functions
                    .checked_add(direct)
                    .is_none_or(|n| n > l.max_functions)
                    || constants
                        .checked_add(direct)
                        .is_none_or(|n| n > l.max_constants)
                {
                    return Err("capacity: class function representation budget exceeded".into());
                }
                add_functions!(implicit);
                push!(
                    Node::Binding(&class.instance_initializer_binding),
                    depth + 1
                );
                if let Some(binding) = &class.self_binding {
                    push!(Node::Binding(binding), depth + 1);
                }
                if let Some(heritage) = &class.heritage {
                    push!(Node::Expression(heritage), depth + 1);
                }
                if let Some(constructor) = &class.constructor {
                    push_function!(constructor, depth + 1);
                }
                for name in &class.private_names {
                    atom_units!(&name.name);
                    // A displayed private name includes its leading '#'.
                    if name
                        .name
                        .len()
                        .checked_add(1)
                        .is_none_or(|n| n > l.max_atom_units)
                    {
                        return Err("capacity: private name atom units budget exceeded".into());
                    }
                }
                for element in class.elements.iter().rev() {
                    match element {
                        ClassElement::Method { key, function, .. } => {
                            if let ClassKey::Public(key) = key {
                                push!(Node::Key(key), depth + 1);
                            }
                            push_function!(function, depth + 1);
                        }
                        ClassElement::Field {
                            key, initializer, ..
                        } => {
                            if let ClassKey::Public(key) = key {
                                push!(Node::Key(key), depth + 1);
                            }
                            if let Some(initializer) = initializer {
                                push_function!(initializer, depth + 1);
                            }
                        }
                        ClassElement::StaticBlock(function) => push_function!(function, depth + 1),
                    }
                }
            }
            Node::Statement(s) => match &s.kind {
                S::Class { binding, class } => {
                    push!(Node::Binding(binding), depth + 1);
                    push!(Node::Class(class), depth + 1);
                }
                S::Empty | S::Debugger | S::Break | S::Continue => {}
                S::BreakLabel(label) | S::ContinueLabel(label) => {
                    charge_work!(label.len());
                }
                S::Expression(e) => push!(Node::Expression(e), depth + 1),
                S::Declaration(ds) => {
                    nodes = nodes
                        .checked_add(ds.len())
                        .ok_or("capacity: IR nodes overflow")?;
                    if ds.len() > l.max_ir_nodes {
                        return Err("capacity: emission declarations budget exceeded".into());
                    }
                    for declaration in ds {
                        push!(Node::Pattern(&declaration.pattern), depth + 1);
                        if let Some(initializer) = &declaration.initializer {
                            push!(Node::Expression(initializer), depth + 1)
                        }
                    }
                }
                S::ForOf {
                    left, right, body, ..
                }
                | S::ForIn {
                    left, right, body, ..
                } => {
                    nodes = nodes
                        .checked_add(1)
                        .ok_or("capacity: iteration target nodes overflow")?;
                    match left {
                        qjs_scalar_ssa_ir::js::ForTarget::Declaration(declaration) => {
                            push!(Node::Pattern(&declaration.pattern), depth + 1);
                            if let Some(initializer) = &declaration.initializer {
                                push!(Node::Expression(initializer), depth + 1)
                            }
                        }
                        qjs_scalar_ssa_ir::js::ForTarget::Assignment(pattern) => {
                            push!(Node::Pattern(pattern), depth + 1)
                        }
                    }
                    push!(Node::Expression(right), depth + 1);
                    push!(Node::Statement(body), depth + 1)
                }
                S::Label { label, body } => {
                    work = work
                        .checked_add(label.len())
                        .ok_or("capacity: label work overflow")?;
                    push!(Node::Statement(body), depth + 1)
                }
                S::Switch {
                    discriminant,
                    cases,
                    ..
                } => {
                    work = work
                        .checked_add(cases.len())
                        .ok_or("capacity: switch work overflow")?;
                    nodes = nodes
                        .checked_add(cases.len())
                        .ok_or("capacity: switch IR nodes overflow")?;
                    if work > l.max_work || nodes > l.max_ir_nodes {
                        return Err("capacity: switch admission budget exceeded".into());
                    }
                    push!(Node::Expression(discriminant), depth + 1);
                    for case in cases.iter().rev() {
                        if let Some(test) = &case.test {
                            push!(Node::Expression(test), depth + 1)
                        }
                        for statement in case.consequent.iter().rev() {
                            push!(Node::Statement(statement), depth + 1)
                        }
                    }
                }
                S::TryFinally {
                    body, finalizer, ..
                } => {
                    for statement in body.iter().chain(finalizer.iter()).rev() {
                        push!(Node::Statement(statement), depth + 1)
                    }
                }
                S::Throw(value) => push!(Node::Expression(value), depth + 1),
                S::TryCatch {
                    body,
                    binding,
                    handler,
                    ..
                } => {
                    if let Some(pattern) = binding {
                        push!(Node::Pattern(pattern), depth + 1)
                    }
                    for statement in body.iter().chain(handler.iter()).rev() {
                        push!(Node::Statement(statement), depth + 1)
                    }
                }
                S::Block(body) => {
                    for s in body.iter().rev() {
                        push!(Node::Statement(s), depth + 1)
                    }
                }
                S::Function { binding, function } => {
                    push!(Node::Binding(binding), depth + 1);
                    push_function!(function, depth + 1);
                }
                S::Return(e) => {
                    if let Some(e) = e {
                        push!(Node::Expression(e), depth + 1)
                    }
                }
                S::With { object, body, .. } => {
                    push!(Node::Expression(object), depth + 1);
                    push!(Node::Statement(body), depth + 1);
                }
                S::If {
                    test,
                    consequent,
                    alternate,
                } => {
                    push!(Node::Expression(test), depth + 1);
                    push!(Node::Statement(consequent), depth + 1);
                    if let Some(s) = alternate {
                        push!(Node::Statement(s), depth + 1)
                    }
                }
                S::While { test, body } | S::DoWhile { test, body } => {
                    push!(Node::Expression(test), depth + 1);
                    push!(Node::Statement(body), depth + 1)
                }
                S::For {
                    init,
                    test,
                    update,
                    body,
                } => {
                    if let Some(s) = init {
                        push!(Node::Statement(s), depth + 1)
                    }
                    if let Some(e) = test {
                        push!(Node::Expression(e), depth + 1)
                    }
                    if let Some(e) = update {
                        push!(Node::Expression(e), depth + 1)
                    }
                    push!(Node::Statement(body), depth + 1)
                }
            },
            Node::Pattern(pattern) => match pattern {
                qjs_scalar_ssa_ir::js::Pattern::Binding {
                    binding,
                    initialization_target,
                } => {
                    push!(Node::Binding(binding), depth);
                    if let Some(target) = initialization_target {
                        push!(Node::Reference(target), depth);
                    }
                }
                qjs_scalar_ssa_ir::js::Pattern::Target(reference) => {
                    push!(Node::Reference(reference), depth);
                }
                qjs_scalar_ssa_ir::js::Pattern::SuperMember { key } => {
                    push!(Node::Expression(key), depth + 1);
                }
                qjs_scalar_ssa_ir::js::Pattern::PrivateMember { object, .. } => {
                    push!(Node::Expression(object), depth + 1);
                }
                qjs_scalar_ssa_ir::js::Pattern::Member { object, key } => {
                    push!(Node::Expression(object), depth + 1);
                    push!(Node::Expression(key), depth + 1)
                }
                qjs_scalar_ssa_ir::js::Pattern::Default { pattern, value } => {
                    push!(Node::Pattern(pattern), depth + 1);
                    push!(Node::Expression(value), depth + 1)
                }
                qjs_scalar_ssa_ir::js::Pattern::Array { elements, rest } => {
                    work = work
                        .checked_add(elements.len())
                        .ok_or("capacity: array pattern work overflow")?;
                    if work > l.max_work || elements.len() > l.max_ir_nodes {
                        return Err("capacity: array pattern admission budget exceeded".into());
                    }
                    for pattern in elements
                        .iter()
                        .flatten()
                        .chain(rest.iter().map(|rest| &**rest))
                    {
                        push!(Node::Pattern(pattern), depth + 1)
                    }
                }
                qjs_scalar_ssa_ir::js::Pattern::Object { properties, rest } => {
                    work = work
                        .checked_add(properties.len())
                        .ok_or("capacity: object pattern work overflow")?;
                    nodes = nodes
                        .checked_add(properties.len())
                        .ok_or("capacity: object pattern nodes overflow")?;
                    if work > l.max_work || nodes > l.max_ir_nodes {
                        return Err("capacity: object pattern admission budget exceeded".into());
                    }
                    for property in properties {
                        push!(Node::Key(&property.key), depth + 1);
                        push!(Node::Pattern(&property.pattern), depth + 1)
                    }
                    if let Some(rest) = rest {
                        push!(Node::Pattern(rest), depth + 1)
                    }
                }
            },
            Node::Expression(e) => match &e.kind {
                E::NamedEvaluation { name, value } => {
                    push!(Node::Expression(name), depth + 1);
                    push!(Node::Expression(value), depth + 1);
                }
                E::Class(class) => {
                    push!(Node::Class(class), depth + 1);
                }
                E::SuperMember { key } | E::UpdateSuper { key, .. } => {
                    push!(Node::Expression(key), depth + 1);
                }
                E::AssignSuper { key, value, .. } => {
                    push!(Node::Expression(key), depth + 1);
                    push!(Node::Expression(value), depth + 1);
                }
                E::PrivateMember { object, .. }
                | E::UpdatePrivate { object, .. }
                | E::PrivateIn { object, .. } => {
                    push!(Node::Expression(object), depth + 1);
                }
                E::AssignPrivate { object, value, .. } => {
                    push!(Node::Expression(object), depth + 1);
                    push!(Node::Expression(value), depth + 1);
                }
                E::SuperCall { arguments } => {
                    for argument in arguments.iter().rev() {
                        push!(Node::Argument(argument), depth + 1);
                    }
                }
                E::Literal(Literal::String(v)) => {
                    constants += 1;
                    work = work
                        .checked_add(v.len())
                        .ok_or("capacity: emission work overflow")?
                }
                E::Literal(Literal::BigInt(v)) => {
                    constants += 1;
                    work = work
                        .checked_add(v.len())
                        .ok_or("capacity: emission work overflow")?
                }
                E::Literal(Literal::Number(_)) => {
                    constants += 1;
                }
                E::Literal(_) | E::This | E::NewTarget => {}
                E::Read(reference)
                | E::Update {
                    target: reference, ..
                } => {
                    push!(Node::Reference(reference), depth);
                }
                E::Function(f) => {
                    push_function!(f, depth + 1);
                }
                E::Await(argument)
                | E::InvalidCallTarget(argument)
                | E::Yield {
                    argument: Some(argument),
                    ..
                } => {
                    push!(Node::Expression(argument), depth + 1);
                }
                E::Yield { argument: None, .. }
                | E::RegExp { .. }
                | E::ImportMeta
                | E::TemplateObject { .. } => {}
                E::ImportCall { specifier, options } => {
                    push!(Node::Expression(specifier), depth + 1);
                    if let Some(options) = options {
                        push!(Node::Expression(options), depth + 1);
                    }
                }
                E::SpreadArray(elements) => {
                    for element in elements.iter().rev() {
                        if let qjs_scalar_ssa_ir::js::ArrayElement::Value(e)
                        | qjs_scalar_ssa_ir::js::ArrayElement::Spread(e) = element
                        {
                            push!(Node::Expression(e), depth + 1)
                        }
                    }
                }
                E::Sequence(es) | E::Array(es) => {
                    for e in es.iter().rev() {
                        push!(Node::Expression(e), depth + 1)
                    }
                }
                E::Template {
                    parts,
                    substitutions,
                } => {
                    work = work
                        .checked_add(parts.len())
                        .ok_or("capacity: template work overflow")?;
                    if work > l.max_work {
                        return Err("capacity: emission work budget exceeded".into());
                    }
                    constants = constants
                        .checked_add(parts.len())
                        .ok_or("capacity: constants overflow")?;
                    if parts.len() > l.max_ir_nodes {
                        return Err("capacity: template parts budget exceeded".into());
                    }
                    for p in parts {
                        work = work
                            .checked_add(p.raw.len())
                            .and_then(|n| n.checked_add(p.cooked.len()))
                            .ok_or("capacity: template work overflow")?
                    }
                    for e in substitutions.iter().rev() {
                        push!(Node::Expression(e), depth + 1)
                    }
                }
                E::Chain { base, segments } => {
                    work = work
                        .checked_add(segments.len())
                        .ok_or("capacity: chain work overflow")?;
                    if work > l.max_work {
                        return Err("capacity: emission work budget exceeded".into());
                    }
                    nodes = nodes
                        .checked_add(segments.len())
                        .ok_or("capacity: chain IR nodes overflow")?;
                    if nodes > l.max_ir_nodes {
                        return Err("capacity: chain IR nodes budget exceeded".into());
                    }
                    push!(Node::Expression(base), depth + 1);
                    for segment in segments.iter().rev() {
                        match segment {
                            qjs_scalar_ssa_ir::js::ChainSegment::PrivateMember { .. } => {}
                            qjs_scalar_ssa_ir::js::ChainSegment::Member { key, .. } => {
                                push!(Node::Expression(key), depth + 1)
                            }
                            qjs_scalar_ssa_ir::js::ChainSegment::Call { arguments, .. } => {
                                for argument in arguments.iter().rev() {
                                    push!(Node::Argument(argument), depth + 1)
                                }
                            }
                        }
                    }
                }
                E::Object(entries) => {
                    work = work
                        .checked_add(entries.len())
                        .ok_or("capacity: object work overflow")?;
                    if work > l.max_work {
                        return Err("capacity: emission work budget exceeded".into());
                    }
                    nodes = nodes
                        .checked_add(entries.len())
                        .ok_or("capacity: object IR nodes overflow")?;
                    if nodes > l.max_ir_nodes {
                        return Err("capacity: object IR nodes budget exceeded".into());
                    }
                    for entry in entries.iter().rev() {
                        let (key, value) = match entry {
                            qjs_scalar_ssa_ir::js::ObjectEntry::Data { key, value, .. }
                            | qjs_scalar_ssa_ir::js::ObjectEntry::Getter { key, value }
                            | qjs_scalar_ssa_ir::js::ObjectEntry::Setter { key, value } => (Some(key), value),
                            qjs_scalar_ssa_ir::js::ObjectEntry::Prototype { value }
                            | qjs_scalar_ssa_ir::js::ObjectEntry::Spread { value } => (None, value),
                        };
                        if let Some(key) = key {
                            push!(Node::Key(key), depth + 1);
                        }
                        push!(Node::Expression(value), depth + 1)
                    }
                }
                E::Unary { argument, .. } => push!(Node::Expression(argument), depth + 1),
                E::DestructureAssign { pattern, value } => {
                    push!(Node::Pattern(pattern), depth + 1);
                    push!(Node::Expression(value), depth + 1)
                }
                E::Assign { target, value } | E::LogicalAssign { target, value, .. } => {
                    push!(Node::Reference(target), depth);
                    push!(Node::Expression(value), depth + 1)
                }
                E::Binary { left, right, .. } | E::Logical { left, right, .. } => {
                    push!(Node::Expression(left), depth + 1);
                    push!(Node::Expression(right), depth + 1)
                }
                E::Member { object, key } | E::UpdateMember { object, key, .. } => {
                    push!(Node::Expression(object), depth + 1);
                    push!(Node::Expression(key), depth + 1)
                }
                E::AssignMember { object, key, value }
                | E::CompoundAssignMember {
                    object, key, value, ..
                }
                | E::LogicalAssignMember {
                    object, key, value, ..
                } => {
                    push!(Node::Expression(object), depth + 1);
                    push!(Node::Expression(key), depth + 1);
                    push!(Node::Expression(value), depth + 1)
                }
                E::Conditional {
                    test,
                    consequent,
                    alternate,
                } => {
                    push!(Node::Expression(test), depth + 1);
                    push!(Node::Expression(consequent), depth + 1);
                    push!(Node::Expression(alternate), depth + 1)
                }
                E::Call { callee, arguments }
                | E::Construct { callee, arguments }
                | E::DirectEval { callee, arguments } => {
                    push!(Node::Expression(callee), depth + 1);
                    for argument in arguments.iter().rev() {
                        push!(Node::Argument(argument), depth + 1)
                    }
                }
            },
        }
        if work > l.max_work {
            return Err("capacity: emission work budget exceeded".into());
        }
        if constants > l.max_constants {
            return Err("capacity: emission constants budget exceeded".into());
        }
        if nodes
            .checked_add(pending.len())
            .is_none_or(|n| n > l.max_ir_nodes)
        {
            return Err("capacity: emission IR nodes budget exceeded".into());
        }
    }
    if functions > l.max_functions {
        return Err("capacity: emission functions budget exceeded".into());
    }
    if nodes > l.max_ir_nodes {
        return Err("capacity: emission IR nodes budget exceeded".into());
    }
    Ok(())
}
impl Budget {
    pub fn string(&self, value: &str) -> Result<(), String> {
        if value.len() > self.limits.max_atom_units.saturating_mul(3) {
            return Err("capacity: emission atom units budget exceeded".into());
        }
        self.charge(Counter::Work, value.len())?;
        self.text(value.encode_utf16().count())
    }
}
pub(crate) fn scalar_ir(f: &qjs_scalar_ssa_ir::Function, l: &EmissionLimits) -> Result<(), String> {
    l.validate()?;
    if l.max_functions == 0 {
        return Err("capacity: scalar functions budget exceeded".into());
    }
    if f.blocks.len() > l.max_ir_nodes {
        return Err("capacity: scalar IR nodes budget exceeded".into());
    }
    let mut nodes = 1usize;
    let mut locals = 0usize;
    let mut instructions = 1usize;
    let mut code = 5usize;
    let mut constants = 0usize;
    for b in &f.blocks {
        nodes = nodes
            .checked_add(1 + b.params.len() + b.instructions.len())
            .ok_or("capacity: IR nodes overflow")?;
        locals = locals
            .checked_add(b.params.len() + b.instructions.len())
            .ok_or("capacity: locals overflow")?;
        if nodes > l.max_ir_nodes {
            return Err("capacity: scalar IR nodes budget exceeded".into());
        }
        if nodes > l.max_work {
            return Err("capacity: scalar admission work budget exceeded".into());
        }
        if locals > l.max_locals {
            return Err("capacity: scalar locals budget exceeded".into());
        }
        for i in &b.instructions {
            use qjs_scalar_ssa_ir::InstructionKind::*;
            let (n, bytes) = match i.kind {
                Constant(qjs_scalar_ssa_ir::Scalar::Number(_)) => {
                    constants += 1;
                    (2, 8)
                }
                Constant(_) => (2, 4),
                Unary { .. } => (3, 7),
                Binary { .. } => (4, 10),
            };
            instructions = instructions
                .checked_add(n)
                .ok_or("capacity: instructions overflow")?;
            code = code
                .checked_add(bytes)
                .ok_or("capacity: code bytes overflow")?;
        }
        let edges: Vec<&qjs_scalar_ssa_ir::Edge> = match &b.terminator.kind {
            qjs_scalar_ssa_ir::TerminatorKind::Return(_) => Vec::new(),
            qjs_scalar_ssa_ir::TerminatorKind::Jump(e) => vec![e],
            qjs_scalar_ssa_ir::TerminatorKind::Branch {
                then_edge,
                else_edge,
                ..
            } => vec![then_edge, else_edge],
        };
        let args = edges
            .iter()
            .try_fold(0usize, |n, e| n.checked_add(e.arguments.len()))
            .ok_or("capacity: edge slots overflow")?;
        locals = locals
            .checked_add(args)
            .ok_or("capacity: locals overflow")?;
        let (extra, bytes) = match &b.terminator.kind {
            qjs_scalar_ssa_ir::TerminatorKind::Return(_) => (2, 4),
            qjs_scalar_ssa_ir::TerminatorKind::Jump(_) => (
                args.checked_mul(4)
                    .and_then(|n| n.checked_add(1))
                    .ok_or("capacity: edge instructions overflow")?,
                args.checked_mul(12)
                    .and_then(|n| n.checked_add(5))
                    .ok_or("capacity: edge code overflow")?,
            ),
            qjs_scalar_ssa_ir::TerminatorKind::Branch { .. } => (
                args.checked_mul(4)
                    .and_then(|n| n.checked_add(4))
                    .ok_or("capacity: edge instructions overflow")?,
                args.checked_mul(12)
                    .and_then(|n| n.checked_add(18))
                    .ok_or("capacity: edge code overflow")?,
            ),
        };
        instructions = instructions
            .checked_add(extra)
            .ok_or("capacity: instructions overflow")?;
        code = code
            .checked_add(bytes)
            .ok_or("capacity: code bytes overflow")?;
    }
    for (n, max, name) in [
        (nodes, l.max_ir_nodes, "IR nodes"),
        (locals, l.max_locals, "locals"),
        (instructions, l.max_instructions, "instructions"),
        (instructions, l.max_positions, "positions"),
        (code, l.max_code_bytes, "code bytes"),
        (constants, l.max_constants, "constants"),
    ] {
        if n > max {
            return Err(format!("capacity: scalar {name} budget exceeded"));
        }
    }
    // Dominator state and actual fixed-point work are metered separately by
    // qjs_scalar_ssa_ir::validate_with_limits before and during validation.
    if nodes > l.max_work {
        return Err("capacity: scalar admission work budget exceeded".into());
    }
    Ok(())
}
