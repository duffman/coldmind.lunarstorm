//! Phase 1 correctness regressions, running entirely against a mock registry.
mod support;
use jetpack::{
    lockfile::Lockfile,
    manifest::Manifest,
    registry::{current_platform, Registry},
    resolve,
};
use serde_json::{json, Value};
use std::fs;
use support::{release, simple, FixtureRegistry};

fn make_manifest(root: Value) -> (tempfile::TempDir, Manifest) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("package.json");
    fs::write(&path, serde_json::to_vec(&root).unwrap()).unwrap();
    let manifest = Manifest::open(&path).unwrap();
    (dir, manifest)
}

#[test]
fn resolves_root_optional_dependencies_and_records_them_in_lock() {
    let fixture = FixtureRegistry::new(vec![("optional-addon", vec![simple("1.0.0")])]);
    let lock = fixture
        .resolve(json!({"optionalDependencies":{"optional-addon":"^1.0.0"}}))
        .unwrap();
    assert!(lock.packages["node_modules/optional-addon"].optional);
    assert_eq!(lock.optional_dependencies["optional-addon"], "^1.0.0");
    lock.validate().unwrap();
}

#[test]
fn unavailable_root_optional_dependency_is_omitted_not_fatal() {
    let fixture = FixtureRegistry::new(vec![]);
    let lock = fixture
        .resolve(json!({"optionalDependencies":{"does-not-exist":"^1"}}))
        .unwrap();
    assert!(lock.packages.is_empty());
    lock.validate().unwrap();
}

#[test]
fn root_optional_overrides_required_entry() {
    let fixture = FixtureRegistry::new(vec![]);
    let lock = fixture
        .resolve(json!({
            "dependencies":{"missing":"1.0.0"},
            "optionalDependencies":{"missing":"2.0.0"}
        }))
        .unwrap();
    assert!(lock.packages.is_empty());
    lock.validate().unwrap();
}

#[test]
fn required_platform_filtered_release_uses_compatible_older_version() {
    let (os, _) = current_platform();
    let other = if os == "linux" { "darwin" } else { "linux" };
    let old = simple("1.0.0");
    let mut newer = simple("1.1.0");
    newer["os"] = json!([other]);
    let fixture = FixtureRegistry::new(vec![("widget", vec![old, newer])]);
    let lock = fixture
        .resolve(json!({"dependencies":{"widget":"^1"}}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/widget"].version, "1.0.0");
}

#[test]
fn backtracks_to_older_release_when_newest_has_incompatible_peer() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![
                release("1.0.0", json!({}), json!({"react":"^18"})),
                release("1.1.0", json!({}), json!({"react":"^19"})),
            ],
        ),
        ("react", vec![simple("18.3.0"), simple("19.0.0")]),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"plugin":"^1", "react":"18.3.0"}}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/plugin"].version, "1.0.0");
    lock.validate().unwrap();
}

#[test]
fn tries_conflicting_candidates_before_unrelated_versions() {
    let mut packages = vec![
        (
            "a-plugin",
            vec![
                release("1.0.0", json!({}), json!({"host":"^1"})),
                release("1.1.0", json!({}), json!({"host":"^2"})),
            ],
        ),
        ("host", vec![simple("1.0.0"), simple("2.0.0")]),
    ];
    let mut dependencies = json!({"a-plugin":"^1", "host":"1.0.0"});
    for name in ["z0", "z1", "z2", "z3", "z4", "z5", "z6", "z7", "z8"] {
        packages.push((name, vec![simple("1.0.0"), simple("1.1.0")]));
        dependencies[name] = json!("^1");
    }
    let registry = FixtureRegistry::new(packages);
    let lock = registry
        .resolve(json!({"dependencies": dependencies}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/a-plugin"].version, "1.0.0");
    assert_eq!(lock.packages["node_modules/z0"].version, "1.1.0");
    assert_eq!(lock.packages.len(), 11);
}

#[test]
fn reports_the_search_limit_when_candidates_remain() {
    let plugin_versions = (0..257)
        .map(|patch| release(&format!("1.0.{patch}"), json!({}), json!({"host":"^2"})))
        .collect();
    let registry = FixtureRegistry::new(vec![
        ("plugin", plugin_versions),
        ("host", vec![simple("1.0.0")]),
    ]);
    let error = registry
        .resolve(json!({"dependencies":{"plugin":"^1", "host":"1.0.0"}}))
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("exhausted the 256-attempt limit"));
    assert!(format!("{error:#}").contains("ERESOLVE peer conflict"));
}

#[test]
fn backtracks_after_transitive_missing_dependency() {
    let fixture = FixtureRegistry::new(vec![(
        "widget",
        vec![
            simple("1.0.0"),
            release("1.1.0", json!({"no-such-subpackage":"1.0.0"}), json!({})),
        ],
    )]);
    let lock = fixture
        .resolve(json!({"dependencies":{"widget":"^1"}}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/widget"].version, "1.0.0");
}

#[test]
fn falls_back_to_older_auto_peer_when_newest_peer_subtree_is_broken() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"runtime":"^2"}))],
        ),
        (
            "runtime",
            vec![
                simple("2.0.0"),
                release("2.1.0", json!({"missing-library":"^1"}), json!({})),
            ],
        ),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0"}}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/runtime"].version, "2.0.0");
    assert!(lock.packages["node_modules/runtime"].peer);
    lock.validate().unwrap();
}

