//! Selected-frame lowering against an owned, independently verified lexical layout.
//! This module has no frame handle. Native installation must separately authenticate
//! a live synchronous frame lease and compare its fresh environment layout.
use qjs_bytecode::image::{
    Capture,
    eval::{HiddenBinding, Slot, VerifiedEnvLayout, VerifiedEvalImage},
};
use qjs_frontend::source::SourceInput;
use qjs_ir::js as ir;
use std::collections::BTreeMap;

/// `global_var`: the caller frame is global script code, so sloppy eval
/// `var`/`function` declarations become configurable global object properties.
pub fn compile_eval(
    input: &SourceInput,
    filename: &str,
    layout: &VerifiedEnvLayout,
    global_var: bool,
) -> Result<VerifiedEvalImage, String> {
    compile_eval_with_limits(
        input,
        filename,
        layout,
        global_var,
        &Default::default(),
        &mut |_| {},
    )
}

/// Compile from owned scope metadata under the same policy as isolated jobs.
/// A live frame lease is still required separately at native installation.
pub fn compile_eval_with_limits(
    input: &SourceInput,
    filename: &str,
    layout: &VerifiedEnvLayout,
    global_var: bool,
    limits: &crate::job_request::Budgets,
    observer: &mut dyn FnMut(qjs_frontend::ParsePhase),
) -> Result<VerifiedEvalImage, String> {
    limits.validate()?;
    if layout.parameter_scope()
        || layout
            .layout()
            .locals
            .iter()
            .any(|binding| binding.scope_next == qjs_bytecode::image::ScopeLink::Parameters)
    {
        return Err("unsupported: parameter-scope eval initialization is not implemented".into());
    }
    let new_target = layout.hidden(HiddenBinding::NewTarget).is_some();
    let private = frame_private_names(layout)?;
    let private_names: Vec<String> = private
        .iter()
        .map(|p| String::from_utf16_lossy(&p.name[1..]))
        .collect();
    let mut program = qjs_frontend::compile_eval_frame_program(
        input,
        filename,
        layout.layout().strict,
        new_target,
        &private_names,
        &limits.frontend,
        observer,
    )
    .map_err(|errors| {
        use qjs_frontend::Category;
        let prefix = if errors.iter().any(|e| e.category == Category::Capacity) {
            "capacity:"
        } else if errors.iter().any(|e| {
            matches!(
                e.category,
                Category::UnsupportedSyntax
                    | Category::UnsupportedType
                    | Category::UnsupportedSemantics
            )
        }) {
            "unsupported:"
        } else {
            "language:"
        };
        format!("{prefix} {errors:?}")
    })?;
    program.this_binding = None;
    program.uses_this = false;
    crate::budget::program(&program, &limits.emission)?;
    let mut maximum = Maximum(None);
    walk_body(&mut program.body, 0, &mut maximum)?;
    let next = maximum.0.map_or(Ok(0), |m| {
        m.checked_add(1).ok_or("capacity: eval symbol capacity")
    })?;
    let mut declared = Declared {
        scope: program.scope,
        names: BTreeMap::new(),
    };
    walk_body(&mut program.body, 0, &mut declared)?;
    let mut rebind = Rebind {
        layout,
        scope: program.scope,
        next,
        symbols: BTreeMap::new(),
        external: Vec::new(),
        renamed: BTreeMap::new(),
    };
    // Sloppy `var`/`function` whose name is a var/argument of the caller's own frame
    // assign that binding. Outer closure bindings stay untouched (the declaration goes
    // to `<var>`); a lexical binding between eval and the var scope is a SyntaxError.
    if !program.strict {
        for (symbol, name) in &declared.names {
            let units: Vec<u16> = name.encode_utf16().collect();
            let Some(visible) = layout.binding(&units) else {
                continue;
            };
            if matches!(visible.slot, Slot::Ref(_)) || visible.binding.var_kind == 4 {
                continue;
            }
            if visible.binding.is_lexical && visible.binding.var_kind != 3 {
                return Err(format!(
                    "language: invalid redefinition of lexical identifier '{name}'"
                ));
            }
            if let Some(external) = rebind.external(name)? {
                rebind.renamed.insert(*symbol, external);
            }
        }
    }
    walk_body(&mut program.body, 0, &mut rebind)?;
    // The caller's `<with>`/`<var>` scope objects, each with its rank in resolution
    // order: a name is tested against every object ranked before its binding.
    let chain = layout.chain();
    let rank = |name: &[u16]| chain.iter().position(|v| v.binding.name == name);
    let with_name: Vec<u16> = "<with>".encode_utf16().collect();
    let var_name: Vec<u16> = "<var>".encode_utf16().collect();
    let mut objects = Vec::new();
    // Other sloppy declarations go to the caller's own `<var>` object, when it has one.
    let mut var_object = None;
    for (at, v) in chain.iter().enumerate() {
        let flag = if v.binding.name == with_name {
            1
        } else if v.binding.name == var_name {
            0
        } else {
            continue;
        };
        if flag == 0
            && var_object.is_none()
            && !program.strict
            && !global_var
            && matches!(v.slot, Slot::Local(_))
        {
            var_object = Some(objects.len());
        }
        let (closure_type, var_idx) = match v.slot {
            Slot::Local(n) => (0, n),
            Slot::Arg(n) => (1, n),
            Slot::Ref(n) => (2, n),
        };
        let capture = Capture {
            name: v.binding.name.clone(),
            var_idx,
            closure_type,
            is_const: v.binding.is_const,
            is_lexical: v.binding.is_lexical,
            var_kind: v.binding.var_kind,
        };
        objects.push((capture, flag, at));
    }
    let ranks: BTreeMap<u32, usize> = rebind
        .external
        .iter()
        .filter_map(|(b, c)| rank(&c.name).map(|r| (b.symbol, r)))
        .collect();
    let (mut function, functions) = crate::js_emit::emit_eval_function_with_spans_and_limits(
        &program,
        &rebind.external,
        &private,
        global_var,
        &objects,
        var_object,
        ranks,
        &limits.emission,
    )?;
    function.function_name = "<eval>".into();
    function.flags = layout.layout().flags.clone();
    function.flags.has_prototype = false;
    function.flags.simple_parameters = true;
    function.flags.kind = 0;
    // Eval code reaches the caller's home object only through the frame, never
    // as its own method/constructor: it inherits no super or home permissions.
    function.flags.super_allowed = false;
    function.flags.super_call = false;
    function.flags.derived_constructor = false;
    function.flags.need_home_object = false;
    qjs_bytecode::image::eval::verify_and_encode_eval_with_limits(
        &function,
        layout,
        Some(qjs_bytecode::image::SpanMetadata {
            source_unit: qjs_bytecode::image::SourceUnit::Utf16,
            source_len: program.source.len() as u32,
            source_digest: qjs_bytecode::image::source_digest_utf16(&program.source),
            functions,
        }),
        &limits.image,
    )
}

