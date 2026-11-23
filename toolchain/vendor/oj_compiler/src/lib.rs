// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Raphael Amorim

pub mod assets;
pub mod bundle;
pub mod cjs;
mod compile;
mod env;
pub mod glob;
mod hot;
pub mod html;
pub mod interop;
pub mod json;
mod rewrite;
mod scan;
mod sourcemap;
pub mod ssr;
pub mod tsconfig;

pub(crate) use compile::detect_refresh_registrations;
pub use compile::{
    compile, compile_module, compile_module_with_maps, CompileError, CompileOptions, CompileOutput,
    HotAccept, ImportRewriter, JsxConfig, COMPILE_STACK_SIZE,
};
pub(crate) use env::defines_for;
pub use env::ImportMetaEnv;

/// UTF-8 text of a JavaScript string value, as `TextEncoder` encodes it: a lone
/// surrogate, which has no UTF-8 form, becomes U+FFFD. Node, and so Vite, turn
/// specifiers and names into paths and keys the same way.
pub(crate) fn js_text(value: oxc_str::JSStr<'_>) -> std::borrow::Cow<'_, str> {
    match value.as_str() {
        Some(text) => std::borrow::Cow::Borrowed(text),
        None => std::borrow::Cow::Owned(
            char::decode_utf16(value.encode_utf16())
                .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect(),
        ),
    }
}
pub(crate) use rewrite::rewrite_module_specifiers_pub;
pub use scan::{exports, imports, rewrite_specifiers, specifier_edits, SpecifierEdits};
pub(crate) use scan::{scan, F_IMPORT_META_GLOB, F_IMPORT_PAREN};
pub use sourcemap::map_json_to_data_url;