#[test]
fn previous_auto_peer_version_is_preferred_during_add_remove() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"runtime":"^2"}))],
        ),
        ("runtime", vec![simple("2.0.0"), simple("2.1.0")]),
        ("added", vec![simple("1.0.0")]),
    ]);
    let (_, initial) = make_manifest(json!({"dependencies":{
        "plugin":"1.0.0", "runtime":"2.0.0"
    }}));
    let mut registry = Registry::new(&fixture.url, support::transport()).unwrap();
    let old = resolve::resolve(&initial, &mut registry).unwrap();
    old.validate().unwrap();
    let (_, updated) = make_manifest(json!({"dependencies":{"plugin":"1.0.0", "added":"1.0.0"}}));
    let new = resolve::resolve_with_lock(&updated, &mut registry, Some(&old)).unwrap();
    assert_eq!(new.packages["node_modules/runtime"].version, "2.0.0");
}

#[test]
fn add_remove_resolution_preserves_compatible_pinned_versions() {
    let fixture = FixtureRegistry::new(vec![
        ("existing", vec![simple("1.0.0"), simple("1.1.0")]),
        ("added", vec![simple("3.0.0")]),
    ]);
    let (_, initial) = make_manifest(json!({"dependencies":{"existing":"1.0.0"}}));
    let mut registry = Registry::new(&fixture.url, support::transport()).unwrap();
    let old = resolve::resolve(&initial, &mut registry).unwrap();
    let (_, updated) = make_manifest(json!({"dependencies":{"existing":"^1", "added":"3.0.0"}}));
    let new = resolve::resolve_with_lock(&updated, &mut registry, Some(&old)).unwrap();
    assert_eq!(new.packages["node_modules/existing"].version, "1.0.0");
    assert_eq!(new.packages["node_modules/added"].version, "3.0.0");
}

#[test]
fn rejects_wrong_root_and_transitive_locked_versions_offline() {
    let fixture = FixtureRegistry::new(vec![
        (
            "app",
            vec![release("1.0.0", json!({"library":"^1"}), json!({}))],
        ),
        ("library", vec![simple("1.4.0")]),
    ]);
    let good = fixture
        .resolve(json!({"dependencies":{"app":"^1"}}))
        .unwrap();
    good.validate().unwrap();
    let mut wrong_root = good.clone();
    wrong_root
        .packages
        .get_mut("node_modules/app")
        .unwrap()
        .version = "2.0.0".into();
    assert!(format!("{:#}", wrong_root.validate().unwrap_err()).contains("version conflict"));
    let mut wrong_child = good.clone();
    wrong_child
        .packages
        .get_mut("node_modules/library")
        .unwrap()
        .version = "2.0.0".into();
    assert!(format!("{:#}", wrong_child.validate().unwrap_err()).contains("version conflict"));
}