/// The caller frame's visible private names (QuickJS var kinds 5..=9), in a
/// stable order whose index is the private id handed to the frontend.
fn frame_private_names(
    layout: &VerifiedEnvLayout,
) -> Result<Vec<crate::js_emit::EvalPrivate>, String> {
    const SETTER_SUFFIX: &str = "<set>";
    let suffix: Vec<u16> = SETTER_SUFFIX.encode_utf16().collect();
    let mut names: BTreeMap<Vec<u16>, crate::js_emit::EvalPrivate> = BTreeMap::new();
    for visible in layout.bindings() {
        let b = &visible.binding;
        if !(5..=9).contains(&b.var_kind) {
            continue;
        }
        let (closure_type, var_idx) = match visible.slot {
            Slot::Local(n) => (0, n),
            Slot::Arg(n) => (1, n),
            Slot::Ref(n) => (2, n),
        };
        let capture = Capture {
            name: b.name.clone(),
            var_idx,
            closure_type,
            is_const: b.is_const,
            is_lexical: b.is_lexical,
            var_kind: b.var_kind,
        };
        let base = if b.var_kind == 8 && b.name.ends_with(&suffix) {
            b.name[..b.name.len() - suffix.len()].to_vec()
        } else {
            b.name.clone()
        };
        if base.len() < 2 || base[0] != u16::from(b'#') {
            return Err("eval private binding name".into());
        }
        let entry = names.entry(base.clone()).or_default();
        entry.name = base;
        let role = match b.var_kind {
            5 => &mut entry.field,
            6 => &mut entry.method,
            7 | 9 => &mut entry.getter,
            _ => &mut entry.setter,
        };
        if role.replace(capture).is_some() {
            return Err("duplicate eval private binding".into());
        }
    }
    Ok(names.into_values().collect())
}

