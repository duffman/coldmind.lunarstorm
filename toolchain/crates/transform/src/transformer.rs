//! `devtools_transform`: the one `onBeforeParse` hook.
//!
//! Bun runs a single `onBeforeParse` hook per file (the first registered that matches), so every
//! stage lives behind this one symbol. Order per file:
//! 1. replace: literal substitutions on the raw text;
//! 2. define and inject: one oxc parse of the stage-1 text, both visitors, merged edits.
//!
//! A stage whose cheap pre-check fails costs no parse. Syntax errors pass the file through.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[cfg(feature = "bun")]
use bun_native_plugin::{anyhow, bun, BunLoader, Result};
#[cfg(feature = "bun")]
use napi::Status;
#[cfg(feature = "bun")]
use napi_derive::napi;

use crate::bun_hook::{with_program, Loader};
use crate::constant_defines::DefineConfig;
use crate::dependency_injection::InjectConfig;
#[cfg(feature = "bun")]
use crate::native_state::{from_raw, Owned, PluginState, Shared};
use crate::source_edits::Edits;
use crate::text_replacements::replace_edits;

/// Upper bound on undrained warnings kept per plugin (a long dev session re-warns on every edit).
const MAX_WARNINGS: usize = 1000;

/// One literal substitution: every occurrence of `from` becomes `to`.
#[cfg_attr(feature = "bun", napi(object))]
#[derive(Debug)]
pub struct ReplacePair {
    pub from: String,
    pub to: String,
}

/// One constant: `key` (a dotted path rooted at a global or `import.meta`) becomes `value`,
/// a JS expression.
#[cfg_attr(feature = "bun", napi(object))]
#[derive(Debug)]
pub struct DefinePair {
    pub key: String,
    pub value: String,
}

/// Configuration of one plugin instance. Absent stages are off.
#[cfg_attr(feature = "bun", napi(object))]
#[derive(Debug, Default)]
pub struct TransformOptions {
    pub replace: Option<Vec<ReplacePair>>,
    pub define: Option<Vec<DefinePair>>,
    /// `Some(false)`: marked classes only; `Some(true)`: every class.
    pub inject_all: Option<bool>,
}

/// Counters since the plugin was created.
#[cfg_attr(feature = "bun", napi(object))]
#[derive(Debug, PartialEq, Eq)]
pub struct TransformStats {
    /// Files the hook was called for.
    pub files: u32,
    /// Files whose source changed.
    pub changed: u32,
    /// Files parsed by oxc (define/inject pre-checks passed).
    pub parsed: u32,
    /// Literal substitutions made.
    pub replaced: u32,
    /// Define references replaced.
    pub defined: u32,
    /// Classes given a `$inject` getter.
    pub injected: u32,
}

#[derive(Default)]
struct Counters {
    files: AtomicU64,
    changed: AtomicU64,
    parsed: AtomicU64,
    replaced: AtomicU64,
    defined: AtomicU64,
    injected: AtomicU64,
}

pub struct TransformState {
    replace: Vec<(String, String)>,
    define: Option<DefineConfig>,
    inject: Option<InjectConfig>,
    counters: Counters,
    warnings: Mutex<Vec<String>>,
}

#[cfg(feature = "bun")]
impl PluginState for TransformState {
    const TAG: u64 = 0x6476_6d2e_7472_6e73;
}

/// Result of running every stage on one file.
#[derive(Default)]
struct Transformed {
    code: Option<String>,
    parsed: bool,
    replaced: u64,
    defined: u64,
    injected: u64,
    warnings: Vec<String>,
}

/// Result for one input, identical across Bun, direct Rust and Tauri callers.
/// `code` always contains source text, unchanged when no stage produced edits.
/// This does not transpile TypeScript/JSX or execute the output.
#[derive(Debug, PartialEq, Eq)]
pub struct TransformOutput {
    pub code: String,
    pub changed: bool,
    pub parsed: bool,
    pub replaced: u64,
    pub defined: u64,
    pub injected: u64,
    /// Warnings for this input, prefixed with the supplied filename.
    pub warnings: Vec<String>,
}

