//! Calls with owned argument lists and early anonymous-definition naming.
use super::*;

impl<'a> Emitter<'a> {
    /// Input is callee, callee/new.target, or receiver/callee, respectively.
    /// The spread path builds a dense private array with native iterator records;
    /// applying it never consults Function.prototype.call or array iteration.
    pub(super) fn emit_call_arguments(
        &mut self,
        arguments: &'a [ir::CallArgument],
        constructor: bool,
        method: bool,
        span: ir::Span,
    ) -> Result<(), String> {
        self.budget.charge(Counter::Work, arguments.len())?;
        if constructor && method {
            return Err("a call cannot be both a constructor and a method call".into());
        }
        if arguments
            .iter()
            .all(|argument| matches!(argument, ir::CallArgument::Value(_)))
        {
            let count = u16::try_from(arguments.len())
                .map_err(|_| "capacity: native call argument count")?;
            for argument in arguments {
                let ir::CallArgument::Value(value) = argument else {
                    unreachable!()
                };
                self.expression(value)?;
            }
            self.instruction(
                if constructor {
                    "call_constructor"
                } else if method {
                    "call_method"
                } else {
                    "call"
                },
                count.to_ne_bytes().to_vec(),
                usize::from(count) + if constructor || method { 2 } else { 1 },
                1,
                span,
            );
            return Ok(());
        }
        if method {
            self.simple("swap", 2, 2, span);
        } else if !constructor {
            self.simple("undefined", 0, 1, span);
        }
        // apply consumes callee, receiver/new.target, argument array.
        self.argument_array(arguments, span)?;
        self.instruction(
            "apply",
            u16::from(constructor).to_ne_bytes().to_vec(),
            3,
            1,
            span,
        );
        Ok(())
    }

    /// Push a dense private array of the evaluated arguments, spreading
    /// iterables with native iterator records.
    pub(super) fn argument_array(
        &mut self,
        arguments: &'a [ir::CallArgument],
        span: ir::Span,
    ) -> Result<(), String> {
        let array = self.temporary()?;
        let index = self.temporary()?;
        let value = self.temporary()?;
        self.instruction("array_from", 0u16.to_ne_bytes().to_vec(), 0, 1, span);
        self.slot("put_loc", array, 1, 0, span);
        self.constant(|| Constant::Number(0.0), span);
        self.slot("put_loc", index, 1, 0, span);
        for argument in arguments {
            match argument {
                ir::CallArgument::Value(expression) => {
                    self.expression(expression)?;
                    self.append_argument_value(array, index, value, expression.span);
                }
                ir::CallArgument::Spread(expression) => {
                    self.expression(expression)?;
                    self.simple("for_of_start", 1, 3, expression.span);
                    let next = self.label();
                    let done = self.label();
                    self.mark(next);
                    self.instruction("for_of_next", vec![0], 3, 5, expression.span);
                    self.jump("if_true", done, expression.span);
                    self.append_argument_value(array, index, value, expression.span);
                    self.jump("goto", next, expression.span);
                    self.mark(done);
                    self.simple("drop", 1, 0, expression.span);
                    self.simple("iterator_close", 3, 0, expression.span);
                }
            }
        }
        self.slot("get_loc", array, 0, 1, span);
        // Retire scratch locals while the array is retained on the stack. The
        // constructor result must then flow directly to checked-this binding.
        self.clear_temporaries(&[array, index, value], span);
        Ok(())
    }
    /// `eval(...)`: `eval` for a plain argument list, else `apply_eval`.
    /// The operand is the lexical scope head + 1 the direct eval runs in.
    pub(super) fn direct_eval(
        &mut self,
        callee: &'a ir::Expression,
        arguments: &'a [ir::CallArgument],
        span: ir::Span,
    ) -> Result<(), String> {
        self.budget.charge(Counter::Work, arguments.len() + 1)?;
        let scope = u16::try_from(self.scope_head().map_or(0, |slot| slot + 1))
            .map_err(|_| "capacity: direct eval scope-head budget")?;
        self.expression(callee)?;
        if arguments
            .iter()
            .all(|argument| matches!(argument, ir::CallArgument::Value(_)))
        {
            let count = u16::try_from(arguments.len())
                .map_err(|_| "capacity: native call argument count")?;
            for argument in arguments {
                if let ir::CallArgument::Value(value) = argument {
                    self.expression(value)?;
                }
            }
            let mut operands = count.to_le_bytes().to_vec();
            operands.extend(scope.to_le_bytes());
            self.instruction("eval", operands, usize::from(count) + 1, 1, span);
        } else {
            self.argument_array(arguments, span)?;
            self.instruction("apply_eval", scope.to_le_bytes().to_vec(), 2, 1, span);
        }
        Ok(())
    }