#[test]
fn rejects_orphans_and_missing_required_transitive_edge() {
    let fixture = FixtureRegistry::new(vec![
        (
            "app",
            vec![release("1.0.0", json!({"library":"1.0.0"}), json!({}))],
        ),
        ("library", vec![simple("1.0.0")]),
    ]);
    let good = fixture
        .resolve(json!({"dependencies":{"app":"1.0.0"}}))
        .unwrap();
    let mut orphan = good.clone();
    let mut stray = orphan.packages["node_modules/app"].clone();
    stray.name = "orphan".into();
    stray.dependencies.clear();
    orphan.packages.insert("node_modules/orphan".into(), stray);
    assert!(format!("{:#}", orphan.validate().unwrap_err()).contains("orphan"));
    let mut missing = good.clone();
    missing.packages.remove("node_modules/library");
    assert!(missing.validate().is_err());
}

#[test]
fn tampering_platform_rejects_lock_graph() {
    let fixture = FixtureRegistry::new(vec![("widget", vec![simple("1.0.0")])]);
    let mut lock = fixture
        .resolve(json!({"dependencies":{"widget":"1.0.0"}}))
        .unwrap();
    lock.packages.get_mut("node_modules/widget").unwrap().os = vec!["fictional-os".into()];
    assert!(lock.validate().is_err());
}

#[test]
fn manifest_optional_changes_invalidate_ci_lock_match() {
    let fixture = FixtureRegistry::new(vec![("widget", vec![simple("1.0.0")])]);
    let lock: Lockfile = fixture
        .resolve(json!({"optionalDependencies":{"widget":"1.0.0"}}))
        .unwrap();
    assert!(lock.matches(
        &Default::default(),
        &Default::default(),
        &lock.optional_dependencies,
        &Default::default(),
        &Default::default()
    ));
    assert!(!lock.matches(
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default(),
        &Default::default()
    ));
}

#[test]
fn shared_transitive_dependency_is_hoisted_to_one_directory() {
    let fixture = FixtureRegistry::new(vec![
        (
            "app-a",
            vec![release("1.0.0", json!({"shared":"^1"}), json!({}))],
        ),
        (
            "app-b",
            vec![release("1.0.0", json!({"shared":"^1"}), json!({}))],
        ),
        (
            "app-c",
            vec![release("1.0.0", json!({"shared":"^1"}), json!({}))],
        ),
        ("shared", vec![simple("1.2.0")]),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"app-a":"1.0.0","app-b":"1.0.0","app-c":"1.0.0"}}))
        .unwrap();
    let shared: Vec<&String> = lock
        .packages
        .keys()
        .filter(|location| location.ends_with("/shared"))
        .collect();
    assert_eq!(shared, ["node_modules/shared"]);
    assert_eq!(lock.packages.len(), 4);
    lock.validate().unwrap();
}

#[test]
fn conflicting_versions_stay_nested_after_hoisting() {
    let fixture = FixtureRegistry::new(vec![
        (
            "app-a",
            vec![release("1.0.0", json!({"shared":"^1"}), json!({}))],
        ),
        (
            "app-b",
            vec![release("1.0.0", json!({"shared":"^2"}), json!({}))],
        ),
        ("shared", vec![simple("1.0.0"), simple("2.0.0")]),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"app-a":"1.0.0","app-b":"1.0.0"}}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/shared"].version, "1.0.0");
    assert_eq!(
        lock.packages["node_modules/app-b/node_modules/shared"].version,
        "2.0.0"
    );
    lock.validate().unwrap();
}

#[test]
fn hoisting_keeps_a_plugin_bound_to_its_nested_peer() {
    let registry = FixtureRegistry::new(vec![
        ("host", vec![simple("1.0.0"), simple("1.1.0")]),
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"host":"^1"}))],
        ),
        (
            "wrapper",
            vec![release(
                "1.0.0",
                json!({"host":"1.1.0", "plugin":"1.0.0"}),
                json!({}),
            )],
        ),
    ]);
    for root_plugin in [false, true] {
        let mut dependencies = json!({"host":"1.0.0", "wrapper":"1.0.0"});
        if root_plugin {
            dependencies["plugin"] = json!("1.0.0");
        }
        let lock = registry
            .resolve(json!({"dependencies": dependencies}))
            .unwrap();
        let (plugin, _) =
            jetpack::peer::dependency_provider(&lock.packages, "node_modules/wrapper", "plugin")
                .unwrap();
        assert_eq!(plugin, "node_modules/wrapper/node_modules/plugin");
        let (provider, host) = jetpack::peer::provider(&lock.packages, plugin, "host").unwrap();
        assert_eq!(provider, "node_modules/wrapper/node_modules/host");
        assert_eq!(host.version, "1.1.0");
    }
}

