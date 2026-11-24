//! What a change does in dev (spec 6.1), computed by comparing two pipeline outputs.
//!
//! | Change                                               | Effect                                 |
//! |------------------------------------------------------|----------------------------------------|
//! | control or layout in a view resource                 | `dev.refresh` for that form            |
//! | form class or model code                             | backend reload (6.2)                   |
//! | live form property, menu, tray                       | host applies the native setter         |
//! | creation-time form property, form added or removed   | that window is recreated               |
//! | `<application>` identity, permissions, `package.json`| `dev.restart`                          |
//!
//! Frontend file conventions: `forms/<Form>.*` and `forms/<Form>/**` belong to one form;
//! `*.css` is a stylesheet (`dev.css`); any other frontend file is shared, and a change to
//! it reloads every WebView (`dev.reload`).

use crate::manifest::CREATION_PROPS;
use crate::pipeline::BuildOutput;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The reactions one rebuild needs, in the order the host applies them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Restart the whole app; every other field is moot.
    pub restart: bool,
    /// Forms that are new: create their windows.
    pub added: Vec<String>,
    /// Forms that are gone: close their windows.
    pub removed: Vec<String>,
    /// Forms whose creation-time properties changed: recreate their windows.
    pub recreate: BTreeSet<String>,
    /// Forms whose live properties changed: form -> new values (`null` = removed).
    pub live: BTreeMap<String, Map<String, Value>>,
    /// Menus changed: rebuild them.
    pub menus: bool,
    /// Tray icons changed: rebuild them.
    pub trays: bool,
    /// Timers or actions changed: reread them.
    pub timers: bool,
    /// Forms to `dev.refresh`.
    pub refresh: BTreeSet<String>,
    /// A stylesheet changed: `dev.css`.
    pub css: bool,
    /// A shared frontend file changed: `dev.reload` every WebView.
    pub reload_all: bool,
    /// Backend artifact or limits changed: reload the backend.
    pub backend: bool,
}

impl Plan {
    /// Whether the rebuild changed nothing.
    ///
    /// # Returns
    /// `true` when no reaction is needed.
    pub fn is_empty(&self) -> bool {
        *self == Plan::default()
    }
}

/// Classifies the difference between the running build and a new one.
///
/// # Arguments
/// * `old` - build the app runs.
/// * `new` - fresh pipeline output.
///
/// # Returns
/// The reactions.
pub fn classify(old: &BuildOutput, new: &BuildOutput) -> Plan {
    let (a, b) = (&old.config, &new.config);
    let mut plan = Plan::default();
    if a.application != b.application || a.permissions != b.permissions || a.documents != b.documents || old.package_hash != new.package_hash
    {
        plan.restart = true;
        return plan;
    }
    for form in &b.forms {
        let Some(prev) = a.form(&form.name) else {
            plan.added.push(form.name.clone());
            continue;
        };
        let keys: BTreeSet<&String> = prev.props.keys().chain(form.props.keys()).collect();
        let changed: Vec<&String> = keys.into_iter().filter(|k| prev.props.get(*k) != form.props.get(*k)).collect();
        if changed.iter().any(|k| CREATION_PROPS.contains(&k.as_str())) {
            plan.recreate.insert(form.name.clone());
        } else if !changed.is_empty() {
            let values = changed.into_iter().map(|k| (k.clone(), form.props.get(k).cloned().unwrap_or(Value::Null))).collect();
            plan.live.insert(form.name.clone(), values);
        }
        if prev.controls != form.controls {
            plan.refresh.insert(form.name.clone());
        }
    }
    plan.removed = a.forms.iter().filter(|f| b.form(&f.name).is_none()).map(|f| f.name.clone()).collect();
    plan.menus = a.menus != b.menus;
    plan.trays = a.trays != b.trays;
    plan.timers = a.timers != b.timers || a.actions != b.actions;
    plan.backend = old.backend != new.backend || a.limits != b.limits;

    let paths: BTreeSet<&String> = old.files.keys().chain(new.files.keys()).collect();
    for path in paths.into_iter().filter(|p| old.files.get(*p) != new.files.get(*p)) {
        if path.ends_with(".css") {
            plan.css = true;
        } else if let Some(form) = b.forms.iter().chain(&a.forms).find(|f| owns(&f.name, path)) {
            plan.refresh.insert(form.name.clone());
        } else {
            plan.reload_all = true;
        }
    }
    // A form that is recreated or new loads fresh; refreshing it as well is redundant.
    for name in plan.recreate.iter().chain(&plan.added).chain(&plan.removed) {
        plan.refresh.remove(name);
    }
    plan
}

