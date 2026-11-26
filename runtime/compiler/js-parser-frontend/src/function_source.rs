// SPDX-License-Identifier: MIT
// Copyright (c) 2026 quickjs-rust contributors
//! Source text of class methods: finds a method's span in the original source, skipping
//! trivia.
// Adapted from Lewin671/quickjs-rust, commit
// 9d1a4bb5ab25774470ddab81e087f9794424e209,
// crates/qjs-runtime/src/bytecode/compiler_class.rs,
// class_method_source_text and skip_js_trivia.
// Full upstream notice: third-party/quickjs-rust/LICENSE.

use crate::source::SourceInput;

/// Preserve the original member text for Function.prototype.toString. OXC's
/// nested Function span starts after the property name; the member span retains
/// names, computed keys, async/generator prefixes, and accessor prefixes.
/// A static modifier is outside the method's source text and is removed.
pub(super) fn class_method_span(
    input: &SourceInput,
    member: oxc_span::Span,
    is_static: bool,
) -> Option<qjs_scalar_ssa_ir::js::Span> {
    let source = input
        .parser_text()
        .get(member.start as usize..member.end as usize)?;
    let mut offset = skip_js_trivia(source, 0);
    if is_static {
        if !source.get(offset..)?.starts_with("static") {
            return None;
        }
        offset = skip_js_trivia(source, offset + "static".len());
    }
    let start = (member.start as usize).checked_add(offset)?;
    let original = input.original_span(start..member.end as usize)?;
    Some(qjs_scalar_ssa_ir::js::Span {
        start: original.start,
        end: original.end,
    })
}

fn skip_js_trivia(source: &str, mut index: usize) -> usize {
    let bytes = source.as_bytes();
    // The second call begins immediately after the static keyword. Only an
    // actual line terminator in trivia can enable Annex B's closing comment.
    let mut line_start = false;
    while index < bytes.len() {
        // Script grammar may admit HTML comments even in a strict class body.
        // Module grammar rejects them before lowering reaches this helper.
        if source[index..].starts_with("<!--") || line_start && source[index..].starts_with("-->") {
            while index < bytes.len() {
                let Some(character) = source[index..].chars().next() else {
                    break;
                };
                if matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                    break;
                }
                index += character.len_utf8();
            }
            continue;
        }
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() {
                    let Some(character) = source[index..].chars().next() else {
                        break;
                    };
                    if matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                        break;
                    }
                    index += character.len_utf8();
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let start = index;
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
                line_start |= source[start..index]
                    .chars()
                    .any(|character| matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}'));
            }
            _ => {
                let Some(character) = source[index..].chars().next() else {
                    break;
                };
                // ECMAScript WhiteSpace and LineTerminator. Rust's broader
                // is_whitespace also includes U+0085, which JS does not.
                if !matches!(character,
                    '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' |
                    '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' |
                    '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
                {
                    break;
                }
                line_start |= matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}');
                index += character.len_utf8();
            }
        }
    }
    index
}