    /// An array literal with spread elements or holes, built element by
    /// element like a spread argument list.
    pub(super) fn spread_array(
        &mut self,
        elements: &'a [ir::ArrayElement],
        span: ir::Span,
    ) -> Result<(), String> {
        let array = self.temporary()?;
        let index = self.temporary()?;
        let value = self.temporary()?;
        self.instruction("array_from", 0u16.to_ne_bytes().to_vec(), 0, 1, span);
        self.slot("put_loc", array, 1, 0, span);
        self.constant(|| Constant::Number(0.0), span);
        self.slot("put_loc", index, 1, 0, span);
        for element in elements {
            match element {
                ir::ArrayElement::Value(expression) => {
                    self.expression(expression)?;
                    self.append_argument_value(array, index, value, expression.span);
                }
                ir::ArrayElement::Spread(expression) => {
                    self.expression(expression)?;
                    self.simple("for_of_start", 1, 3, expression.span);
                    let next = self.label();
                    let done = self.label();
                    self.mark(next);
                    self.instruction("for_of_next", vec![0], 3, 5, expression.span);
                    self.jump("if_true", done, expression.span);
                    self.append_argument_value(array, index, value, expression.span);
                    self.jump("goto", next, expression.span);
                    self.mark(done);
                    self.simple("drop", 1, 0, expression.span);
                    self.simple("iterator_close", 3, 0, expression.span);
                }
                ir::ArrayElement::Hole => {
                    self.slot("get_loc", index, 0, 1, span);
                    self.simple("inc", 1, 1, span);
                    self.slot("put_loc", index, 1, 0, span);
                }
            }
        }
        if matches!(elements.last(), Some(ir::ArrayElement::Hole)) {
            self.slot("get_loc", array, 0, 1, span);
            self.slot("get_loc", index, 0, 1, span);
            self.atom_op("put_field", "length", 2, 0, None, span);
        }
        self.slot("get_loc", array, 0, 1, span);
        self.clear_temporaries(&[array, index, value], span);
        Ok(())
    }
    fn append_argument_value(&mut self, array: u32, index: u32, value: u32, span: ir::Span) {
        self.slot("put_loc", value, 1, 0, span);
        self.slot("get_loc", array, 0, 1, span);
        self.slot("get_loc", index, 0, 1, span);
        self.slot("get_loc", value, 0, 1, span);
        self.simple("define_array_el", 3, 2, span);
        self.simple("drop", 1, 0, span);
        self.simple("drop", 1, 0, span);
        self.slot("get_loc", index, 0, 1, span);
        self.simple("inc", 1, 1, span);
        self.slot("put_loc", index, 1, 0, span);
    }

    /// Name classes during creation so static initializers can observe the name.
    pub(super) fn expression_named(
        &mut self,
        value: &'a ir::Expression,
        name: Option<u32>,
    ) -> Result<(), String> {
        match &value.kind {
            ir::ExpressionKind::Class(class) => self.class_value(class, name),
            ir::ExpressionKind::Function(function) if function.name.is_none() && name.is_some() => {
                let name = name.ok_or("anonymous definition name is missing")?;
                self.slot("get_loc", name, 0, 1, value.span);
                self.expression(value)?;
                self.simple("set_name_computed", 2, 2, value.span);
                self.simple("nip", 2, 1, value.span);
                Ok(())
            }
            _ => self.expression(value),
        }
    }

    pub(super) fn named_evaluation(
        &mut self,
        name: &'a ir::Expression,
        value: &'a ir::Expression,
        span: ir::Span,
    ) -> Result<(), String> {
        let slot = self.temporary()?;
        self.expression(name)?;
        self.slot("put_loc", slot, 1, 0, span);
        self.expression_named(value, Some(slot))?;
        self.clear_temporaries(&[slot], span);
        Ok(())
    }
}
