use coldmind_runtime::{engine::*, modules::*};
fn key(s: &str) -> ModuleKey {
    ModuleKey::from_utf8(s).unwrap()
}
#[test]
fn memory_provider_preserves_exact_aliases_and_registration() {
    let mut p = MemoryModuleProvider::new();
    let source = ScriptSource::new(
        JsString::from_utf8("export default 42"),
        "memory:one".into(),
    );
    p.insert(key("one"), source.clone()).unwrap();
    assert!(p.insert(key("one"), source).is_err());
    p.alias(key("entry"), JsString::from_utf8("./one"), key("one"))
        .unwrap();
    assert!(
        p.alias(key("entry"), JsString::from_utf8("./one"), key("other"))
            .is_err()
    );
    assert_eq!(
        p.resolve(&key("entry"), &JsString::from_utf8("./one"))
            .unwrap(),
        key("one")
    );
    assert_eq!(
        p.resolve(&key("entry"), &JsString::from_utf8("missing")),
        Err(ModuleError::NotFound)
    );
    let request = ModuleLoadRequest {
        key: key("one"),
        image_profile: None,
    };
    assert!(matches!(p.load(&request).unwrap(), ModuleInput::Source(_)));
    assert!(ModuleKey::from_utf8("").is_err());
    let limits = ModuleLimits {
        edges: 0,
        ..Default::default()
    };
    assert!(limits.validate().is_err());
}
#[test]
fn file_provider_confines_resolution_and_checks_read_limits() {
    use std::path::PathBuf;
    struct Directory(PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = std::env::temp_dir().join(format!(
        "jetbunny-provider-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let dir = Directory(root);
    std::fs::write(dir.0.join("entry.js"), "export const x=1").unwrap();
    let open = || cap_std::fs::Dir::open_ambient_dir(&dir.0, cap_std::ambient_authority()).unwrap();
    assert!(FileModuleProvider::new(open(), 0).is_err());
    let mut provider = FileModuleProvider::new(open(), 1024).unwrap();
    let entry = provider.entry("entry.js").unwrap();
    provider.alias("bare".into(), "entry.js".into()).unwrap();
    assert!(provider.alias("bare".into(), "entry.js".into()).is_err());
    assert!(
        provider
            .alias("invalid/name".into(), "entry.js".into())
            .is_err()
    );
    assert_eq!(
        provider
            .resolve(&entry, &JsString::from_utf8("./entry.js"))
            .unwrap(),
        entry
    );
    assert_eq!(
        provider
            .resolve(&entry, &JsString::from_utf8("bare"))
            .unwrap(),
        entry
    );
    assert_eq!(
        provider.resolve(&entry, &JsString::from_utf8("unknown")),
        Err(ModuleError::NotFound)
    );
    assert!(matches!(
        provider
            .load(&ModuleLoadRequest {
                key: entry.clone(),
                image_profile: None
            })
            .unwrap(),
        ModuleInput::Source(_)
    ));
    for invalid in [
        "",
        "../escape",
        "/etc/passwd",
        "x:y",
        "x\\y",
        "x%20y",
        "x\0y",
    ] {
        assert!(provider.entry(invalid).is_err());
    }
    assert!(provider.entry("missing.js").is_err());
    std::fs::create_dir(dir.0.join("nested")).unwrap();
    std::fs::write(dir.0.join("nested/child.js"), "export default 1").unwrap();
    let child = provider.entry("nested/child.js").unwrap();
    assert_eq!(
        provider
            .resolve(&child, &JsString::from_utf8("../entry.js"))
            .unwrap(),
        entry
    );
    assert_eq!(provider.entry("nested/../entry.js").unwrap(), entry);
    let small = FileModuleProvider::new(open(), 1).unwrap();
    assert_eq!(
        small
            .load(&ModuleLoadRequest {
                key: entry,
                image_profile: None
            })
            .err(),
        Some(ModuleError::Limit)
    );
    assert_eq!(
        provider
            .load(&ModuleLoadRequest {
                key: key("jetbunny:builtin"),
                image_profile: None
            })
            .err(),
        Some(ModuleError::NotFound)
    );
    std::fs::write(dir.0.join("invalid.js"), [0xff]).unwrap();
    let invalid = provider.entry("invalid.js").unwrap();
    assert_eq!(
        provider
            .load(&ModuleLoadRequest {
                key: invalid,
                image_profile: None
            })
            .err(),
        Some(ModuleError::InvalidIdentity)
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", dir.0.join("outside.js")).unwrap();
        assert!(provider.entry("outside.js").is_err());
    }
}
struct TempRoot(std::path::PathBuf);
impl TempRoot {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "jetbunny-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn provider(&self) -> FileModuleProvider {
        let dir =
            cap_std::fs::Dir::open_ambient_dir(&self.0, cap_std::ambient_authority()).unwrap();
        FileModuleProvider::new(dir, 1024).unwrap()
    }
}
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn file_identities_require_the_exact_on_disk_spelling() {
    let root = TempRoot::new("spelling");
    std::fs::write(root.0.join("a.js"), "").unwrap();
    std::fs::create_dir(root.0.join("lib")).unwrap();
    std::fs::write(root.0.join("lib/b.js"), "").unwrap();
    std::fs::write(root.0.join("\u{e9}.js"), "").unwrap();
    let provider = root.provider();
    let a = provider.entry("a.js").unwrap();
    provider.entry("lib/b.js").unwrap();
    provider.entry("\u{e9}.js").unwrap();
    for wrong in ["A.js", "LIB/b.js", "lib/B.js", "e\u{301}.js"] {
        assert_eq!(provider.entry(wrong), Err(ModuleError::NotFound), "{wrong}");
    }
    assert_eq!(
        provider.resolve(&a, &JsString::from_utf8("./A.js")),
        Err(ModuleError::NotFound)
    );
}
#[cfg(unix)]
#[test]
fn relative_parent_segments_resolve_lexically_before_symlinks() {
    let root = TempRoot::new("lexical");
    std::fs::create_dir_all(root.0.join("other/deep")).unwrap();
    std::fs::write(root.0.join("other/b.js"), "").unwrap();
    std::fs::write(root.0.join("b.js"), "").unwrap();
    std::fs::write(root.0.join("a.js"), "").unwrap();
    std::os::unix::fs::symlink("other/deep", root.0.join("lib")).unwrap();
    let provider = root.provider();
    let a = provider.entry("a.js").unwrap();
    assert_eq!(
        provider
            .resolve(&a, &JsString::from_utf8("./lib/../b.js"))
            .unwrap(),
        provider.entry("b.js").unwrap()
    );
}
