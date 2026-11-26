//! Disposable compiler process. Links Rust/OXC and the pure codec, never a VM.
use qjs_bytecode_emitter::{
    job_protocol as protocol,
    job_request::{self, Mode},
};
#[path = "../worker_heap.rs"]
mod heap;
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};
type Failure = (i32, String);
fn failure(error: String) -> Failure {
    let code = if error.starts_with("capacity:") {
        4
    } else if error.starts_with("unsupported:") {
        5
    } else if error.starts_with("language:") {
        1
    } else {
        6
    };
    (code, error)
}

fn diagnostics(errors: Vec<qjs_js_parser_frontend::Diagnostic>) -> Failure {
    use qjs_js_parser_frontend::Category;
    let code = if errors.iter().any(|e| e.category == Category::Capacity) {
        4
    } else if errors.iter().any(|e| {
        matches!(
            e.category,
            Category::UnsupportedSyntax
                | Category::UnsupportedType
                | Category::UnsupportedSemantics
        )
    }) {
        5
    } else if errors.iter().any(|e| e.category == Category::InvalidIr) {
        6
    } else {
        1
    };
    (code, format!("{errors:?}"))
}
fn compile(request: &Path, response: &Path) -> Result<(), Failure> {
    let mut bytes = Vec::new();
    File::open(request)
        .and_then(|f| {
            f.take(job_request::MAX_REQUEST_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|e| (9, e.to_string()))?;
    let request = job_request::decode(&bytes).map_err(|e| (9, e))?;
    let input = qjs_js_parser_frontend::source::SourceInput::from_utf16(&request.source)
        .map_err(|e| (4, e.into()))?;
    let artifact = match request.mode {
        Mode::Script | Mode::Module => {
            let program = match request.mode {
                Mode::Script => qjs_js_parser_frontend::compile_program_input_with_limits_and_observer(
                    &input,
                    &request.filename,
                    &request.budgets.frontend,
                    &mut heap::parse_phase,
                ),
                Mode::Module => qjs_js_parser_frontend::compile_module_input_with_limits_and_observer(
                    &input,
                    &request.filename,
                    &request.budgets.frontend,
                    &mut heap::parse_phase,
                ),
                Mode::Eval => unreachable!(),
            }
            .map_err(diagnostics)?;
            let image = qjs_bytecode_emitter::emit_program_with_policies(
                &program,
                &Default::default(),
                &request.budgets.emission,
                &request.budgets.image,
            )
            .map_err(failure)?;
            image
                .serialize_artifact_with_limits(&request.budgets.image)
                .map_err(failure)?
        }
        Mode::Eval => {
            let image = qjs_bytecode_emitter::eval::compile_eval_with_limits(
                &input,
                &request.filename,
                request.environment.as_ref().unwrap(),
                false,
                &request.budgets,
                &mut heap::parse_phase,
            )
            .map_err(failure)?;
            image
                .serialize_artifact_with_limits(&request.budgets.image)
                .map_err(failure)?
        }
    };
    if artifact.len() > protocol::MAX_ARTIFACT_BYTES {
        return Err((3, "compiler artifact capacity exceeded".into()));
    }
    let response_bytes =
        job_request::response(request.mode, &request.digest, &artifact).map_err(failure)?;
    File::create_new(response)
        .and_then(|mut file| file.write_all(&response_bytes))
        .map_err(|e| (9, e.to_string()))
}
fn main() {
    if let Err(error) = heap::configure() {
        eprintln!("{error}");
        std::process::exit(9);
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = if (2..=3).contains(&args.len()) {
        if args.len() == 3 {
            match File::create_new(&args[2]) {
                Ok(file) => heap::set_progress(file),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(9);
                }
            }
        }
        compile(Path::new(&args[0]), Path::new(&args[1]))
    } else {
        Err((
            9,
            "usage: qjs-compile-worker REQUEST RESPONSE [PROGRESS]".into(),
        ))
    };
    if let Err((code, error)) = result {
        let end = error.floor_char_boundary(protocol::MAX_DIAGNOSTIC_BYTES.min(error.len()));
        eprintln!("{}", &error[..end]);
        std::process::exit(code);
    }
}
