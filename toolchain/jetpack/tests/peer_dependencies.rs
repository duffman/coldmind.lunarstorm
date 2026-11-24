mod support;
use jetpack::{lockfile::Lockfile, peer};
use serde_json::json;
use support::{release, simple, FixtureRegistry};

#[test]
fn required_peer_is_automatically_installed_as_root_sibling() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"react":"^18"}))],
        ),
        (
            "react",
            vec![simple("18.2.0"), simple("18.3.1"), simple("19.0.0")],
        ),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"plugin":"^1"}}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/react"].version, "18.3.1");
    assert!(lock.packages["node_modules/react"].peer);
    assert!(!lock
        .packages
        .contains_key("node_modules/plugin/node_modules/react"));
    lock.validate().unwrap();
}

#[test]
fn compatible_explicit_root_provider_is_reused() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"react":"^18"}))],
        ),
        ("react", vec![simple("18.3.1")]),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0", "react":"18.3.1"}}))
        .unwrap();
    assert_eq!(lock.packages.len(), 2);
    assert!(!lock.packages["node_modules/react"].peer);
}

#[test]
fn explicit_root_conflict_is_a_fatal_error() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"react":"^18"}))],
        ),
        ("react", vec![simple("18.3.1"), simple("19.0.0")]),
    ]);
    let error = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0", "react":"19.0.0"}}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("ERESOLVE"), "{error}");
    assert!(error.contains("react"), "{error}");
}

#[test]
fn nested_peer_is_placed_next_to_plugin_and_shadows_wrong_root_version() {
    let fixture = FixtureRegistry::new(vec![
        (
            "host",
            vec![release("1.0.0", json!({"plugin":"1.0.0"}), json!({}))],
        ),
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"react":"^18"}))],
        ),
        ("react", vec![simple("18.3.0"), simple("19.0.0")]),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"host":"1.0.0", "react":"19.0.0"}}))
        .unwrap();
    assert_eq!(
        lock.packages["node_modules/host/node_modules/react"].version,
        "18.3.0"
    );
    assert_eq!(lock.packages["node_modules/react"].version, "19.0.0");
    assert!(!lock
        .packages
        .contains_key("node_modules/host/node_modules/plugin/node_modules/react"));
    lock.validate().unwrap();
}

#[test]
fn two_consumers_share_a_version_satisfying_both_ranges() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin-a",
            vec![release("1.0.0", json!({}), json!({"react":"^18.0.0"}))],
        ),
        (
            "plugin-b",
            vec![release("1.0.0", json!({}), json!({"react":"^18.2.0"}))],
        ),
        (
            "react",
            vec![
                simple("18.1.0"),
                simple("18.2.0"),
                simple("18.3.1"),
                simple("19.0.0"),
            ],
        ),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"plugin-a":"1.0.0", "plugin-b":"1.0.0"}}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/react"].version, "18.3.1");
    assert_eq!(lock.packages.len(), 3);
}

#[test]
fn incompatible_sibling_peer_ranges_fail_without_a_bad_lockfile() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin-a",
            vec![release("1.0.0", json!({}), json!({"react":"^18"}))],
        ),
        (
            "plugin-b",
            vec![release("1.0.0", json!({}), json!({"react":"^19"}))],
        ),
        ("react", vec![simple("18.3.0"), simple("19.0.0")]),
    ]);
    let error = fixture
        .resolve(json!({"dependencies":{"plugin-a":"1.0.0", "plugin-b":"1.0.0"}}))
        .unwrap_err();
    assert!(format!("{error:#}").contains("ERESOLVE"));
}

#[test]
fn missing_optional_peer_is_not_auto_installed() {
    let mut plugin = release("1.0.0", json!({}), json!({"typescript":"^5"}));
    plugin["peerDependenciesMeta"] = json!({"typescript":{"optional":true}});
    let fixture = FixtureRegistry::new(vec![("plugin", vec![plugin])]);
    let lock = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0"}}))
        .unwrap();
    assert_eq!(lock.packages.len(), 1);
    lock.validate().unwrap();
}

