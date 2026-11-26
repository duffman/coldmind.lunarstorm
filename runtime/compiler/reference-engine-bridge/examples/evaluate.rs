//! Example: evaluates a JavaScript snippet with the pinned reference QuickJS-NG and
//! prints the result.

use qjs_reference_engine_bridge::ReferenceRunner;
use std::io::{self, Read};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let source_file = arguments.next();
    if arguments.next().is_some() {
        return Err("usage: evaluate [source.js|-]".into());
    }
    let runner = ReferenceRunner::from_env();
    let value = match source_file {
        Some(path) if path != "-" => runner.evaluate_file(path)?,
        _ => {
            let mut source = String::new();
            io::stdin().read_to_string(&mut source)?;
            runner.evaluate(&source)?
        }
    };
    println!("{value:?}");
    Ok(())
}