#[test]
fn hoisting_can_merge_plugins_that_share_the_same_peer() {
    let registry = FixtureRegistry::new(vec![
        ("host", vec![simple("1.0.0")]),
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"host":"^1"}))],
        ),
        (
            "wrapper",
            vec![release("1.0.0", json!({"plugin":"1.0.0"}), json!({}))],
        ),
    ]);
    let lock = registry
        .resolve(json!({"dependencies":{"host":"1.0.0", "plugin":"1.0.0", "wrapper":"1.0.0"}}))
        .unwrap();
    assert_eq!(lock.packages.len(), 3);
}

#[test]
fn optional_package_for_another_platform_is_kept_in_the_lock() {
    let mut native = simple("1.0.0");
    native["os"] = json!(["fictional-os"]);
    let fixture = FixtureRegistry::new(vec![("native", vec![native])]);
    let lock = fixture
        .resolve(json!({"optionalDependencies":{"native":"^1"}}))
        .unwrap();
    let record = &lock.packages["node_modules/native"];
    assert!(record.optional);
    assert_eq!(record.os, ["fictional-os"]);
    lock.validate().unwrap();
    let host = jetpack::lockfile::TargetPlatform::current();
    assert!(lock.skipped_on(&host).contains("node_modules/native"));
}

#[test]
fn optional_branch_pins_required_children_and_sibling_peers_for_its_targets() {
    let mut addon = release("1.0.0", json!({"child":"^1"}), json!({"runtime":"^1"}));
    addon["os"] = json!(["foreign-os", "excluded-os", "!excluded-os"]);
    addon["cpu"] = json!(["foreign-cpu"]);
    let mut child = simple("1.0.0");
    child["os"] = json!(["foreign-os"]);
    child["cpu"] = json!(["foreign-cpu"]);
    let mut unrelated = simple("1.1.0");
    unrelated["os"] = json!(["excluded-os"]);
    let runtime = release("1.0.0", json!({"native":"^1"}), json!({}));
    let fixture = FixtureRegistry::new(vec![
        ("addon", vec![addon]),
        ("child", vec![child.clone(), unrelated]),
        ("native", vec![child]),
        ("runtime", vec![runtime]),
    ]);
    let lock = fixture
        .resolve(json!({"optionalDependencies":{"addon":"^1"}}))
        .unwrap();

    assert_eq!(
        lock.packages["node_modules/addon/node_modules/child"].version,
        "1.0.0"
    );
    let peer = &lock.packages["node_modules/runtime"];
    assert!(peer.peer && peer.optional);
    assert_eq!(peer.os, ["foreign-os"]);
    assert_eq!(peer.cpu, ["foreign-cpu"]);
    assert!(lock
        .packages
        .contains_key("node_modules/runtime/node_modules/native"));
    let skipped = lock.skipped_on(&jetpack::lockfile::TargetPlatform::current());
    assert_eq!(
        skipped.into_iter().collect::<Vec<_>>(),
        ["node_modules/addon", "node_modules/runtime"]
    );
    lock.validate_installed(std::iter::empty()).unwrap();
    lock.validate_installed(lock.packages.keys()).unwrap();
}

#[test]
fn optional_branch_keeps_targets_supported_by_its_required_children() {
    let mut addon = release("1.0.0", json!({"child":"^1"}), json!({}));
    addon["os"] = json!(["foreign-os"]);
    let mut child = simple("1.0.0");
    child["os"] = json!(["foreign-os"]);
    child["cpu"] = json!(["foreign-cpu"]);
    let fixture = FixtureRegistry::new(vec![("addon", vec![addon]), ("child", vec![child])]);
    let lock = fixture
        .resolve(json!({"optionalDependencies":{"addon":"^1"}}))
        .unwrap();

    let parent = &lock.packages["node_modules/addon"];
    assert_eq!(parent.os, ["foreign-os"]);
    assert_eq!(parent.cpu, ["foreign-cpu"]);
    assert!(lock
        .packages
        .contains_key("node_modules/addon/node_modules/child"));
    lock.validate().unwrap();
}

