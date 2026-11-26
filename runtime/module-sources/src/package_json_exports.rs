//! `package.json`: the fields module resolution reads, and the `exports` and
//! `imports` mappings with their conditions and subpath patterns.
//!
//! Ported from jetbunny (`src/platform/package/manifest.rs`), with these changes:
//! - A condition is chosen by the mapping's own key order, as Node does, not by a
//!   fixed priority list. Decision: the JSON is read into [`Json`], which keeps key
//!   order, rather than enabling serde_json's `preserve_order` (a workspace-wide
//!   feature that would change every map in the build).
//! - `name` and `version` are optional and dependency groups are not read:
//!   packages are found by Node's `node_modules` lookup, not a declared closure.
//! - Array targets (fallback lists) are accepted; the first valid entry is used.

use crate::Error;

/// Export conditions of an `import`.
pub const IMPORT: &[&str] = &["lunarstorm", "node", "import", "default"];

/// Export conditions of a `require()`.
pub const REQUIRE: &[&str] = &["lunarstorm", "node", "require", "default"];

/// A JSON value with object keys in document order.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// A string.
    Str(String),
    /// An array.
    Array(Vec<Json>),
    /// An object, keys in document order.
    Object(Vec<(String, Json)>),
    /// A boolean or a number: never a valid mapping target.
    Other,
}

impl Json {
    /// # Returns
    /// The entry under `key`, for an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Parses JSON keeping object key order.
///
/// # Errors
/// The text is not JSON.
fn ordered(bytes: &[u8]) -> Result<Json, String> {
    use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
    use std::fmt;

    /// Deserializes into [`Json`].
    struct Ordered(Json);

    impl<'de> serde::Deserialize<'de> for Ordered {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            /// The visitor building [`Json`].
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Ordered;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("JSON")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Ordered, A::Error> {
                    let mut entries = Vec::new();
                    while let Some(key) = a.next_key::<String>()? {
                        let Ordered(value) = a.next_value()?;
                        // Node keeps the last of duplicate keys, at the first one's place.
                        match entries.iter_mut().find(|(k, _): &&mut (String, Json)| *k == key) {
                            Some(entry) => entry.1 = value,
                            None => entries.push((key, value)),
                        }
                    }
                    Ok(Ordered(Json::Object(entries)))
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Ordered, A::Error> {
                    let mut items = Vec::new();
                    while let Some(Ordered(item)) = a.next_element()? {
                        items.push(item);
                    }
                    Ok(Ordered(Json::Array(items)))
                }
                fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Ordered, E> {
                    Ok(Ordered(Json::Str(s.into())))
                }
                fn visit_string<E: serde::de::Error>(self, s: String) -> Result<Ordered, E> {
                    Ok(Ordered(Json::Str(s)))
                }
                fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Ordered, E> {
                    Ok(Ordered(Json::Other))
                }
                fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Ordered, E> {
                    Ok(Ordered(Json::Other))
                }
                fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Ordered, E> {
                    Ok(Ordered(Json::Other))
                }
                fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Ordered, E> {
                    Ok(Ordered(Json::Other))
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Ordered, E> {
                    Ok(Ordered(Json::Null))
                }
            }
            d.deserialize_any(V)
        }
    }

    serde_json::from_slice::<Ordered>(bytes).map(|Ordered(json)| json).map_err(|e| e.to_string())
}

/// What `type` says about a package's `.js` and `.ts` files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    /// `"module"`: ES modules.
    Module,
    /// `"commonjs"`: CommonJS.
    CommonJs,
    /// No `type`: decided per file by its syntax.
    Unset,
}

/// The fields of a `package.json` that module resolution reads.
#[derive(Clone, Debug)]
pub struct Manifest {
    /// `name`, if any.
    pub name: Option<String>,
    /// `type`.
    pub kind: Type,
    /// `main`, if any.
    pub main: Option<String>,
    /// `exports`, if any; validated.
    pub exports: Option<Json>,
    /// `imports`, if any; validated.
    pub imports: Option<Json>,
}

