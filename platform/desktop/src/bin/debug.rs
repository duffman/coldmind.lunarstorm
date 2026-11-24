//! Debug runtime: embeds the pipeline, the file watcher and the dev asset server (spec 6).
//!
//! Usage: `lunarstorm-debug <project>`. The project is what the embedded pipeline reads (here a
//! generated output directory: `zebra.json`, `backend.js`, `frontend/`).

use lunarstorm_platform::pipeline::OutputDirPipeline;
use lunarstorm_platform::{dev, ui};

/// Builds the project, then runs it with dev reload.
fn main() {
    let Some(root) = std::env::args_os().nth(1) else {
        eprintln!("usage: lunarstorm-debug <project>");
        std::process::exit(2);
    };
    let mut pipeline = Box::new(OutputDirPipeline::new(root));
    let build = dev::first_build(pipeline.as_mut());
    ui::run(build, ui::Mode::Debug, Some(dev::starter(pipeline)))
}