impl TransformState {
    /// Validates stage options once. An instance may be shared across threads.
    pub fn new(options: TransformOptions) -> std::result::Result<Self, String> {
        let replace: Vec<(String, String)> = options
            .replace
            .unwrap_or_default()
            .into_iter()
            .map(|pair| (pair.from, pair.to))
            .collect();
        if replace.iter().any(|(from, _)| from.is_empty()) {
            return Err("replace pair `from` must not be empty".to_string());
        }
        let define = match options.define {
            Some(pairs) if !pairs.is_empty() => Some(DefineConfig::new(
                pairs.into_iter().map(|pair| (pair.key, pair.value)),
            )?),
            _ => None,
        };
        Ok(Self {
            replace,
            define,
            inject: options.inject_all.map(|all| InjectConfig { all }),
            counters: Counters::default(),
            warnings: Mutex::new(Vec::new()),
        })
    }

    /// Transforms source in memory and records counters/warnings. `filename` is
    /// a diagnostic label only: no file is read, written or evaluated.
    pub fn transform_source(
        &self,
        filename: &str,
        source: &str,
        loader: Loader,
    ) -> TransformOutput {
        let transformed = self.transform(source, loader);
        self.record(filename, &transformed);
        TransformOutput {
            changed: transformed.code.is_some(),
            code: transformed.code.unwrap_or_else(|| source.to_string()),
            parsed: transformed.parsed,
            replaced: transformed.replaced,
            defined: transformed.defined,
            injected: transformed.injected,
            warnings: transformed
                .warnings
                .into_iter()
                .map(|warning| format!("{filename}: {warning}"))
                .collect(),
        }
    }

    /// Counters since this engine was created, saturated to the Bun API's u32.
    pub fn stats(&self) -> TransformStats {
        let counters = &self.counters;
        let read =
            |counter: &AtomicU64| counter.load(Ordering::Relaxed).min(u32::MAX as u64) as u32;
        TransformStats {
            files: read(&counters.files),
            changed: read(&counters.changed),
            parsed: read(&counters.parsed),
            replaced: read(&counters.replaced),
            defined: read(&counters.defined),
            injected: read(&counters.injected),
        }
    }

    /// Drains the bounded warning queue; individual transform results also carry
    /// their own warnings even if the queue has reached its capacity.
    pub fn take_warnings(&self) -> Vec<String> {
        self.warnings
            .lock()
            .map(|mut warnings| std::mem::take(&mut *warnings))
            .unwrap_or_default()
    }

    /// Runs text replacement before AST work so later spans refer to the staged text.
    /// Define/injection visitors share one scoped parse and merge non-overlapping
    /// source edits; parse failure preserves already completed text replacements.
    fn transform(&self, source: &str, loader: Loader) -> Transformed {
        let mut out = Transformed::default();
        let replaced = replace_edits(&self.replace, source);
        out.replaced = replaced.len() as u64;
        let staged = (!replaced.is_empty()).then(|| replaced.apply(source));
        let text = staged.as_deref().unwrap_or(source);

        let define = self.define.as_ref().filter(|config| config.may_match(text));
        let inject = self.inject.as_ref().filter(|config| config.may_match(text));
        let mut edits = Edits::default();
        if define.is_some() || inject.is_some() {
            let ran = with_program(text, loader, |program, scoping| {
                if let Some(config) = define {
                    let found = config.edits(program, scoping);
                    out.defined = found.len() as u64;
                    edits.extend(found);
                }
                if let Some(config) = inject {
                    let (found, warnings) = config.edits(program, scoping);
                    out.injected = found.len() as u64;
                    out.warnings = warnings;
                    edits.extend(found);
                }
            });
            out.parsed = ran.is_some();
        }

        out.code = if !edits.is_empty() {
            Some(edits.apply(text))
        } else {
            staged
        };
        out
    }

    fn record(&self, file: &str, transformed: &Transformed) {
        let add = |counter: &AtomicU64, value: u64| counter.fetch_add(value, Ordering::Relaxed);
        add(&self.counters.files, 1);
        add(&self.counters.changed, transformed.code.is_some() as u64);
        add(&self.counters.parsed, transformed.parsed as u64);
        add(&self.counters.replaced, transformed.replaced);
        add(&self.counters.defined, transformed.defined);
        add(&self.counters.injected, transformed.injected);
        if !transformed.warnings.is_empty() {
            if let Ok(mut warnings) = self.warnings.lock() {
                for warning in &transformed.warnings {
                    if warnings.len() < MAX_WARNINGS {
                        warnings.push(format!("{file}: {warning}"));
                    }
                }
            }
        }
    }
}