impl Manifest {
    /// Parses and validates a `package.json`.
    ///
    /// # Errors
    /// Not a JSON object, or an `exports`/`imports` mapping that is malformed or
    /// whose targets leave the package.
    pub fn parse(bytes: &[u8]) -> Result<Manifest, Error> {
        let invalid = |message: String| Error::Invalid(format!("package.json: {message}"));
        let json = ordered(bytes).map_err(invalid)?;
        if !matches!(json, Json::Object(_)) {
            return Err(invalid(String::from("must be an object")));
        }
        let string = |key: &str| match json.get(key) {
            Some(Json::Str(s)) => Some(s.clone()),
            _ => None,
        };
        let kind = match string("type").as_deref() {
            Some("module") => Type::Module,
            Some("commonjs") => Type::CommonJs,
            _ => Type::Unset,
        };
        let exports = json.get("exports").cloned();
        if let Some(exports) = &exports {
            validate_exports(exports)?;
        }
        let imports = json.get("imports").cloned();
        if let Some(imports) = &imports {
            let Json::Object(entries) = imports else {
                return Err(invalid(String::from("imports must be a mapping")));
            };
            for (key, target) in entries {
                if !key.starts_with('#') || key == "#" || key.starts_with("#/") {
                    return Err(invalid(format!("invalid imports key {key}")));
                }
                validate_target(target, true, key.contains('*'), 0)?;
            }
        }
        Ok(Manifest { name: string("name"), kind, main: string("main"), exports, imports })
    }

    /// The package-relative target of a subpath (`.` or `./x`).
    ///
    /// Without `exports`, `.` is `main` (else `index.js`) and any other subpath
    /// names its own file.
    ///
    /// # Errors
    /// [`Error::NotFound`] when `exports` does not export the subpath.
    pub fn export_target(&self, subpath: &str, conditions: &[&str]) -> Result<String, Error> {
        match &self.exports {
            Some(exports) => {
                let target = match exports {
                    Json::Object(entries) if entries.iter().any(|(k, _)| k.starts_with('.')) => {
                        match exports.get(subpath) {
                            Some(target) => select(target, conditions, None),
                            None => pattern(entries, subpath, conditions),
                        }
                    }
                    sugar if subpath == "." => select(sugar, conditions, None),
                    _ => None,
                };
                target.ok_or(Error::NotFound)
            }
            None if subpath == "." => Ok(self.main.clone().unwrap_or_else(|| String::from("index.js"))),
            None => Ok(subpath.strip_prefix("./").unwrap_or(subpath).to_owned()),
        }
    }

    /// The target of a private `#name` import: a `./` path in this package, or a
    /// bare specifier to resolve as a package import.
    ///
    /// # Errors
    /// [`Error::NotFound`] when `imports` does not map the name.
    pub fn import_target(&self, specifier: &str, conditions: &[&str]) -> Result<String, Error> {
        let Some(Json::Object(entries)) = &self.imports else {
            return Err(Error::NotFound);
        };
        match entries.iter().find(|(k, _)| k == specifier) {
            Some((_, target)) => select(target, conditions, None),
            None => pattern(entries, specifier, conditions),
        }
        .ok_or(Error::NotFound)
    }
}

/// Selects a target through conditions: the first key, in the mapping's own order,
/// that is one of `conditions` and yields a target. A `*` in the result takes
/// `capture`.
///
/// # Returns
/// `None` when nothing is selected, including an explicit `null`.
fn select(target: &Json, conditions: &[&str], capture: Option<&str>) -> Option<String> {
    resolve_target(target, conditions, capture).flatten()
}