#[test]
fn staged_graph_rejects_a_missing_provider_of_a_required_peer() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use flate2::{write::GzEncoder, Compression};
    use jetpack::transport::{Request, Response, Transport};
    use sha2::{Digest, Sha512};

    struct OnlyPlugin(Vec<u8>);
    impl Transport for OnlyPlugin {
        fn fetch(&self, requests: Vec<Request>) -> Vec<anyhow::Result<Response>> {
            requests
                .into_iter()
                .map(|request| {
                    anyhow::ensure!(request.url.ends_with("/plugin.tgz"), "runtime unavailable");
                    Ok(Response {
                        status: 200,
                        location: None,
                        body: self.0.clone(),
                    })
                })
                .collect()
        }
    }

    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"runtime":"^1"}))],
        ),
        ("runtime", vec![simple("1.0.0")]),
    ]);
    let mut lock = fixture
        .resolve(json!({
            "dependencies":{"plugin":"^1"},
            "optionalDependencies":{"runtime":"^1"}
        }))
        .unwrap();
    assert!(lock.packages["node_modules/runtime"].optional);
    lock.validate_installed(lock.packages.keys()).unwrap();
    let error = lock
        .validate_installed(
            lock.packages
                .keys()
                .filter(|path| *path != "node_modules/runtime"),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("missing required peer runtime"));

    // A download failure for the optional provider must leave the old install in place.
    let manifest = br#"{"name":"plugin","version":"1.0.0"}"#;
    let mut archive = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "package/package.json", manifest.as_slice())
        .unwrap();
    let tarball = archive.into_inner().unwrap().finish().unwrap();
    let plugin = lock.packages.get_mut("node_modules/plugin").unwrap();
    plugin.resolved = "https://fixtures.invalid/plugin.tgz".into();
    plugin.integrity = format!("sha512-{}", STANDARD.encode(Sha512::digest(&tarball)));
    let project = tempfile::tempdir().unwrap();
    fs::create_dir(project.path().join("node_modules")).unwrap();
    let old_file = project.path().join("node_modules/previous-install");
    fs::write(&old_file, "original").unwrap();

    let error = jetpack::install::install(project.path(), &lock, &OnlyPlugin(tarball)).unwrap_err();
    assert!(format!("{error:#}").contains("missing required peer runtime"));
    assert_eq!(fs::read_to_string(old_file).unwrap(), "original");
    assert!(!project.path().join("node_modules/plugin").exists());
}

#[test]
fn dependencies_entry_wins_over_dev_dependencies_entry() {
    let fixture = FixtureRegistry::new(vec![("widget", vec![simple("1.0.0"), simple("2.0.0")])]);
    let lock = fixture
        .resolve(json!({"dependencies":{"widget":"1.0.0"},"devDependencies":{"widget":"2.0.0"}}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/widget"].version, "1.0.0");
    lock.validate().unwrap();
}

#[test]
fn add_preserves_the_complete_artifact_pin_when_registry_metadata_changes() {
    let fixture = FixtureRegistry::new(vec![
        ("existing", vec![simple("1.0.0")]),
        ("added", vec![simple("1.0.0")]),
        ("retained-child", vec![simple("1.0.0")]),
    ]);
    let (_, initial) = make_manifest(json!({"dependencies":{"existing":"1.0.0"}}));
    let mut registry = Registry::new(&fixture.url, support::transport()).unwrap();
    let mut old = resolve::resolve(&initial, &mut registry).unwrap();
    let child = fixture
        .resolve(json!({"dependencies":{"retained-child":"1.0.0"}}))
        .unwrap();
    old.packages.insert(
        "node_modules/retained-child".into(),
        child.packages["node_modules/retained-child"].clone(),
    );
    let pin = old.packages.get_mut("node_modules/existing").unwrap();
    pin.dependencies
        .insert("retained-child".into(), "1.0.0".into());
    pin.resolved = "https://original.example/existing.tgz".into();
    pin.integrity = "sha1-AQEBAQEBAQEBAQEBAQEBAQEBAQE=".into();
    let original = pin.clone();
    let (_, updated) = make_manifest(json!({"dependencies":{"existing":"^1", "added":"1.0.0"}}));
    let mut registry = Registry::new(&fixture.url, support::transport()).unwrap();
    let new = resolve::resolve_with_lock(&updated, &mut registry, Some(&old)).unwrap();
    let retained = &new.packages["node_modules/existing"];
    assert_eq!(retained.resolved, original.resolved);
    assert_eq!(retained.integrity, original.integrity);
    assert_eq!(retained.dependencies, original.dependencies);
    assert!(new.packages.contains_key("node_modules/retained-child"));
}

#[test]
fn optional_consumer_tries_an_older_auto_peer_before_it_is_omitted() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"runtime":"^2"}))],
        ),
        (
            "runtime",
            vec![
                simple("2.0.0"),
                release("2.1.0", json!({"missing-library":"^1"}), json!({})),
            ],
        ),
    ]);
    let lock = fixture
        .resolve(json!({"optionalDependencies":{"plugin":"1.0.0"}}))
        .unwrap();
    assert!(lock.packages.contains_key("node_modules/plugin"));
    assert_eq!(lock.packages["node_modules/runtime"].version, "2.0.0");
    lock.validate().unwrap();
}

