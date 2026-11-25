//! Backend generation (spec 4c): the generated entry, and the bundled artifact
//! run for real, under bun, against a fake of the host's bridge.

use std::fs;

use lunarstorm_model::extract;
use lunarstorm_pipeline::{backend, build, Failure};
use tempfile::TempDir;

/// Writes a project to a temporary directory.
///
/// # Arguments
/// * `files` - project-relative paths with their text.
///
/// # Returns
/// The project directory.
fn project(files: &[(&str, &str)]) -> TempDir {
    let directory = TempDir::new().expect("temp dir");
    for (path, text) in files {
        let path = directory.path().join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }
    directory
}

#[test]
fn code_that_does_not_bundle_fails_the_build_and_writes_nothing() {
    let directory = project(&[
        ("src/app.tsx", "export default (<application id=\"a\" mainForm=\"Main\" />);\n"),
        ("src/forms/main.view.tsx", "export default (<form name=\"Main\" />);\n"),
        ("src/forms/main.ts", "import { Form } from '@desktop/forms';\nimport { missing } from '../models/missing';\nexport class Main extends Form { x = missing; }\n"),
    ]);
    let out = TempDir::new().expect("temp dir");
    let target = out.path().join("build");
    match build(directory.path(), &target) {
        Err(Failure::Backend(diagnostics)) => assert!(diagnostics.contains("missing"), "{diagnostics}"),
        other => panic!("expected a backend failure, got {other:?}"),
    }
    assert!(!target.exists());
}

/// Builds in several threads of one process each bundle their own project: the
/// backend generator's work directory is not shared between them.
#[test]
fn concurrent_builds_each_bundle_their_own_project() {
    /// Builds started at the same instant, per round.
    const THREADS: usize = 16;
    /// Rounds, so one lucky schedule does not pass.
    const ROUNDS: usize = 12;
    for round in 0..ROUNDS {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
        let builds: Vec<_> = (0..THREADS)
            .map(|index| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let marker = format!("marker-{round}-{index}");
                    let class = format!("import {{ Form }} from '@desktop/forms';\nexport class Main extends Form {{ mark() {{ return '{marker}'; }} }}\n");
                    let directory = project(&[
                        ("src/app.tsx", "export default (<application id=\"a\" mainForm=\"Main\" />);\n"),
                        ("src/forms/main.view.tsx", "export default (<form name=\"Main\" />);\n"),
                        ("src/forms/main.ts", &class),
                    ]);
                    let model = extract(directory.path()).project.expect("the project extracts");
                    barrier.wait();
                    (marker, backend::generate(&model, directory.path()))
                })
            })
            .collect();
        for build in builds {
            let (marker, bundle) = build.join().expect("build thread");
            let bundle = bundle.unwrap_or_else(|diagnostics| panic!("{marker} did not bundle: {diagnostics}"));
            assert!(bundle.contains(&marker), "the bundle of {marker} holds another project's code");
        }
    }
}