/// Node's PACKAGE_TARGET_RESOLVE.
///
/// Fix: the first matching condition ended the search even when nothing inside
/// it matched (`{"node": {"import": ...}, "default": ...}` under `require` gave
/// nothing). Decision: as in Node, an unmatched condition continues with the next
/// key; only an explicit `null` stops.
///
/// # Returns
/// `None` when nothing matched (keep looking), `Some(None)` for `null` (excluded),
/// `Some(Some(target))` when found.
fn resolve_target(target: &Json, conditions: &[&str], capture: Option<&str>) -> Option<Option<String>> {
    match target {
        Json::Str(s) => Some(Some(match capture {
            Some(capture) => s.replace('*', capture),
            None => s.clone(),
        })),
        Json::Null => Some(None),
        Json::Object(entries) => entries
            .iter()
            .filter(|(key, _)| conditions.contains(&key.as_str()))
            .find_map(|(_, value)| resolve_target(value, conditions, capture)),
        Json::Array(items) => items.iter().find_map(|item| resolve_target(item, conditions, capture).flatten()).map(Some),
        Json::Other => None,
    }
}

/// The target of a subpath through a pattern key.
///
/// Node's PATTERN_KEY_COMPARE: among the keys whose text before the `*` starts
/// the subpath and whose text after it ends it, the longest text before the `*`
/// wins, then the longest key. The capture (at least one character, slashes
/// included) replaces every `*` of the selected target.
fn pattern(entries: &[(String, Json)], subpath: &str, conditions: &[&str]) -> Option<String> {
    let mut best: Option<(&str, &str, &Json)> = None;
    for (key, value) in entries {
        let Some((before, after)) = key.split_once('*') else { continue };
        if subpath.len() > before.len() + after.len()
            && subpath.starts_with(before)
            && subpath.ends_with(after)
            && best.is_none_or(|(b, a, _)| (before.len(), before.len() + after.len()) > (b.len(), b.len() + a.len()))
        {
            best = Some((before, after, value));
        }
    }
    let (before, after, value) = best?;
    select(value, conditions, Some(&subpath[before.len()..subpath.len() - after.len()]))
}

/// Checks `exports`: either subpath keys (`.`, `./x`, `./x/*`) or one target.
///
/// # Errors
/// Mixed subpath and condition keys, a key with more than one `*`, or a bad target.
fn validate_exports(exports: &Json) -> Result<(), Error> {
    if let Json::Object(entries) = exports
        && entries.iter().any(|(k, _)| k.starts_with('.'))
    {
        for (key, target) in entries {
            let stars = key.matches('*').count();
            let plain = key.replacen('*', "x", 1);
            if stars > 1
                || (key != "."
                    && (!plain.starts_with("./") || plain[2..].split('/').any(|p| p.is_empty() || p == "." || p == "..")))
            {
                return Err(Error::Invalid(format!("package.json: invalid exports key {key}")));
            }
            validate_target(target, false, stars == 1, 0)?;
        }
        return Ok(());
    }
    validate_target(exports, false, false, 0)
}

