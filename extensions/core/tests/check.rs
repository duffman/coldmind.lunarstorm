//! Names, manifest parsing and the load-time checks.

use lunar_extension::{check, valid_name, Cardinality, Manifest, Offer, PluginHost, Requirement};

/// A host offering `store` (plugin host `backend`: one, required, with `sqlite`
/// and `file`) and `hostapi` (plugin host `area`: many, optional, with `ui`).
fn offers() -> Vec<Offer> {
    vec![
        Offer {
            name: "store",
            plugin_hosts: vec![PluginHost { name: "backend", cardinality: Cardinality::One, requirement: Requirement::Required, plugins: vec!["sqlite", "file"] }],
        },
        Offer {
            name: "hostapi",
            plugin_hosts: vec![PluginHost { name: "area", cardinality: Cardinality::Many, requirement: Requirement::Optional, plugins: vec!["ui"] }],
        },
    ]
}

/// # Returns
/// The messages `check` gives for this manifest text against [`offers`].
fn errors(text: &str) -> Vec<String> {
    let manifest = Manifest::parse(text).expect("the test manifest parses");
    check(&manifest, &offers()).err().unwrap_or_default()
}

#[test]
fn names_follow_the_rule() {
    // The longest valid name has 32 characters: one letter and 31 more.
    for name in ["a", "ui", "hostapi", "store2", "a2345678901234567890123456789012"] {
        assert!(valid_name(name), "{name} is valid");
    }
    for name in ["", "Ui", "2d", "host-api", "host_api", "a.b", "a23456789012345678901234567890123", "é"] {
        assert!(!valid_name(name), "{name:?} is invalid");
    }
}

#[test]
fn reserved_and_invalid_offers_are_reported() {
    let offers = vec![
        Offer { name: "ipc", plugin_hosts: vec![] },
        Offer { name: "Bad", plugin_hosts: vec![] },
        Offer { name: "dup", plugin_hosts: vec![] },
        Offer { name: "dup", plugin_hosts: vec![] },
        Offer {
            name: "two",
            plugin_hosts: vec![
                PluginHost { name: "a", cardinality: Cardinality::Many, requirement: Requirement::Optional, plugins: vec!["x"] },
                PluginHost { name: "b", cardinality: Cardinality::Many, requirement: Requirement::Optional, plugins: vec!["x"] },
            ],
        },
    ];
    let errors = check(Manifest::empty(), &offers).unwrap_err();
    assert_eq!(
        errors,
        [
            "host: extension name ipc is reserved",
            "host: extension name Bad is invalid",
            "host: extension dup is offered twice",
            "host: extension two offers plugin x twice",
        ]
    );
}

#[test]
fn the_empty_manifest_passes() {
    assert_eq!(check(Manifest::empty(), &offers()), Ok(()));
    assert_eq!(check(&Manifest::parse("{}").unwrap(), &offers()), Ok(()));
}

#[test]
fn a_valid_manifest_passes() {
    assert!(errors(r#"{ "extensions": { "store": { "plugins": ["sqlite"], "config": { "file": "s.db" } }, "hostapi": {} } }"#).is_empty());
}

#[test]
fn unknown_extension() {
    assert_eq!(errors(r#"{ "extensions": { "nosuch": {} } }"#), ["manifest: this host has no extension nosuch"]);
}

#[test]
fn unknown_plugin() {
    assert_eq!(errors(r#"{ "extensions": { "hostapi": { "plugins": ["gamma"] } } }"#), ["manifest: extension hostapi has no plugin gamma"]);
}

#[test]
fn plugin_listed_twice() {
    assert_eq!(errors(r#"{ "extensions": { "hostapi": { "plugins": ["ui", "ui"] } } }"#), ["manifest: extension hostapi lists plugin ui twice"]);
}

#[test]
fn required_plugin_host_left_empty() {
    assert_eq!(errors(r#"{ "extensions": { "store": {} } }"#), ["manifest: extension store: plugin host backend requires a plugin"]);
}

#[test]
fn two_plugins_in_a_one_plugin_host() {
    assert_eq!(errors(r#"{ "extensions": { "store": { "plugins": ["sqlite", "file"] } } }"#), ["manifest: extension store: plugin host backend takes one plugin"]);
}

#[test]
fn every_error_is_reported() {
    assert_eq!(
        errors(r#"{ "extensions": { "nosuch": {}, "store": { "plugins": ["nope"] } } }"#),
        ["manifest: this host has no extension nosuch", "manifest: extension store has no plugin nope", "manifest: extension store: plugin host backend requires a plugin"]
    );
}

#[test]
fn unknown_keys_are_refused() {
    let error = Manifest::parse(r#"{ "permissions": {} }"#).unwrap_err();
    assert!(error.starts_with("manifest: unknown field `permissions`"), "{error}");
    let error = Manifest::parse(r#"{ "extensions": { "hostapi": { "plugin": [] } } }"#).unwrap_err();
    assert!(error.starts_with("manifest: unknown field `plugin`"), "{error}");
}

#[test]
fn use_gives_plugins_in_manifest_order_and_config() {
    let manifest = Manifest::parse(r#"{ "extensions": { "hostapi": { "plugins": ["b", "a"], "config": { "k": 1 } } } }"#).unwrap();
    let using = manifest.using("hostapi").expect("hostapi is used");
    assert_eq!(using.plugins(), ["b", "a"]);
    assert!(using.has("a") && !using.has("c"));
    assert_eq!(using.config(), Some(&serde_json::json!({ "k": 1 })));
    assert!(manifest.using("store").is_none());
}