trait Visit {
    fn restricted_context(&mut self, _context: &'static str) -> Result<(), String> {
        Ok(())
    }
    fn binding(&mut self, _binding: &mut ir::Binding) -> Result<(), String> {
        Ok(())
    }
    fn reference(&mut self, _reference: &mut ir::Reference, _depth: usize) -> Result<(), String> {
        Ok(())
    }
    fn expression(
        &mut self,
        _expression: &mut ir::Expression,
        _depth: usize,
    ) -> Result<(), String> {
        Ok(())
    }
}
struct Maximum(Option<u32>);
impl Maximum {
    fn insert(&mut self, symbol: u32) {
        self.0 = Some(self.0.map_or(symbol, |m| m.max(symbol)));
    }
}
impl Visit for Maximum {
    fn binding(&mut self, b: &mut ir::Binding) -> Result<(), String> {
        self.insert(b.symbol);
        Ok(())
    }
    fn reference(&mut self, r: &mut ir::Reference, _depth: usize) -> Result<(), String> {
        if let Some(id) = r.symbol {
            self.insert(id)
        }
        Ok(())
    }
}
/// Symbols of the program-scope sloppy `var`/`function` bindings of eval code.
struct Declared {
    scope: u32,
    names: BTreeMap<u32, String>,
}
impl Visit for Declared {
    fn binding(&mut self, b: &mut ir::Binding) -> Result<(), String> {
        if b.scope == self.scope
            && matches!(
                b.kind,
                ir::DeclarationKind::Var | ir::DeclarationKind::Function
            )
        {
            self.names.insert(b.symbol, b.name.clone());
        }
        Ok(())
    }
}
struct Rebind<'a> {
    layout: &'a VerifiedEnvLayout,
    scope: u32,
    next: u32,
    symbols: BTreeMap<Vec<u16>, u32>,
    external: Vec<(ir::Binding, Capture)>,
    /// Eval-declared top-level symbol -> the caller's visible binding of the same name.
    renamed: BTreeMap<u32, u32>,
}
impl Rebind<'_> {
    fn external(&mut self, name: &str) -> Result<Option<u32>, String> {
        let units: Vec<_> = name.encode_utf16().collect();
        if let Some(symbol) = self.symbols.get(&units) {
            return Ok(Some(*symbol));
        }
        let Some(visible) = self.layout.binding(&units) else {
            return Ok(None);
        };
        let symbol = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or("capacity: eval symbol capacity")?;
        let b = &visible.binding;
        let kind = if b.var_kind == 4 {
            ir::DeclarationKind::FunctionName
        } else if b.var_kind == 3 {
            ir::DeclarationKind::Catch
        } else if b.is_const {
            ir::DeclarationKind::Const
        } else if b.is_lexical {
            ir::DeclarationKind::Let
        } else {
            ir::DeclarationKind::Var
        };
        let (closure_type, var_idx) = match visible.slot {
            Slot::Local(n) => (0, n),
            Slot::Arg(n) => (1, n),
            Slot::Ref(n) => (2, n),
        };
        self.external.push((
            ir::Binding {
                symbol,
                scope: self.scope,
                name: name.into(),
                kind,
                span: ir::Span { start: 0, end: 0 },
            },
            Capture {
                name: units.clone(),
                var_idx,
                closure_type,
                is_const: b.is_const,
                is_lexical: b.is_lexical,
                var_kind: b.var_kind,
            },
        ));
        self.symbols.insert(units, symbol);
        Ok(Some(symbol))
    }
}
impl Visit for Rebind<'_> {
    fn restricted_context(&mut self, context: &'static str) -> Result<(), String> {
        Err(format!(
            "unsupported: selected-frame eval {context} context is not qualified"
        ))
    }
    fn binding(&mut self, b: &mut ir::Binding) -> Result<(), String> {
        if let Some(&external) = self.renamed.get(&b.symbol) {
            b.symbol = external;
        }
        Ok(())
    }
    fn reference(&mut self, r: &mut ir::Reference, depth: usize) -> Result<(), String> {
        if let Some(external) = r.symbol.and_then(|s| self.renamed.get(&s).copied()) {
            r.symbol = Some(external);
            return Ok(());
        }
        if r.symbol.is_some() || depth > 0 && r.name == "arguments" {
            return Ok(());
        }
        r.symbol = self.external(&r.name)?;
        if depth == 0
            && r.name == "arguments"
            && r.symbol.is_none()
            && self.layout.layout().flags.arguments_allowed
        {
            return Err("eval arguments binding is not materialized in the selected frame".into());
        }
        Ok(())
    }
    fn expression(&mut self, e: &mut ir::Expression, depth: usize) -> Result<(), String> {
        use ir::ExpressionKind as E;
        match &e.kind {
            E::SuperMember { .. }
            | E::AssignSuper { .. }
            | E::UpdateSuper { .. }
            | E::SuperCall { .. } => {
                self.restricted_context("super")?;
            }
            E::NewTarget if depth == 0 => {
                if self.layout.hidden(HiddenBinding::NewTarget).is_none() {
                    return Err(
                        "eval new.target binding is not materialized in the selected frame".into(),
                    );
                }
                e.kind = ir::ExpressionKind::Read(ir::Reference {
                    symbol: self.external("new.target")?,
                    name: "new.target".into(),
                });
                return Ok(());
            }
            _ => {}
        }
        if depth == 0 && matches!(e.kind, ir::ExpressionKind::This) {
            if self.layout.hidden(HiddenBinding::This).is_none() {
                return Err("eval this binding is not materialized in the selected frame".into());
            }
            e.kind = ir::ExpressionKind::Read(ir::Reference {
                symbol: self.external("this")?,
                name: "this".into(),
            });
        }
        Ok(())
    }
}
fn walk_function(f: &mut ir::Function, depth: usize, v: &mut impl Visit) -> Result<(), String> {
    if let Some(binding) = &mut f.initializer_name_binding {
        v.binding(binding)?;
    }
    if let Some(binding) = &mut f.home_object_binding {
        v.binding(binding)?;
    }
    if let Some(binding) = &mut f.super_constructor_binding {
        v.binding(binding)?;
    }
    if let Some(binding) = &mut f.field_initializer_binding {
        v.binding(binding)?;
    }
    if let Some(binding) = &mut f.this_binding {
        v.binding(binding)?;
    }
    if let Some(binding) = &mut f.new_target_binding {
        v.binding(binding)?;
    }
    if let Some(binding) = &mut f.arguments_binding {
        v.binding(binding)?;
    }
    if let Some(b) = &mut f.self_binding {
        v.binding(b)?
    }
    let function_depth = depth + usize::from(!f.is_arrow);
    for pattern in f.parameters.iter_mut().chain(f.rest_parameter.iter_mut()) {
        walk_pattern(pattern, function_depth, v)?;
    }
    for copy in &mut f.body_parameter_copies {
        v.binding(&mut copy.binding)?;
        v.reference(&mut copy.parameter, function_depth)?;
    }
    walk_body(&mut f.body, depth + usize::from(!f.is_arrow), v)
}
fn walk_argument(
    argument: &mut ir::CallArgument,
    depth: usize,
    v: &mut impl Visit,
) -> Result<(), String> {
    match argument {
        ir::CallArgument::Value(value) | ir::CallArgument::Spread(value) => {
            walk_expression(value, depth, v)
        }
    }
}
fn walk_key(key: &mut ir::ObjectKey, depth: usize, v: &mut impl Visit) -> Result<(), String> {
    match key {
        ir::ObjectKey::Static(_) => Ok(()),
        ir::ObjectKey::Computed(value) => walk_expression(value, depth, v),
    }
}
fn walk_class_key(key: &mut ir::ClassKey, depth: usize, v: &mut impl Visit) -> Result<(), String> {
    match key {
        ir::ClassKey::Public(key) => walk_key(key, depth, v),
        ir::ClassKey::Private(_) => Ok(()),
    }
}
fn walk_class(class: &mut ir::Class, depth: usize, v: &mut impl Visit) -> Result<(), String> {
    // Classes declared in eval code own fresh private ids above the caller
    // frame's (0..n); the walk only rebinds their free references.
    if let Some(binding) = &mut class.self_binding {
        v.binding(binding)?;
    }
    v.binding(&mut class.instance_initializer_binding)?;
    if let Some(heritage) = &mut class.heritage {
        walk_expression(heritage, depth, v)?;
    }
    if let Some(constructor) = &mut class.constructor {
        walk_function(constructor, depth, v)?;
    }
    for element in &mut class.elements {
        match element {
            ir::ClassElement::Method { key, function, .. } => {
                walk_class_key(key, depth, v)?;
                walk_function(function, depth, v)?;
            }
            ir::ClassElement::Field {
                key, initializer, ..
            } => {
                walk_class_key(key, depth, v)?;
                if let Some(initializer) = initializer {
                    walk_function(initializer, depth, v)?;
                }
            }
            ir::ClassElement::StaticBlock(function) => walk_function(function, depth, v)?,
        }
    }
    Ok(())
}
fn walk_body(body: &mut [ir::Statement], depth: usize, v: &mut impl Visit) -> Result<(), String> {
    for s in body {
        walk_statement(s, depth, v)?
    }
    Ok(())
}
fn walk_statement(s: &mut ir::Statement, depth: usize, v: &mut impl Visit) -> Result<(), String> {
    use ir::StatementKind as S;
    match &mut s.kind {
        S::Class { binding, class } => {
            v.binding(binding)?;
            walk_class(class, depth, v)?;
        }
        S::Empty
        | S::Debugger
        | S::Break
        | S::Continue
        | S::BreakLabel(_)
        | S::ContinueLabel(_) => {}
        S::Expression(e) => walk_expression(e, depth, v)?,
        S::Declaration(ds) => {
            for d in ds {
                walk_pattern(&mut d.pattern, depth, v)?;
                if let Some(e) = &mut d.initializer {
                    walk_expression(e, depth, v)?
                }
            }
        }
        S::ForOf {
            left, right, body, ..
        }
        | S::ForIn {
            left, right, body, ..
        } => {
            match left {
                ir::ForTarget::Declaration(declaration) => {
                    walk_pattern(&mut declaration.pattern, depth, v)?;
                    if let Some(initializer) = &mut declaration.initializer {
                        walk_expression(initializer, depth, v)?;
                    }
                }
                ir::ForTarget::Assignment(pattern) => walk_pattern(pattern, depth, v)?,
            }
            walk_expression(right, depth, v)?;
            walk_statement(body, depth, v)?;
        }
        S::Label { body, .. } => walk_statement(body, depth, v)?,
        S::Switch {
            discriminant,
            cases,
            ..
        } => {
            walk_expression(discriminant, depth, v)?;
            for case in cases {
                if let Some(test) = &mut case.test {
                    walk_expression(test, depth, v)?;
                }
                walk_body(&mut case.consequent, depth, v)?;
            }
        }
        S::TryFinally {
            body, finalizer, ..
        } => {
            walk_body(body, depth, v)?;
            walk_body(finalizer, depth, v)?;
        }
        S::Throw(value) => walk_expression(value, depth, v)?,
        S::TryCatch {
            body,
            binding,
            handler,
            ..
        } => {
            walk_body(body, depth, v)?;
            if let Some(pattern) = binding {
                walk_pattern(pattern, depth, v)?;
            }
            walk_body(handler, depth, v)?;
        }
        S::Block(body) => walk_body(body, depth, v)?,
        S::With { object, body, .. } => {
            walk_expression(object, depth, v)?;
            walk_statement(body, depth, v)?;
        }
        S::If {
            test,
            consequent,
            alternate,
        } => {
            walk_expression(test, depth, v)?;
            walk_statement(consequent, depth, v)?;
            if let Some(s) = alternate {
                walk_statement(s, depth, v)?
            }
        }
        S::While { test, body } | S::DoWhile { test, body } => {
            walk_expression(test, depth, v)?;
            walk_statement(body, depth, v)?
        }
        S::For {
            init,
            test,
            update,
            body,
        } => {
            if let Some(s) = init {
                walk_statement(s, depth, v)?
            }
            if let Some(e) = test {
                walk_expression(e, depth, v)?
            }
            if let Some(e) = update {
                walk_expression(e, depth, v)?
            }
            walk_statement(body, depth, v)?
        }
        S::Return(e) => {
            if let Some(e) = e {
                walk_expression(e, depth, v)?
            }
        }
        S::Function { binding, function } => {
            v.binding(binding)?;
            walk_function(function, depth, v)?
        }
    }
    Ok(())
}
fn walk_pattern(pattern: &mut ir::Pattern, depth: usize, v: &mut impl Visit) -> Result<(), String> {
    match pattern {
        ir::Pattern::SuperMember { key } => {
            v.restricted_context("super assignment")?;
            walk_expression(key, depth, v)?;
        }
        ir::Pattern::PrivateMember { object, .. } => {
            walk_expression(object, depth, v)?;
        }
        ir::Pattern::Binding {
            binding,
            initialization_target,
        } => {
            v.binding(binding)?;
            if let Some(target) = initialization_target {
                v.reference(target, depth)?;
            }
        }
        ir::Pattern::Target(reference) => v.reference(reference, depth)?,
        ir::Pattern::Member { object, key } => {
            walk_expression(object, depth, v)?;
            walk_expression(key, depth, v)?;
        }
        ir::Pattern::Default { pattern, value } => {
            walk_pattern(pattern, depth, v)?;
            walk_expression(value, depth, v)?;
        }
        ir::Pattern::Array { elements, rest } => {
            for pattern in elements.iter_mut().flatten() {
                walk_pattern(pattern, depth, v)?;
            }
            if let Some(rest) = rest {
                walk_pattern(rest, depth, v)?;
            }
        }
        ir::Pattern::Object { properties, rest } => {
            for property in properties {
                if let ir::ObjectKey::Computed(key) = &mut property.key {
                    walk_expression(key, depth, v)?;
                }
                walk_pattern(&mut property.pattern, depth, v)?;
            }
            if let Some(rest) = rest {
                walk_pattern(rest, depth, v)?;
            }
        }
    }
    Ok(())
}
fn walk_expression(e: &mut ir::Expression, depth: usize, v: &mut impl Visit) -> Result<(), String> {
    v.expression(e, depth)?;
    use ir::ExpressionKind as E;
    match &mut e.kind {
        E::NamedEvaluation { name, value } => {
            walk_expression(name, depth, v)?;
            walk_expression(value, depth, v)?;
        }
        E::Class(class) => walk_class(class, depth, v)?,
        E::SuperMember { key } | E::UpdateSuper { key, .. } => walk_expression(key, depth, v)?,
        E::AssignSuper { key, value, .. } => {
            walk_expression(key, depth, v)?;
            walk_expression(value, depth, v)?;
        }
        E::SuperCall { arguments } => {
            for argument in arguments {
                walk_argument(argument, depth, v)?;
            }
        }
        E::PrivateMember { object, .. }
        | E::UpdatePrivate { object, .. }
        | E::PrivateIn { object, .. } => {
            walk_expression(object, depth, v)?;
        }
        E::AssignPrivate { object, value, .. } => {
            walk_expression(object, depth, v)?;
            walk_expression(value, depth, v)?;
        }
        E::Literal(_)
        | E::This
        | E::NewTarget
        | E::RegExp { .. }
        | E::ImportMeta
        | E::TemplateObject { .. }
        | E::Yield { argument: None, .. } => {}
        E::ImportCall { specifier, options } => {
            walk_expression(specifier, depth, v)?;
            if let Some(options) = options {
                walk_expression(options, depth, v)?;
            }
        }
        E::SpreadArray(elements) => {
            for element in elements {
                if let ir::ArrayElement::Value(e) | ir::ArrayElement::Spread(e) = element {
                    walk_expression(e, depth, v)?;
                }
            }
        }
        E::Await(argument)
        | E::InvalidCallTarget(argument)
        | E::Yield {
            argument: Some(argument),
            ..
        } => walk_expression(argument, depth, v)?,
        E::Read(r) | E::Update { target: r, .. } => v.reference(r, depth)?,
        E::Sequence(es) | E::Array(es) => {
            for e in es {
                walk_expression(e, depth, v)?
            }
        }
        E::Template { substitutions, .. } => {
            for e in substitutions {
                walk_expression(e, depth, v)?
            }
        }
        E::Conditional {
            test,
            consequent,
            alternate,
        } => {
            walk_expression(test, depth, v)?;
            walk_expression(consequent, depth, v)?;
            walk_expression(alternate, depth, v)?
        }
        E::Chain { base, segments } => {
            walk_expression(base, depth, v)?;
            for segment in segments {
                match segment {
                    ir::ChainSegment::PrivateMember { .. } => {}
                    ir::ChainSegment::Member { key, .. } => walk_expression(key, depth, v)?,
                    ir::ChainSegment::Call { arguments, .. } => {
                        for argument in arguments {
                            walk_argument(argument, depth, v)?;
                        }
                    }
                }
            }
        }
        E::Object(properties) => {
            for property in properties {
                let (key, value) = match property {
                    ir::ObjectEntry::Data { key, value, .. }
                    | ir::ObjectEntry::Getter { key, value }
                    | ir::ObjectEntry::Setter { key, value } => (Some(key), value),
                    ir::ObjectEntry::Prototype { value } | ir::ObjectEntry::Spread { value } => {
                        (None, value)
                    }
                };
                if let Some(ir::ObjectKey::Computed(key)) = key {
                    walk_expression(key, depth, v)?;
                }
                walk_expression(value, depth, v)?;
            }
        }
        E::Member { object, key } | E::UpdateMember { object, key, .. } => {
            walk_expression(object, depth, v)?;
            walk_expression(key, depth, v)?
        }
        E::Unary { argument, .. } => walk_expression(argument, depth, v)?,
        E::Binary { left, right, .. } | E::Logical { left, right, .. } => {
            walk_expression(left, depth, v)?;
            walk_expression(right, depth, v)?
        }
        E::DestructureAssign { pattern, value } => {
            walk_pattern(pattern, depth, v)?;
            walk_expression(value, depth, v)?;
        }
        E::Assign { target, value } | E::LogicalAssign { target, value, .. } => {
            v.reference(target, depth)?;
            walk_expression(value, depth, v)?
        }
        E::AssignMember { object, key, value }
        | E::CompoundAssignMember {
            object, key, value, ..
        }
        | E::LogicalAssignMember {
            object, key, value, ..
        } => {
            walk_expression(object, depth, v)?;
            walk_expression(key, depth, v)?;
            walk_expression(value, depth, v)?
        }
        E::Call { callee, arguments }
        | E::Construct { callee, arguments }
        | E::DirectEval { callee, arguments } => {
            walk_expression(callee, depth, v)?;
            for argument in arguments {
                walk_argument(argument, depth, v)?
            }
        }
        E::Function(f) => walk_function(f, depth, v)?,
    }
    Ok(())
}