#[test]
fn present_but_incompatible_optional_peer_is_rejected() {
    let mut plugin = release("1.0.0", json!({}), json!({"typescript":"^5"}));
    plugin["peerDependenciesMeta"] = json!({"typescript":{"optional":true}});
    let fixture = FixtureRegistry::new(vec![
        ("plugin", vec![plugin]),
        ("typescript", vec![simple("4.9.0")]),
    ]);
    let error = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0", "typescript":"4.9.0"}}))
        .unwrap_err();
    assert!(format!("{error:#}").contains("ERESOLVE"));
}

#[test]
fn peer_of_an_auto_installed_peer_is_placed_in_the_same_host_scope() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"host":"^2"}))],
        ),
        (
            "host",
            vec![release("2.0.0", json!({}), json!({"runtime":"^3"}))],
        ),
        ("runtime", vec![simple("3.2.0")]),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0"}}))
        .unwrap();
    assert!(lock.packages.contains_key("node_modules/host"));
    assert!(lock.packages.contains_key("node_modules/runtime"));
    assert_eq!(lock.packages.len(), 3);
}

#[test]
fn cyclic_peer_graph_reaches_a_fixed_point() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin-a",
            vec![release("1.0.0", json!({}), json!({"plugin-b":"^1"}))],
        ),
        (
            "plugin-b",
            vec![release("1.0.0", json!({}), json!({"plugin-a":"^1"}))],
        ),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"plugin-a":"1.0.0"}}))
        .unwrap();
    assert_eq!(lock.packages.len(), 2);
    lock.validate().unwrap();
}

#[test]
fn scoped_peer_package_resolves_under_correct_path() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"@scope/runtime":"^2"}))],
        ),
        ("@scope/runtime", vec![simple("2.0.0")]),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0"}}))
        .unwrap();
    assert_eq!(
        lock.packages["node_modules/@scope/runtime"].version,
        "2.0.0"
    );
}

#[test]
fn optional_ordinary_subtree_failure_rolls_back_its_auto_peers() {
    let mut optional = release(
        "1.0.0",
        json!({"missing-required":"1.0.0"}),
        json!({"react":"^18"}),
    );
    optional["optionalDependencies"] = json!({});
    let mut app = release("1.0.0", json!({}), json!({}));
    app["optionalDependencies"] = json!({"optional-plugin":"1.0.0"});
    let fixture = FixtureRegistry::new(vec![
        ("app", vec![app]),
        ("optional-plugin", vec![optional]),
        ("react", vec![simple("18.3.0")]),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"app":"1.0.0"}}))
        .unwrap();
    assert_eq!(lock.packages.len(), 1);
    assert!(!lock
        .packages
        .contains_key("node_modules/app/node_modules/react"));
}

#[test]
fn optional_package_with_incompatible_peer_is_omitted() {
    let mut app = release("1.0.0", json!({"react":"19.0.0"}), json!({}));
    app["optionalDependencies"] = json!({"optional-plugin":"1.0.0"});
    let fixture = FixtureRegistry::new(vec![
        ("app", vec![app]),
        (
            "optional-plugin",
            vec![release("1.0.0", json!({}), json!({"react":"^18"}))],
        ),
        ("react", vec![simple("18.3.0"), simple("19.0.0")]),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{"app":"1.0.0"}}))
        .unwrap();
    assert_eq!(lock.packages.len(), 2);
    assert!(!lock.packages.contains_key("node_modules/optional-plugin"));
    assert_eq!(lock.packages["node_modules/react"].version, "19.0.0");
    lock.validate().unwrap();
}