#[test]
fn optional_consumer_tries_an_older_release_before_it_is_omitted() {
    let fixture = FixtureRegistry::new(vec![(
        "plugin",
        vec![
            simple("1.0.0"),
            release("1.1.0", json!({"missing-library":"^1"}), json!({})),
        ],
    )]);
    let lock = fixture
        .resolve(json!({"optionalDependencies":{"plugin":"^1"}}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/plugin"].version, "1.0.0");
    lock.validate().unwrap();
}

#[test]
fn optional_failure_is_retained_after_a_required_candidate_fails() {
    let fixture = FixtureRegistry::new(vec![
        (
            "app",
            vec![
                release("1.0.0", json!({"missing-library":"^1"}), json!({})),
                simple("1.1.0"),
            ],
        ),
        (
            "plugin",
            vec![release("1.0.0", json!({"missing-library":"^1"}), json!({}))],
        ),
    ]);
    let lock = fixture
        .resolve(json!({
            "dependencies":{"app":"^1"},
            "optionalDependencies":{"plugin":"1.0.0"}
        }))
        .unwrap();
    assert_eq!(lock.packages["node_modules/app"].version, "1.1.0");
    assert!(!lock.packages.contains_key("node_modules/plugin"));
    lock.validate().unwrap();
}

#[test]
fn required_peer_narrows_optional_consumer_platforms() {
    let mut peer = simple("1.0.0");
    peer["cpu"] = json!(["foreign-cpu"]);
    let fixture = FixtureRegistry::new(vec![
        (
            "addon",
            vec![release("1.0.0", json!({}), json!({"runtime":"^1"}))],
        ),
        ("runtime", vec![peer]),
    ]);
    let lock = fixture
        .resolve(json!({"optionalDependencies":{"addon":"^1"}}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/addon"].cpu, ["foreign-cpu"]);
    assert_eq!(lock.packages["node_modules/runtime"].cpu, ["foreign-cpu"]);
    lock.validate().unwrap();
}

#[test]
fn narrowed_optional_consumer_preserves_a_peer_needed_by_a_required_consumer() {
    let (_, cpu) = current_platform();
    let mut constrained = release("1.0.0", json!({}), json!({"runtime":"^1"}));
    constrained["cpu"] = json!([cpu]);
    let fixture = FixtureRegistry::new(vec![
        (
            "required",
            vec![release("1.0.0", json!({}), json!({"runtime":"^1"}))],
        ),
        ("addon", vec![constrained]),
        ("runtime", vec![simple("1.0.0")]),
    ]);
    let lock = fixture
        .resolve(json!({
            "dependencies":{"required":"^1"}, "optionalDependencies":{"addon":"^1"}
        }))
        .unwrap();
    assert!(!lock.packages["node_modules/runtime"].optional);
    lock.validate().unwrap();
}

#[test]
fn equal_precedence_releases_have_a_deterministic_tie_breaker() {
    let first = simple("1.0.0+build1");
    let second = simple("1.0.0+build2");
    for versions in [
        vec![first.clone(), second.clone()],
        vec![second.clone(), first.clone()],
    ] {
        let fixture = FixtureRegistry::new(vec![("library", versions)]);
        for _ in 0..8 {
            let lock = fixture
                .resolve(json!({"dependencies":{"library":"^1"}}))
                .unwrap();
            assert_eq!(
                lock.packages["node_modules/library"].version,
                "1.0.0+build2"
            );
        }
    }
}
