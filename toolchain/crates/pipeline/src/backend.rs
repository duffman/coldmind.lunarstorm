//! Backend generation (spec 4c): the form classes bound to generated control
//! tables, bundled into the one ES module the host loads as `backend.js`.
//!
//! Decisions:
//! - The generated part is one entry module: it imports each form class and
//!   calls the backend runtime's `start` with, per form, the control table
//!   (kind and initial values of every control) and the event connections. The
//!   proxies themselves are built by the runtime from that table, so there is no
//!   per-control generated code to keep in step with the VCL.
//! - Initial values are the view resource's literals over the kind's defaults,
//!   for every prop of the kind. A form class can therefore read any prop of a
//!   control before it was ever written (`button.enabled` is `true`, not
//!   `undefined`).
//! - The developer's TypeScript and the runtime are bundled with rolldown, which
//!   is already the toolchain's bundler; `@desktop/bridge` and `@desktop/api`
//!   stay external because the host provides them.
//! - The runtime source is embedded in this library, so a prebuilt debug runtime
//!   builds a backend without any files of the framework on disk (spec 6).

use std::fs;
use std::path::{Path, PathBuf};

use lunarstorm_model::schema::{self, PropSpec, Type};
use lunarstorm_model::{Element, Project, Value};
use rolldown::{BundlerBuilder, BundlerOptions, InputItem, OutputFormat, Platform, ResolveOptions};
use rolldown_common::Output;
use serde_json::{json, Map, Value as Json};

use crate::host::{literal, APPLICATION_OWNER};

/// The backend runtime, `@desktop/forms`.
const RUNTIME: &str = include_str!("../../../../framework/desktop/forms.ts");

/// The module the form classes import the runtime as.
pub const RUNTIME_MODULE: &str = "@desktop/forms";

/// The modules the host provides; never bundled.
pub const HOST_MODULES: [&str; 2] = ["@desktop/bridge", "@desktop/api"];

/// The value a prop has when the view resource does not set it.
///
/// # Arguments
/// * `spec` - the prop.
///
/// # Returns
/// The default: Delphi's where it has one (`enabled`, `visible`, `itemIndex`
/// -1, a one-second `interval`), otherwise the empty value of the type.
pub fn default_value(spec: &PropSpec) -> Json {
    match (spec.name, spec.ty) {
        ("enabled" | "visible" | "wordWrap", Type::Bool) => json!(true),
        (_, Type::Bool) => json!(false),
        ("itemIndex", Type::Number) => json!(-1),
        ("max", Type::Number) => json!(100),
        ("interval", Type::Number) => json!(1000),
        (_, Type::Number) => json!(0),
        ("align", Type::Choice(_)) => json!("none"),
        ("layout", Type::Choice(_)) => json!("column"),
        ("alignment", Type::Choice(_)) => json!("left"),
        (_, Type::Text | Type::Choice(_) | Type::Color | Type::Ref(_)) => json!(""),
        (_, Type::Texts | Type::Choices(_) | Type::Rows | Type::Nodes) => json!([]),
    }
}

/// Builds the control table of one owner: name to `[kind, initial values]` for
/// every named element under `root`.
///
/// # Arguments
/// * `root` - the `<form>` or `<application>` element.
///
/// # Returns
/// The table, in document order.
fn controls(root: &Element) -> Map<String, Json> {
    let mut table = Map::new();
    root.walk(&mut |element| {
        if std::ptr::eq(element, root) || element.name.is_empty() {
            return;
        }
        let Some(kind) = schema::lookup(element.tag) else { return };
        let mut values = Map::new();
        for spec in kind.all_props() {
            let value = match element.prop(spec.name) {
                Some(value) => literal(value),
                None => default_value(spec),
            };
            values.insert(spec.name.to_string(), value);
        }
        table.insert(element.name.clone(), json!([element.tag, values]));
    });
    table
}

/// Builds the event connections of one owner: `<target>.<event>` to the handler
/// names, and the action links.
///
/// # Arguments
/// * `root` - the `<form>` or `<application>` element.
/// * `owner` - the target name of the root's own events.
///
/// # Returns
/// The connections, and action name to the controls linked to it.
fn connections(root: &Element, owner: &str) -> (Map<String, Json>, Map<String, Json>) {
    let mut executes: Vec<(&str, &str)> = Vec::new();
    root.walk(&mut |element| {
        if let (true, Some(Value::Handler(handler))) = (element.tag == "action", element.prop("onExecute")) {
            executes.push((element.name.as_str(), handler.as_str()));
        }
    });

    let mut connected: Map<String, Json> = Map::new();
    let mut links: Map<String, Json> = Map::new();
    root.walk(&mut |element| {
        let target = if std::ptr::eq(element, root) { owner } else { element.name.as_str() };
        for prop in &element.props {
            if let Value::Handler(handler) = &prop.value {
                connected.insert(format!("{target}.{}", prop.name), json!([handler]));
            }
        }
        if let Some(Value::Text(action)) = element.prop("action") {
            match links.get_mut(action).and_then(Json::as_array_mut) {
                Some(list) => list.push(json!(element.name)),
                None => {
                    links.insert(action.clone(), json!([element.name]));
                }
            }
            // A click on a linked control runs the action after its own handler.
            if let Some((_, execute)) = executes.iter().find(|(name, _)| name == action) {
                let key = format!("{target}.onClick");
                match connected.get_mut(&key).and_then(Json::as_array_mut) {
                    Some(handlers) => handlers.push(json!(execute)),
                    None => {
                        connected.insert(key, json!([execute]));
                    }
                }
            }
        }
    });
    (connected, links)
}