#[test]
fn root_manifest_peer_is_auto_installed_and_optional_root_peer_is_not() {
    let fixture = FixtureRegistry::new(vec![("react", vec![simple("18.3.0")])]);
    let required = fixture
        .resolve(json!({"peerDependencies":{"react":"^18"}}))
        .unwrap();
    assert!(required.packages["node_modules/react"].peer);
    let optional = fixture
        .resolve(json!({
            "peerDependencies":{"react":"^18"},
            "peerDependenciesMeta":{"react":{"optional":true}}
        }))
        .unwrap();
    assert!(optional.packages.is_empty());
}

#[test]
fn broken_lockfile_peer_version_or_missing_provider_is_rejected_offline() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"react":"^18"}))],
        ),
        ("react", vec![simple("18.3.0")]),
    ]);
    let good = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0"}}))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rustnpm-lock.json");
    good.write(&path).unwrap();
    assert_eq!(Lockfile::open(&path).unwrap().packages.len(), 2);

    let mut wrong = good.clone();
    wrong
        .packages
        .get_mut("node_modules/react")
        .unwrap()
        .version = "19.0.0".into();
    assert!(wrong.validate().is_err());

    let mut absent = good.clone();
    absent.packages.remove("node_modules/react");
    assert!(absent.validate().is_err());

    let mut inside = good.clone();
    let provider = inside.packages.remove("node_modules/react").unwrap();
    inside
        .packages
        .insert("node_modules/plugin/node_modules/react".into(), provider);
    assert!(inside.validate().is_err());
}

#[test]
fn missing_peer_is_not_confused_with_an_inner_node_modules_dependency() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"react":"^18"}))],
        ),
        ("react", vec![simple("18.3.0")]),
    ]);
    let graph = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0"}}))
        .unwrap();
    assert_eq!(
        peer::provider(&graph.packages, "node_modules/plugin", "react")
            .unwrap()
            .1
            .version,
        "18.3.0"
    );
}

#[test]
fn optional_peer_is_installed_when_another_consumer_requires_it() {
    let mut optional = release("1.0.0", json!({}), json!({"typescript":"^5"}));
    optional["peerDependenciesMeta"] = json!({"typescript":{"optional":true}});
    let fixture = FixtureRegistry::new(vec![
        ("plugin-optional", vec![optional]),
        (
            "plugin-required",
            vec![release("1.0.0", json!({}), json!({"typescript":"^5"}))],
        ),
        ("typescript", vec![simple("5.4.0")]),
    ]);
    let lock = fixture
        .resolve(json!({"dependencies":{
            "plugin-optional":"1.0.0", "plugin-required":"1.0.0"
        }}))
        .unwrap();
    assert_eq!(lock.packages["node_modules/typescript"].version, "5.4.0");
    assert!(lock.packages["node_modules/typescript"].peer);
}

#[test]
fn non_semver_peer_spec_is_explicitly_rejected() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"react":"latest"}))],
        ),
        ("react", vec![simple("18.3.0")]),
    ]);
    let error = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0"}}))
        .unwrap_err();
    assert!(format!("{error:#}").contains("unsupported peer dependency"));
}

#[test]
fn dangling_optional_peer_metadata_is_rejected() {
    let mut plugin = simple("1.0.0");
    plugin["peerDependenciesMeta"] = json!({"react":{"optional":true}});
    let fixture = FixtureRegistry::new(vec![("plugin", vec![plugin])]);
    let error = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0"}}))
        .unwrap_err();
    assert!(format!("{error:#}").contains("undeclared"));
}

#[test]
fn incompatible_explicit_peer_cannot_be_hidden_inside_consumer_scope() {
    let fixture = FixtureRegistry::new(vec![
        (
            "plugin",
            vec![release("1.0.0", json!({}), json!({"react":"^18"}))],
        ),
        ("react", vec![simple("18.3.0")]),
    ]);
    let mut lock = fixture
        .resolve(json!({"dependencies":{"plugin":"1.0.0"}}))
        .unwrap();
    let mut nested = lock.packages["node_modules/react"].clone();
    nested.peer = false;
    lock.packages
        .insert("node_modules/plugin/node_modules/react".into(), nested);
    assert!(lock.validate().is_err());
}
