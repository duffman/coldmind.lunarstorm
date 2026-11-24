//! Every file the pipeline embeds with `include_str!` must be tracked by git.

use std::path::Path;
use std::process::Command;

/// Defect: `templates/zebra.framework/dist/vcl.js` is embedded by `frontend.rs` but the
/// template's `.gitignore` ignored `dist`, so a fresh clone did not compile
/// `lunarstorm-pipeline` ("couldn't read ... vcl.js"). Decision: the prebuilt file is
/// committed (see `frontend.rs`: "a project build needs no JavaScript toolchain").
///
/// This test fails while git ignores any embedded input.
#[test]
fn embedded_inputs_are_not_git_ignored() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for path in [
        "templates/zebra.framework/dist/vcl.js",
        "framework/vcl/react/src/vcl.css",
        "framework/desktop/forms.js",
    ] {
        let ignored = Command::new("git")
            .args(["check-ignore", "-q", path])
            .current_dir(&root)
            .status()
            .expect("run git")
            .success();
        assert!(!ignored, "{path} is embedded by the pipeline but ignored by git");
    }
}