/// Generates the backend entry module of a project.
///
/// # Arguments
/// * `project` - the extracted model.
/// * `root` - the project directory, to import the form classes from.
///
/// # Returns
/// The module's JavaScript source.
pub fn entry(project: &Project, root: &Path) -> String {
    let mut source = String::from("// Generated by the build pipeline (spec 4c). Do not edit.\n");
    source.push_str(&format!("import {{ start }} from {};\n", json!(RUNTIME_MODULE)));
    for (index, form) in project.forms.iter().enumerate() {
        let path = root.join(&form.class);
        source.push_str(&format!("import * as module{index} from {};\n", json!(path.to_string_lossy())));
    }

    let mut forms = String::new();
    for (index, form) in project.forms.iter().enumerate() {
        let (connected, links) = connections(&form.root, &form.name);
        // The class is the export with the form's name, or the default export.
        forms.push_str(&format!(
            "    {name}: {{\n      create: module{index}[{name}] ?? module{index}.default,\n      controls: {controls},\n      connections: {connected},\n      actions: {links},\n    }},\n",
            name = json!(form.name),
            controls = Json::Object(controls(&form.root)),
            connected = Json::Object(connected),
            links = Json::Object(links),
        ));
    }
    let (application, _) = connections(&project.application, APPLICATION_OWNER);
    let main = project.application.prop("mainForm").map(literal).unwrap_or(Json::Null);
    source.push_str(&format!(
        "\nstart({{\n  application: {{ mainForm: {main}, connections: {} }},\n  forms: {{\n{forms}  }},\n}});\n",
        Json::Object(application)
    ));
    source
}

/// A directory for the files of one bundling run, removed when dropped.
struct WorkDir(PathBuf);

impl WorkDir {
    /// Creates the directory.
    ///
    /// # Returns
    /// The directory.
    ///
    /// # Errors
    /// The directory cannot be created.
    fn new() -> std::io::Result<Self> {
        // Fix: the name was the process id and the clock alone. Two builds on
        // different threads of one process could read the same clock value, get
        // the same directory, and one then bundled the other project's entry.
        // Decision: a process-wide sequence number makes the name unique within
        // the process, and `create_dir` (not `create_dir_all`) turns any name
        // that already exists into an error instead of a shared directory.
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_nanos());
        let path = std::env::temp_dir().join(format!("lunarstorm-backend-{}-{unique}-{sequence}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for WorkDir {
    /// Removes the directory and the two small files in it.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Generates and bundles the backend artifact of a project.
///
/// # Arguments
/// * `project` - the extracted model.
/// * `root` - the project directory.
///
/// # Returns
/// The source of `backend.js`: one ES module importing only the host's modules.
///
/// # Errors
/// The bundler's diagnostics, as text, when the project's code does not bundle
/// (a syntax error in a form class or model, an import that does not resolve).
pub fn generate(project: &Project, root: &Path) -> Result<String, String> {
    // Fix: a relative project root (`lunarstorm-build starters/desktop out`) put
    // relative class paths in the entry, which the bundler resolved against the
    // work directory and reported as "Module not found". The root is made
    // absolute here, at the one place the entry and the bundler's cwd come from.
    let root = &root.canonicalize().map_err(|error| format!("{}: {error}", root.display()))?;
    let work = WorkDir::new().map_err(|error| format!("cannot create a work directory: {error}"))?;
    let entry_path = work.0.join("entry.js");
    // Decision: the runtime is written as `forms.ts` so the bundler transpiles it; the
    // alias below points the `@desktop/forms` import at this file.
    let runtime_path = work.0.join("forms.ts");
    fs::write(&entry_path, entry(project, root)).map_err(|error| error.to_string())?;
    fs::write(&runtime_path, RUNTIME).map_err(|error| error.to_string())?;

    let options = BundlerOptions {
        input: Some(vec![InputItem { name: Some("backend".into()), import: entry_path.to_string_lossy().into_owned() }]),
        cwd: Some(root.to_path_buf()),
        format: Some(OutputFormat::Esm),
        platform: Some(Platform::Neutral),
        external: Some(HOST_MODULES.iter().map(|module| module.to_string()).collect::<Vec<_>>().into()),
        resolve: Some(ResolveOptions {
            alias: Some(vec![(RUNTIME_MODULE.to_string(), vec![Some(runtime_path.to_string_lossy().into_owned())])]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .map_err(|error| format!("cannot start the bundler: {error}"))?;
    runtime.block_on(async {
        let mut bundler =
            BundlerBuilder::default().with_options(options).build().map_err(|error| format!("bundler setup failed: {error:?}"))?;
        let output = bundler.generate().await.map_err(|errors| {
            errors.iter().map(|error| error.to_diagnostic().convert_to_string(false)).collect::<Vec<_>>().join("\n")
        })?;
        let _ = bundler.close().await;
        if let Some(warning) = output.warnings.iter().find(|warning| warning.kind().to_string() == "UNRESOLVED_IMPORT") {
            return Err(warning.to_diagnostic().convert_to_string(false));
        }
        let mut chunks = output.assets.iter().filter_map(|asset| match asset {
            Output::Chunk(chunk) => Some(chunk),
            Output::Asset(_) => None,
        });
        match (chunks.next(), chunks.next()) {
            (Some(chunk), None) => Ok(chunk.code.clone()),
            // The host loads exactly one module; a dynamic import would need a second.
            _ => Err("the backend must bundle into one module; remove dynamic imports from form classes and models".into()),
        }
    })
}