/// Creates the state for the `devtools_transform` hook.
#[cfg(feature = "bun")]
#[napi]
pub fn create_state(options: TransformOptions) -> napi::Result<Owned<TransformState>> {
    TransformState::new(options)
        .map(Owned::new)
        .map_err(|message| napi::Error::new(Status::InvalidArg, message))
}

/// Counters of a state created by `create_state`.
#[cfg(feature = "bun")]
#[napi]
pub fn stats(state: Shared<TransformState>) -> TransformStats {
    state.get().stats()
}

/// Removes and returns the warnings collected so far. Bun drops warnings logged by native hooks,
/// so the JS side surfaces these.
#[cfg(feature = "bun")]
#[napi]
pub fn take_warnings(state: Shared<TransformState>) -> Vec<String> {
    state.get().take_warnings()
}

#[cfg(feature = "bun")]
#[bun]
pub fn devtools_transform(handle: &mut OnBeforeParse) -> Result<()> {
    // SAFETY: `from_raw` only accepts pointers carrying `TransformState`'s tag.
    let state: &TransformState = unsafe { handle.external(from_raw::<TransformState>)? }
        .ok_or_else(|| {
            anyhow::anyhow!("devtools_transform needs the `external` from createState()")
        })?;
    // SAFETY: `default_loader` is a `BunLoader` value written by Bun.
    let loader: BunLoader =
        unsafe { std::mem::transmute((*handle.args_raw).default_loader as u32) };
    let syntax = match loader {
        BunLoader::BUN_LOADER_JS => Loader::Js,
        BunLoader::BUN_LOADER_JSX => Loader::Jsx,
        BunLoader::BUN_LOADER_TS => Loader::Ts,
        BunLoader::BUN_LOADER_TSX => Loader::Tsx,
        _ => Loader::Other,
    };
    let file = handle.path()?.into_owned();
    let transformed = state.transform(&handle.input_source_code()?, syntax);
    state.record(&file, &transformed);
    if let Some(code) = transformed.code {
        handle.set_output_source_code(code, loader);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(
        replace: &[(&str, &str)],
        define: &[(&str, &str)],
        inject: Option<bool>,
    ) -> TransformState {
        TransformState {
            replace: replace
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            define: (!define.is_empty()).then(|| {
                DefineConfig::new(define.iter().map(|(a, b)| (a.to_string(), b.to_string())))
                    .unwrap()
            }),
            inject: inject.map(|all| InjectConfig { all }),
            counters: Counters::default(),
            warnings: Mutex::new(Vec::new()),
        }
    }

    #[test]
    fn runs_all_stages_with_one_parse() {
        let state = state(
            &[("__NAME__", "__MODE__")],
            &[("__MODE__", "\"dev\"")],
            Some(true),
        );
        let source = "import { Dep } from './dep';\nclass A { constructor(d: Dep) {} }\nexport const m = __NAME__;";
        let out = state.transform(source, Loader::Ts);
        assert!(out.parsed);
        assert_eq!((out.replaced, out.defined, out.injected), (1, 1, 1));
        let code = out.code.unwrap();
        assert!(code.contains("export const m = \"dev\";"), "{code}");
        assert!(
            code.contains("class A { static get $inject() { return [Dep]; }"),
            "{code}"
        );
    }

    #[test]
    fn skips_parsing_when_no_stage_can_match() {
        let state = state(&[], &[("__MODE__", "1")], Some(false));
        let out = state.transform("export const a = 1;", Loader::Ts);
        assert!(!out.parsed && out.code.is_none());
    }

    #[test]
    fn keeps_replace_output_when_parsing_fails() {
        let state = state(&[("A", "B")], &[("B", "1")], None);
        let out = state.transform("const = A", Loader::Ts);
        assert_eq!(out.code.as_deref(), Some("const = B"));
    }

    #[test]
    fn records_warnings_with_the_file() {
        let state = state(&[], &[], Some(false));
        let out = state.transform("@d class A { constructor(n: number) {} }", Loader::Ts);
        state.record("/src/a.ts", &out);
        let warnings = state.warnings.lock().unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].starts_with("/src/a.ts: devtools inject: A not wired"),
            "{}",
            warnings[0]
        );
    }
}
