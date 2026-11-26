//! Pure Rust source-to-final-QuickJS compiler; no native runtime dependency.
use qjs_scalar_ssa_ir::{Function, Scalar, Span, Type, ValueId};
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Label(u32),
    Constant(Scalar),
    GetLocal(u32),
    PutLocal(u32),
    Negate,
    Not,
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
    Jump(u32),
    IfFalse(u32),
    Return,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Instruction {
    pub op: Op,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub struct Local {
    pub value: Option<ValueId>,
    pub ty: Type,
}
#[derive(Clone, Debug)]
pub struct Target {
    pub instructions: Vec<Instruction>,
    pub locals: Vec<Local>,
    pub label_count: u32,
}
pub fn lower(function: &Function) -> Result<Target, String> {
    lower_with_limits(function, &EmissionLimits::default())
}
mod budget;
pub fn lower_with_limits(function: &Function, limits: &EmissionLimits) -> Result<Target, String> {
    lower::lower_with_limits(function, limits)
}
mod lower;
pub use budget::EmissionLimits;

pub use qjs_js_parser_frontend::source::SourceInput;
pub use qjs_js_parser_frontend::{Category, Diagnostic, SourceMetadata};
#[derive(Clone, Copy, Debug, Default)]
pub struct CompileOptions {
    pub optimize: bool,
}
#[derive(Clone, Debug)]
pub struct CompilationUnit {
    pub target: Target,
    pub metadata: SourceMetadata,
}
pub fn compile(
    source: &str,
    filename: &str,
    options: &CompileOptions,
) -> Result<CompilationUnit, Vec<Diagnostic>> {
    let mut parsed = qjs_js_parser_frontend::compile_with_metadata(source, filename)?;
    budget::scalar_ir(&parsed.function, &EmissionLimits::default()).map_err(|message| {
        vec![Diagnostic {
            category: Category::Capacity,
            filename: filename.into(),
            span: parsed.metadata.function_span,
            message,
        }]
    })?;
    if options.optimize {
        qjs_scalar_ssa_ir::fold_constants(&mut parsed.function).map_err(|e| {
            vec![Diagnostic {
                category: if e
                    .iter()
                    .any(|e| e.kind == qjs_scalar_ssa_ir::ValidationErrorKind::Capacity)
                {
                    Category::Capacity
                } else {
                    Category::InvalidIr
                },
                filename: filename.into(),
                span: parsed.metadata.function_span,
                message: format!("{e:?}"),
            }]
        })?;
    }
    qjs_scalar_ssa_ir::validate(&parsed.function).map_err(|e| {
        vec![Diagnostic {
            category: if e
                .iter()
                .any(|e| e.kind == qjs_scalar_ssa_ir::ValidationErrorKind::Capacity)
            {
                Category::Capacity
            } else {
                Category::InvalidIr
            },
            filename: filename.into(),
            span: parsed.metadata.function_span,
            message: format!("{e:?}"),
        }]
    })?;
    let target = lower(&parsed.function).map_err(|message| {
        vec![Diagnostic {
            category: if message.starts_with("capacity:") {
                Category::Capacity
            } else {
                Category::InvalidIr
            },
            filename: filename.into(),
            span: parsed.metadata.function_span,
            message,
        }]
    })?;
    Ok(CompilationUnit {
        target,
        metadata: parsed.metadata,
    })
}
mod emit;
pub use emit::{FinalizedFunction, emit_target, emit_target_with_limits};

impl CompilationUnit {
    /// Produce verified native-format bytes using only Rust-owned compilation.
    pub fn finalize(&self) -> Result<qjs_bytecode_image_verifier::image::VerifiedImage, String> {
        self.finalize_with_limits(&EmissionLimits::default())
    }
    pub fn finalize_with_limits(
        &self,
        limits: &EmissionLimits,
    ) -> Result<qjs_bytecode_image_verifier::image::VerifiedImage, String> {
        let final_code = emit_target_with_limits(&self.target, &self.metadata, limits)?;
        let admission = budget::Budget::new(*limits)?;
        admission.string(&self.metadata.filename)?;
        admission.string(&self.metadata.function_name)?;
        if self
            .metadata
            .source
            .as_ref()
            .is_some_and(|s| s.original().len() > limits.max_source_units)
        {
            return Err("capacity: retained source units budget exceeded".into());
        }
        let retained_source = self
            .metadata
            .source
            .as_ref()
            .map(|source| {
                source
                    .original_units(
                        self.metadata.function_span.start as usize
                            ..self.metadata.function_span.end as usize,
                    )
                    .map(<[u16]>::to_vec)
                    .ok_or_else(|| "function range outside known legacy source".to_string())
            })
            .transpose()?;
        let function = qjs_bytecode_image_verifier::image::FunctionImage {
            code: final_code.code,
            constants: final_code
                .constants
                .into_iter()
                .map(|value| match value {
                    Scalar::Number(n) => qjs_bytecode_image_verifier::image::Constant::Number(n),
                    Scalar::Bool(b) => qjs_bytecode_image_verifier::image::Constant::Bool(b),
                })
                .collect(),
            local_count: final_code.local_count,
            arg_count: 0,
            defined_arg_count: 0,
            stack_size: final_code.stack_size,
            strict: self.metadata.strict,
            function_name: self.metadata.function_name.clone(),
            filename: self.metadata.filename.clone(),
            debug_locations: final_code.debug_locations,
            source: retained_source,
            ..Default::default()
        };
        let spans = self
            .metadata
            .source
            .as_ref()
            .map(|source| qjs_bytecode_image_verifier::image::SpanMetadata {
                source_unit: qjs_bytecode_image_verifier::image::SourceUnit::Utf8,
                source_len: self.metadata.source_len,
                source_digest: source.utf8_digest(),
                functions: vec![qjs_bytecode_image_verifier::image::FunctionSpans {
                    path: Vec::new(),
                    function_start: self.metadata.function_span.start,
                    function_end: self.metadata.function_span.end,
                    positions: final_code.positions,
                }],
            });
        qjs_bytecode_image_verifier::image::verify_and_encode(&function, spans)
    }
}

pub mod eval;
pub mod job;
pub mod job_protocol;
pub mod job_request;
pub mod js_emit;
pub use js_emit::{
    JsEmitOptions, compile_module, compile_script, compile_script_input_with_options,
    compile_script_with_options, emit_program, emit_program_with_limits, emit_program_with_options,
    emit_program_with_policies,
};

pub use qjs_bytecode_image_verifier::image::source_digest_utf16;