/// Whether a frontend path belongs to one form.
///
/// # Arguments
/// * `form` - form name.
/// * `path` - frontend file path.
///
/// # Returns
/// `true` for `forms/<form>.*` and `forms/<form>/**`.
fn owns(form: &str, path: &str) -> bool {
    path.strip_prefix("forms/")
        .and_then(|rest| rest.strip_prefix(form))
        .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::tests::sample_build;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn same_build_needs_nothing() {
        assert!(classify(&sample_build(), &sample_build()).is_empty());
    }

    #[test]
    fn view_change_refreshes_only_that_form() {
        let old = sample_build();
        let mut new = old.clone();
        new.files.insert("forms/MainForm.js".into(), Arc::from(&b"changed"[..]));
        let plan = classify(&old, &new);
        assert_eq!(plan.refresh.iter().collect::<Vec<_>>(), vec!["MainForm"]);
        assert!(!plan.backend && !plan.reload_all && !plan.restart);

        let mut new = old.clone();
        new.config.forms[0].controls.remove("StatusBar");
        assert!(classify(&old, &new).refresh.contains("MainForm"));
    }

    #[test]
    fn backend_change_reloads_backend() {
        let old = sample_build();
        let mut new = old.clone();
        new.backend = Arc::from("export {};");
        let plan = classify(&old, &new);
        assert!(plan.backend && plan.refresh.is_empty());
    }

    #[test]
    fn live_and_creation_form_properties() {
        let old = sample_build();
        let mut new = old.clone();
        new.config.forms[0].props.insert("caption".into(), json!("Renamed"));
        let plan = classify(&old, &new);
        assert_eq!(plan.live["MainForm"]["caption"], json!("Renamed"));
        assert!(plan.recreate.is_empty());

        let mut new = old.clone();
        new.config.forms[0].props.insert("width".into(), json!(400));
        let plan = classify(&old, &new);
        assert!(plan.recreate.contains("MainForm") && plan.live.is_empty());
    }

    #[test]
    fn identity_permissions_and_package_restart() {
        let old = sample_build();
        let mut new = old.clone();
        new.config.application.name = "Other".into();
        assert!(classify(&old, &new).restart);
        let mut new = old.clone();
        new.config.permissions.native.push("shell.*".into());
        assert!(classify(&old, &new).restart);
        let mut new = old.clone();
        new.package_hash = "x".into();
        assert!(classify(&old, &new).restart);
    }

    #[test]
    fn css_shared_files_menus_and_forms() {
        let old = sample_build();
        let mut new = old.clone();
        new.files.insert("vcl.css".into(), Arc::from(&b"x"[..]));
        new.files.insert("vcl.js".into(), Arc::from(&b"x"[..]));
        new.config.menus.clear();
        let mut extra = new.config.forms[0].clone();
        extra.name = "AboutForm".into();
        new.config.forms.push(extra);
        let plan = classify(&old, &new);
        assert!(plan.css && plan.reload_all && plan.menus);
        assert_eq!(plan.added, vec!["AboutForm".to_owned()]);
    }

    #[test]
    fn form_ownership_is_by_exact_name() {
        assert!(owns("Main", "forms/Main.js"));
        assert!(owns("Main", "forms/Main/part.js"));
        assert!(!owns("Main", "forms/MainForm.js"));
    }
}