/// Checks one mapping target through its conditions.
///
/// # Arguments
/// * `target` - a string, `null`, an array or an object of conditions.
/// * `private` - an `imports` target, which may also name a package.
/// * `pattern` - the key holds a `*`, so a `./` target may hold `*`.
/// * `depth` - condition nesting so far.
///
/// # Errors
/// A target that leaves the package, is not a path, or nests deeper than 8.
fn validate_target(target: &Json, private: bool, pattern: bool, depth: usize) -> Result<(), Error> {
    let invalid = |message: &str| Err(Error::Invalid(format!("package.json: {message}")));
    if depth > 8 {
        return invalid("conditions nest deeper than 8");
    }
    match target {
        Json::Null => Ok(()),
        Json::Str(s) if s.starts_with("./") => {
            let s = if pattern { s.replace('*', "x") } else { s.clone() };
            if s.contains(['\\', '\0']) || s.split('/').any(|p| p == ".." || p == "node_modules") {
                return invalid("mapping target leaves the package");
            }
            Ok(())
        }
        Json::Str(s) if private && !s.is_empty() && !s.starts_with(['.', '/']) && !s.contains(['\\', '\0', ':']) => {
            Ok(())
        }
        Json::Str(_) => invalid("invalid mapping target"),
        Json::Array(items) => items.iter().try_for_each(|item| validate_target(item, private, pattern, depth + 1)),
        Json::Object(entries) if !entries.is_empty() => {
            for (key, value) in entries {
                if key.is_empty() || key.starts_with(['.', '#']) {
                    return invalid("mixed subpath and condition keys");
                }
                validate_target(value, private, pattern, depth + 1)?;
            }
            Ok(())
        }
        _ => invalid("mapping must be a string, null, an array or an object of conditions"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// # Returns
    /// A manifest with these `exports`.
    fn with_exports(exports: &str) -> Result<Manifest, Error> {
        Manifest::parse(format!(r#"{{"name":"p","exports":{exports}}}"#).as_bytes())
    }

    #[test]
    fn subpath_patterns_resolve_like_node() {
        let m = with_exports(
            r#"{".":"./dist/index.js",
                "./utils/*":{"import":"./dist/utils/*.js","require":"./dist/cjs/utils/*.js"},
                "./utils/exact":"./dist/exact.js",
                "./utils/internal/*":null,
                "./*":"./dist/*/index.js"}"#,
        )
        .expect("valid");
        let target = |subpath: &str| m.export_target(subpath, IMPORT).ok();
        assert_eq!(target(".").as_deref(), Some("./dist/index.js"));
        assert_eq!(target("./utils/cookie").as_deref(), Some("./dist/utils/cookie.js"));
        assert_eq!(target("./utils/a/b").as_deref(), Some("./dist/utils/a/b.js"));
        assert_eq!(target("./utils/exact").as_deref(), Some("./dist/exact.js"));
        assert_eq!(target("./utils/internal/secret"), None);
        assert_eq!(target("./jsx").as_deref(), Some("./dist/jsx/index.js"));
        assert_eq!(m.export_target("./utils/x", REQUIRE).ok().as_deref(), Some("./dist/cjs/utils/x.js"));
        assert!(with_exports(r#"{"./a/*/b/*":"./x/*.js"}"#).is_err());
        assert!(with_exports(r#"{"./a/*":"./../x/*.js"}"#).is_err());
    }

    #[test]
    fn conditions_follow_the_mappings_key_order() {
        let m = with_exports(r#"{"default":"./d.js","import":"./i.js"}"#).expect("valid");
        assert_eq!(m.export_target(".", IMPORT).ok().as_deref(), Some("./d.js"));
        let m = with_exports(r#"{"types":"./t.d.ts","node":{"import":"./n.mjs"},"default":"./d.js"}"#).expect("valid");
        assert_eq!(m.export_target(".", IMPORT).ok().as_deref(), Some("./n.mjs"));
        assert_eq!(m.export_target(".", REQUIRE).ok().as_deref(), Some("./d.js"));
    }

    #[test]
    fn main_and_private_imports() {
        let m = Manifest::parse(br##"{"main":"lib/main","type":"module","imports":{"#cfg":"./src/cfg.ts","#dep":"other/x","#u/*":"./u/*.js"}}"##)
            .expect("valid");
        assert_eq!(m.kind, Type::Module);
        assert_eq!(m.export_target(".", IMPORT).ok().as_deref(), Some("lib/main"));
        assert_eq!(m.export_target("./a/b.js", IMPORT).ok().as_deref(), Some("a/b.js"));
        assert_eq!(m.import_target("#cfg", IMPORT).ok().as_deref(), Some("./src/cfg.ts"));
        assert_eq!(m.import_target("#dep", IMPORT).ok().as_deref(), Some("other/x"));
        assert_eq!(m.import_target("#u/x", IMPORT).ok().as_deref(), Some("./u/x.js"));
        assert!(m.import_target("#nope", IMPORT).is_err());
        assert!(Manifest::parse(br#"{"imports":{"nohash":"./x.js"}}"#).is_err());
        assert!(Manifest::parse(b"[1]").is_err());
    }
}
