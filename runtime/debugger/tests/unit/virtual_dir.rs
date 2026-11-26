//! VirtualDir: exact spelling, snapshot refresh, symlinks, confinement and scan counts.
use super::*;
use crate::modules::ModuleError;
use std::path::PathBuf;

struct Root(PathBuf);
impl Root {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "jetbunny-vdir-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, text: &str) {
        let full = self.0.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    fn tree(&self) -> VirtualDir {
        VirtualDir::new(
            cap_std::fs::Dir::open_ambient_dir(&self.0, cap_std::ambient_authority()).unwrap(),
        )
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn located(tree: &VirtualDir, path: &str) -> Option<(String, Kind)> {
    tree.locate(path).unwrap().map(|l| (l.path, l.kind))
}

#[test]
fn files_and_directories_resolve_to_root_relative_paths() {
    let root = Root::new("basic");
    root.write("a.js", "");
    root.write("lib/b.js", "");
    let tree = root.tree();
    assert_eq!(located(&tree, "a.js"), Some(("a.js".into(), Kind::File)));
    assert_eq!(
        located(&tree, "./lib//b.js"),
        Some(("lib/b.js".into(), Kind::File))
    );
    assert_eq!(located(&tree, "lib"), Some(("lib".into(), Kind::Dir)));
    assert_eq!(
        located(&tree, "lib/../a.js"),
        Some(("a.js".into(), Kind::File))
    );
    assert_eq!(located(&tree, ""), Some((String::new(), Kind::Dir)));
    assert_eq!(located(&tree, "missing.js"), None);
    assert_eq!(located(&tree, "a.js/x"), None, "a file has no children");
    assert_eq!(located(&tree, "a.js/.."), None, "a file has no parent link");
}

#[test]
fn only_the_exact_stored_spelling_matches() {
    let root = Root::new("spelling");
    root.write("a.js", "");
    root.write("lib/b.js", "");
    root.write("\u{e9}.js", "");
    let tree = root.tree();
    assert!(located(&tree, "\u{e9}.js").is_some());
    for wrong in ["A.js", "LIB/b.js", "lib/B.js", "e\u{301}.js"] {
        assert_eq!(located(&tree, wrong), None, "{wrong}");
    }
}

#[test]
fn each_directory_is_scanned_once_and_a_miss_rescans_once() {
    let root = Root::new("scans");
    for i in 0..50 {
        root.write(&format!("lib/m{i}.js"), "");
    }
    let tree = root.tree();
    for i in 0..50 {
        assert!(located(&tree, &format!("lib/m{i}.js")).is_some());
    }
    assert_eq!(tree.scans(), 2, "root and lib, once each");
    assert_eq!(located(&tree, "lib/missing.js"), None);
    assert_eq!(tree.scans(), 3, "a miss rereads its directory once");
    assert_eq!(tree.probe("lib/missing.js").unwrap(), None);
    assert_eq!(tree.scans(), 3, "a probe never rereads");
}

#[test]
fn files_created_after_a_listing_are_found_by_locate_but_not_by_probe() {
    let root = Root::new("late");
    root.write("a.js", "");
    let tree = root.tree();
    assert!(located(&tree, "a.js").is_some());
    root.write("late.js", "");
    assert_eq!(tree.probe("late.js").unwrap(), None);
    assert_eq!(
        located(&tree, "late.js"),
        Some(("late.js".into(), Kind::File))
    );
    assert_eq!(tree.probe("late.js").unwrap(), Some(Kind::File));
}

#[cfg(unix)]
#[test]
fn symlinks_resolve_to_real_paths_and_parent_links_are_physical() {
    let root = Root::new("links");
    root.write("other/deep/x.js", "");
    root.write("other/b.js", "");
    root.write("b.js", "");
    std::os::unix::fs::symlink("other/deep", root.0.join("link")).unwrap();
    std::os::unix::fs::symlink("b.js", root.0.join("alias.js")).unwrap();
    let tree = root.tree();
    assert_eq!(
        located(&tree, "link"),
        Some(("other/deep".into(), Kind::Dir))
    );
    assert_eq!(
        located(&tree, "link/x.js"),
        Some(("other/deep/x.js".into(), Kind::File))
    );
    assert_eq!(
        located(&tree, "link/../b.js"),
        Some(("other/b.js".into(), Kind::File)),
        "`..` leaves the directory the link resolved to"
    );
    assert_eq!(
        located(&tree, "alias.js"),
        Some(("b.js".into(), Kind::File))
    );
    assert_eq!(tree.probe("link").unwrap(), Some(Kind::Dir));
}

#[cfg(unix)]
#[test]
fn symlink_cycles_and_escapes_fail() {
    let root = Root::new("escape");
    root.write("a.js", "");
    std::os::unix::fs::symlink("b", root.0.join("a")).unwrap();
    std::os::unix::fs::symlink("a", root.0.join("b")).unwrap();
    std::os::unix::fs::symlink("/etc", root.0.join("out")).unwrap();
    std::os::unix::fs::symlink("../", root.0.join("up")).unwrap();
    let tree = root.tree();
    assert!(tree.locate("a").is_err(), "cycle");
    assert!(
        tree.locate("out/passwd").is_err(),
        "absolute link out of the root"
    );
    assert!(
        tree.locate("up/x").is_err(),
        "relative link out of the root"
    );
    assert!(matches!(tree.locate(".."), Err(ModuleError::Io(_))));
    assert!(matches!(tree.locate("lib/../../a.js"), Ok(None) | Err(_)));
    assert!(matches!(tree.locate("a.js/../.."), Ok(None)));
}

#[cfg(unix)]
#[test]
fn special_files_are_not_module_candidates() {
    let root = Root::new("special");
    let _socket = std::os::unix::net::UnixListener::bind(root.0.join("s.js")).unwrap();
    let tree = root.tree();
    assert_eq!(located(&tree, "s.js"), None);
}

#[test]
fn reads_stop_one_byte_past_the_limit() {
    let root = Root::new("read");
    root.write("big.js", &"x".repeat(100));
    let tree = root.tree();
    assert_eq!(tree.read("big.js", 10).unwrap().len(), 11);
    assert_eq!(tree.read("big.js", 1000).unwrap().len(), 100);
    assert_eq!(tree.read("missing.js", 10), Err(ModuleError::NotFound));
}
