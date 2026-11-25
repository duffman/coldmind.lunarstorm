// SPDX-License-Identifier: MIT
// Copyright (c) 2026 quickjs-rust contributors
// Adapted from Lewin671/quickjs-rust, commit
// 9d1a4bb5ab25774470ddab81e087f9794424e209,
// crates/qjs-runtime/src/bytecode/compiler_params.rs::contains_expressions.
// The complete upstream license is in third-party/quickjs-rust/LICENSE.

use oxc_ast::ast;

/// ContainsExpression decides whether function instantiation needs a separate
/// body variable environment. Rest parameters and bare destructuring patterns
/// are non-simple without themselves requiring this additional environment.
pub(super) fn contains_expressions(parameters: &ast::FormalParameters<'_>) -> bool {
    parameters.items.iter().any(|parameter| {
        parameter.initializer.is_some() || pattern_contains_expression(&parameter.pattern)
    }) || parameters
        .rest
        .as_ref()
        .is_some_and(|rest| pattern_contains_expression(&rest.rest.argument))
}

fn pattern_contains_expression(binding: &ast::BindingPattern<'_>) -> bool {
    match binding {
        ast::BindingPattern::BindingIdentifier(_) => false,
        // OXC represents nested defaults as AssignmentPattern, whereas the
        // upstream AST keeps a separate default on each binding element.
        ast::BindingPattern::AssignmentPattern(_) => true,
        ast::BindingPattern::ArrayPattern(pattern) => {
            pattern
                .elements
                .iter()
                .flatten()
                .any(pattern_contains_expression)
                || pattern
                    .rest
                    .as_ref()
                    .is_some_and(|rest| pattern_contains_expression(&rest.argument))
        }
        ast::BindingPattern::ObjectPattern(pattern) => {
            pattern
                .properties
                .iter()
                .any(|property| property.computed || pattern_contains_expression(&property.value))
                || pattern
                    .rest
                    .as_ref()
                    .is_some_and(|rest| pattern_contains_expression(&rest.argument))
        }
    }
}
