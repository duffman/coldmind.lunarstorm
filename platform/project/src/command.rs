//! The `project` package of the `lunar` command line: `project init`, `insight`,
//! `compile` and `sign`.
//!
//! Every crate that adds commands to `lunar` exposes them from `src/command.rs` as
//! `command::package()`; `platform/cli/app/src/main.rs` mounts this one behind the
//! `project` feature. The host (`lunarstorm-cli-host`) parses the command line,
//! renders help and dispatches; this module holds the specs and the handlers.
//!
//! ```text
//! lunar project init [dir] --type module|program --mode lunar|node
//! lunar project init [dir] --template desktop/app | <folder>
//! lunar project templates
//! lunar project insight [dir]
//! lunar project compile [dir]            build/<name>.jbn, development key
//! lunar project sign [dir] --key <developer.json>
//! ```
//!
//! Decisions:
//! - The project description is `package.json`: `name`, `version`, `main` (the
//!   entry) and a `lunar` object with `type` and `mode`. No other file is needed.
//! - Templates come from the products, not from this crate: the binary passes each
//!   product's embedded files ([`Templates`]) to [`package`], so this crate names no
//!   product and a binary mounts only the templates it ships. A template folder is
//!   `template.json` (`description`, `type`, `main`) plus the files to write;
//!   `{{name}}` in a UTF-8 file becomes the package name, and `gitignore` is written
//!   as `.gitignore`. `init` writes `package.json` itself from `type`, `main` and
//!   `--mode`, so templates carry no conditionals. `--type` picks `cli/<type>`;
//!   `--template` overrides it with a mounted name or a folder on disk.
//! - A `desktop` project is built by `lunar build -p` (lunarstorm-pipeline), so
//!   `compile` and `sign` refuse it, and `init` refuses `--mode node` for it.
//! - `compile` signs with a per-project development key (`.lunar/dev.der`, made on
//!   first use, owner-only) and writes `build/trust.json` naming it, so
//!   `lunar run --trust build/trust.json build/<name>.jbn` runs the build. A `.jbn`
//!   cannot be unsigned, so there is no unsigned build.
//! - `sign` compiles again and signs with a developer key issued by Lunarstorm
//!   (see [`developer`]); builds are deterministic, so this equals re-signing the
//!   compiled package. It refuses any key whose certificate does not verify
//!   against [`developer::ROOT`].

use crate::developer;
use lunarstorm_cli_contract::{Arg, Command, Failure, Invocation, Package};
use lunarstorm_jbn::{Signer, TrustStore, Trusted};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// One product's embedded templates, as its templates crate exports them:
/// `(product, files)`, each file `("<template>/<path>", bytes)`. A template is named
/// `<product>/<template>` on the command line.
pub type Templates = (&'static str, &'static [(&'static str, &'static [u8])]);

/// The project package.
struct ProjectPackage {
    /// The templates `init` can use, per product.
    templates: &'static [Templates],
}

/// Returns the project package for mounting on the command line.
///
/// # Arguments
/// * `templates` - the products' templates `init` offers; `--type` needs `cli/program`
///   and `cli/module` among them.
///
/// # Returns
/// The package; it exports `project`.
pub fn package(templates: &'static [Templates]) -> Box<dyn Package> {
    Box::new(ProjectPackage { templates })
}

impl Package for ProjectPackage {
    /// # Returns
    /// `"project"`.
    fn id(&self) -> &'static str {
        "project"
    }

    /// # Returns
    /// The project tools' line on the top-level help screen.
    fn headline(&self) -> &'static str {
        "create, inspect, compile and sign projects"
    }

    /// # Returns
    /// `project` with `init`, `insight`, `compile` and `sign`.
    fn commands(&self) -> Vec<Command> {
        let directory = || Arg::positional("dir").value(lunarstorm_cli_contract::ValueType::Path).default(".").about("the project directory");
        let templates = self.templates;
        let release = || Arg::option("release").value(lunarstorm_cli_contract::ValueType::Int).default("1").about("release number trust stores compare");
        vec![Command::new("project", "create, inspect, compile and sign a Lunarstorm project")
            .child(
                Command::new("init", "create a project: package.json, an entry file and .gitignore")
                    .arg(directory())
                    .arg(Arg::option("type").choices(&["module", "program"]).default("program").about("a program to run, or a module to import"))
                    .arg(Arg::option("mode").choices(&["lunar", "node"]).default("lunar").about("the runtime it targets"))
                    .arg(Arg::option("template").placeholder("<product/name|folder>").about("a template from `lunar project templates`, or a template folder; overrides --type"))
                    .run(move |invocation| init(invocation, templates))
                    .example("lunar project init hello --type program --mode lunar")
                    .example("lunar project init notes --template desktop/app"),
            )
            .child(Command::new("templates", "list the templates init can use").run(move |_| list(templates)))
            .child(Command::new("insight", "summarize a project: description, dependencies, files and build").arg(directory()).run(insight))
            .child(
                Command::new("compile", "compile to runtime bytecode: build/<name>.jbn, signed with a development key")
                    .arg(directory())
                    .arg(release())
                    .run(compile)
                    .example("lunar project compile && lunar run --trust build/trust.json build/hello.jbn"),
            )
            .child(
                Command::new("sign", "compile and sign build/<name>.jbn with a developer key issued by Lunarstorm")
                    .arg(directory())
                    .arg(release())
                    .arg(Arg::option("key").value(lunarstorm_cli_contract::ValueType::Path).placeholder("<developer.json>").env("LUNARSTORM_DEVELOPER_KEY").required().about("your issued developer key"))
                    .run(sign)
                    .example("lunar project sign --key ~/.lunarstorm/developer.json"),
            )]
    }
}

/// A project template: what `init` writes besides `package.json`.
struct Template {
    /// `program`, `module` or `desktop`: `lunar.type` in `package.json`.
    kind: String,
    /// The entry file: `main` in `package.json`.
    main: String,
    /// The files to write, by path relative to the project; `template.json` left out.
    files: Vec<(String, Vec<u8>)>,
}

impl Template {
    /// Builds a template from its files.
    ///
    /// # Arguments
    /// * `files` - every file of the template folder, `template.json` included.
    /// * `origin` - the template's name or folder, for errors.
    ///
    /// # Returns
    /// The template.
    ///
    /// # Errors
    /// No or malformed `template.json`, an unknown `type`, or no file for `main`.
    fn parse(files: Vec<(String, Vec<u8>)>, origin: &str) -> Result<Template, Failure> {
        let (meta, files): (Vec<_>, Vec<_>) = files.into_iter().partition(|(path, _)| path == "template.json");
        let (_, meta) = meta.into_iter().next().ok_or_else(|| Failure::failed(format!("{origin}: no template.json")))?;
        let json: Value = serde_json::from_slice(&meta).map_err(|e| Failure::failed(format!("{origin}/template.json: {e}")))?;
        let field = |key: &str| json.get(key).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| Failure::failed(format!("{origin}/template.json has no \"{key}\"")));
        let (kind, main) = (field("type")?, field("main")?);
        if !matches!(kind.as_str(), "program" | "module" | "desktop") {
            return Err(Failure::failed(format!("{origin}/template.json: type {kind:?} is not program, module or desktop")));
        }
        if !files.iter().any(|(path, _)| *path == main) {
            return Err(Failure::failed(format!("{origin}: no file {main} for \"main\"")));
        }
        Ok(Template { kind, main, files })
    }

    /// Finds a mounted template.
    ///
    /// # Arguments
    /// * `templates` - the mounted products' templates.
    /// * `name` - `<product>/<template>`.
    ///
    /// # Returns
    /// The template.
    ///
    /// # Errors
    /// No such template (the error lists the mounted ones), or a broken one.
    fn mounted(templates: &[Templates], name: &str) -> Result<Template, Failure> {
        let found = name.split_once('/').and_then(|(product, template)| {
            let (_, files) = templates.iter().find(|(p, _)| *p == product)?;
            let prefix = format!("{template}/");
            let files: Vec<(String, Vec<u8>)> = files.iter().filter_map(|(path, bytes)| path.strip_prefix(prefix.as_str()).map(|p| (p.to_owned(), bytes.to_vec()))).collect();
            (!files.is_empty()).then_some(files)
        });
        match found {
            Some(files) => Template::parse(files, name),
            None => {
                let names: Vec<String> = catalog(templates).into_iter().map(|(name, _)| name).collect();
                Err(Failure::failed(format!("no template {name}; available: {}", if names.is_empty() { String::from("none") } else { names.join(", ") })))
            }
        }
    }

    /// Reads a template folder from disk; names starting with `.` are skipped, as
    /// the templates crates' `build.rs` skips them.
    ///
    /// # Arguments
    /// * `folder` - the template folder.
    ///
    /// # Returns
    /// The template.
    ///
    /// # Errors
    /// An unreadable file, or a broken template.
    fn folder(folder: &Path) -> Result<Template, Failure> {
        /// Collects the files under `dir` as paths relative to `root`.
        fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), Failure> {
            for entry in fs::read_dir(dir).map_err(io(dir))?.flatten() {
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    walk(root, &path, out)?;
                } else {
                    let relative = path.strip_prefix(root).expect("under the folder").to_string_lossy().replace('\\', "/");
                    out.push((relative, fs::read(&path).map_err(io(&path))?));
                }
            }
            Ok(())
        }
        let mut files = Vec::new();
        walk(folder, folder, &mut files)?;
        Template::parse(files, &folder.display().to_string())
    }
}

/// Lists the mounted templates.
///
/// # Arguments
/// * `templates` - the mounted products' templates.
///
/// # Returns
/// `(<product>/<template>, description)` per template, sorted by name.
fn catalog(templates: &[Templates]) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = templates
        .iter()
        .flat_map(|(product, files)| {
            files.iter().filter_map(move |(path, bytes)| {
                let template = path.strip_suffix("/template.json").filter(|t| !t.contains('/'))?;
                let json: Value = serde_json::from_slice(bytes).ok()?;
                Some((format!("{product}/{template}"), json.get("description").and_then(Value::as_str).unwrap_or("").to_owned()))
            })
        })
        .collect();
    list.sort();
    list
}

/// The `templates` handler: prints each mounted template and its description.
///
/// # Arguments
/// * `templates` - the mounted products' templates.
///
/// # Returns
/// `0`.
///
/// # Errors
/// None; the signature is the handler's.
fn list(templates: &[Templates]) -> Result<i32, Failure> {
    for (name, description) in catalog(templates) {
        println!("{name:<16}{description}");
    }
    Ok(0)
}

/// The project description read from `package.json`.
struct Project {
    /// The project directory.
    root: PathBuf,
    /// `package.json`, parsed.
    json: Value,
}

impl Project {
    /// Reads `<root>/package.json`.
    ///
    /// # Arguments
    /// * `root` - the project directory.
    ///
    /// # Returns
    /// The project.
    ///
    /// # Errors
    /// No `package.json`, or not JSON.
    fn open(root: &Path) -> Result<Project, Failure> {
        let file = root.join("package.json");
        let text = fs::read(&file).map_err(|_| Failure::failed(format!("no package.json in {}; create one with `lunar project init`", root.display())))?;
        let json = serde_json::from_slice(&text).map_err(|e| Failure::failed(format!("{}: {e}", file.display())))?;
        Ok(Project { root: root.to_path_buf(), json })
    }

    /// # Arguments
    /// * `key` - a top-level string field.
    ///
    /// # Returns
    /// Its value, if it is a string.
    fn text(&self, key: &str) -> Option<&str> {
        self.json.get(key).and_then(Value::as_str)
    }

    /// Refuses a desktop project, which `lunar build` builds instead of `compile`.
    ///
    /// # Errors
    /// `lunar.type` is `desktop`.
    fn packable(&self) -> Result<(), Failure> {
        match self.json.get("lunar").and_then(|l| l.get("type")).and_then(Value::as_str) {
            Some("desktop") => Err(Failure::failed(format!("{} is a desktop project; build it with `lunar build -p {}`", self.root.display(), self.root.display()))),
            _ => Ok(()),
        }
    }

    /// # Returns
    /// `name`, or the error a build reports without it.
    fn name(&self) -> Result<&str, Failure> {
        self.text("name").ok_or_else(|| Failure::failed("package.json has no \"name\""))
    }

    /// # Returns
    /// The entry file named by `main`.
    fn entry(&self) -> Result<PathBuf, Failure> {
        self.text("main").map(|main| self.root.join(main)).ok_or_else(|| Failure::failed("package.json has no \"main\" entry"))
    }

    /// # Returns
    /// `build/<name>.jbn`.
    fn output(&self) -> Result<PathBuf, Failure> {
        Ok(self.root.join("build").join(format!("{}.jbn", self.name()?.replace('/', "-"))))
    }
}

/// Wraps an I/O error with the path it concerns.
///
/// # Arguments
/// * `path` - the file.
///
/// # Returns
/// A closure turning the error into a failed run.
fn io(path: &Path) -> impl Fn(std::io::Error) -> Failure + '_ {
    move |e| Failure::failed(format!("{}: {e}", path.display()))
}

/// Turns a directory name into a package name: lowercase, `[a-z0-9._-]`.
///
/// # Arguments
/// * `root` - the project directory.
///
/// # Returns
/// The name; `app` when nothing usable is left.
fn package_name(root: &Path) -> String {
    let base = fs::canonicalize(root).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase())).unwrap_or_default();
    let name: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
    let name = name.trim_matches(|c| matches!(c, '.' | '_' | '-')).to_owned();
    if name.is_empty() { String::from("app") } else { name }
}

/// The `init` handler: writes `package.json` and the template's files, and a default
/// `.gitignore` when the template has none.
///
/// Decision: it refuses a directory that already has a `package.json` rather than
/// merging into it, and never overwrites a file that exists.
///
/// # Arguments
/// * `invocation` - `dir`, `type`, `mode` and `template`.
/// * `templates` - the mounted products' templates.
///
/// # Errors
/// An existing project, an unknown or broken template, `--mode node` for a desktop
/// project, or an unwritable directory.
fn init(invocation: &mut Invocation, templates: &[Templates]) -> Result<i32, Failure> {
    let values = &invocation.values;
    let root = values.path("dir").unwrap_or_else(|| PathBuf::from("."));
    let mode = values.text("mode").unwrap_or("lunar").to_owned();
    let template = match values.text("template") {
        Some(folder) if Path::new(folder).is_dir() => Template::folder(Path::new(folder))?,
        Some(name) => Template::mounted(templates, name)?,
        None => Template::mounted(templates, &format!("cli/{}", values.text("type").unwrap_or("program")))?,
    };
    if root.join("package.json").exists() {
        return Err(Failure::failed(format!("{} already has a package.json", root.display())));
    }
    if template.kind == "desktop" && mode == "node" {
        return Err(Failure::failed("a desktop project runs on lunar; drop --mode node"));
    }
    fs::create_dir_all(&root).map_err(io(&root))?;
    let name = package_name(&root);
    let kind = template.kind.clone();
    let program = kind == "program";
    let entry = template.main.as_str();
    let mut description = json!({
        "name": name,
        "version": "0.1.0",
        "type": "module",
        "main": entry,
        "lunar": { "type": kind, "mode": mode },
    });
    if program {
        // `node` runs TypeScript by stripping types (Node 22.18 and later).
        let runner = if mode == "node" { "node" } else { "lunar" };
        description["scripts"] = json!({ "start": format!("{runner} {entry}") });
    }
    if mode == "node" {
        description["engines"] = json!({ "node": ">=22.18" });
    }
    let mut files: Vec<(String, Vec<u8>)> = vec![(String::from("package.json"), (serde_json::to_string_pretty(&description).expect("JSON") + "\n").into_bytes())];
    for (path, bytes) in &template.files {
        let path = if path == "gitignore" { String::from(".gitignore") } else { path.clone() };
        let bytes = match std::str::from_utf8(bytes) {
            Ok(text) => text.replace("{{name}}", &name).into_bytes(),
            Err(_) => bytes.clone(),
        };
        files.push((path, bytes));
    }
    if !files.iter().any(|(path, _)| path == ".gitignore") {
        files.push((String::from(".gitignore"), b"build/\n.lunar/\nnode_modules/\n".to_vec()));
    }
    for (path, bytes) in &files {
        let path = root.join(path);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(io(parent))?;
            }
            fs::write(&path, bytes).map_err(io(&path))?;
        }
    }
    eprintln!("{}: {kind} '{name}' for {mode}, entry {entry}", root.display());
    Ok(0)
}

/// Counts files and lines per extension, skipping `node_modules`, `build` and
/// hidden entries.
///
/// # Arguments
/// * `directory` - where to start.
/// * `counts` - extension to (files, lines), filled in.
fn count(directory: &Path, counts: &mut BTreeMap<String, (usize, usize)>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" || name == "build" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            count(&path, counts);
        } else if let Ok(bytes) = fs::read(&path) {
            let extension = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_else(|| String::from("(none)"));
            let slot = counts.entry(extension).or_default();
            slot.0 += 1;
            slot.1 += bytes.iter().filter(|&&b| b == b'\n').count();
        }
    }
}

/// The `insight` handler: prints the project's description, dependencies, file
/// and line counts per extension, and its build.
///
/// # Errors
/// No readable `package.json`.
fn insight(invocation: &mut Invocation) -> Result<i32, Failure> {
    let root = invocation.values.path("dir").unwrap_or_else(|| PathBuf::from("."));
    let project = Project::open(&root)?;
    let lunar = |key: &str| project.json.get("lunar").and_then(|l| l.get(key)).and_then(Value::as_str).unwrap_or("-").to_owned();
    let names = |key: &str| -> Vec<String> { project.json.get(key).and_then(Value::as_object).map(|o| o.keys().cloned().collect()).unwrap_or_default() };
    let list = |names: &[String]| if names.is_empty() { String::from("0") } else { format!("{} ({})", names.len(), names.join(", ")) };
    println!("{:<14}{} {}", "name", project.text("name").unwrap_or("-"), project.text("version").unwrap_or("-"));
    println!("{:<14}{}", "type", lunar("type"));
    println!("{:<14}{}", "mode", lunar("mode"));
    println!("{:<14}{}", "entry", project.text("main").unwrap_or("-"));
    println!("{:<14}{}", "dependencies", list(&names("dependencies")));
    println!("{:<14}{}", "dev deps", list(&names("devDependencies")));
    let mut counts = BTreeMap::new();
    count(&root, &mut counts);
    let (files, lines) = counts.values().fold((0, 0), |(f, l), (cf, cl)| (f + cf, l + cl));
    println!("{:<14}{files} files, {lines} lines", "files");
    for (extension, (files, lines)) in &counts {
        println!("  {extension:<12}{files:>5} files {lines:>7} lines");
    }
    let build = match project.output().ok().and_then(|p| fs::metadata(&p).ok().map(|m| (p, m.len()))) {
        Some((path, bytes)) => format!("{} ({bytes} bytes)", path.strip_prefix(&root).unwrap_or(&path).display()),
        None => String::from("not compiled"),
    };
    println!("{:<14}{build}", "build");
    Ok(0)
}

/// Compiles the project and writes `build/<name>.jbn` and `build/trust.json`.
///
/// # Arguments
/// * `project` - the project.
/// * `release` - the release number.
/// * `signer` - the key that signs.
/// * `metadata` - extra manifest metadata.
///
/// # Returns
/// The package path and its size.
///
/// # Errors
/// A module that does not resolve or compile, or an unwritable `build/`.
fn pack(project: &Project, release: u64, signer: &Signer, metadata: &[(&str, String)]) -> Result<(PathBuf, usize), Failure> {
    let name = project.name()?;
    let version = project.text("version").unwrap_or("0.0.0");
    let mut builder = lunarstorm_host_cli::compile(&project.entry()?, name, version, release).map_err(Failure::failed)?;
    for (key, value) in metadata {
        builder.metadata(key, value);
    }
    let bytes = builder.build(signer).map_err(|e| Failure::failed(e.to_string()))?;
    let output = project.output()?;
    let build = output.parent().expect("build/");
    fs::create_dir_all(build).map_err(io(build))?;
    fs::write(&output, &bytes).map_err(io(&output))?;
    let trust = TrustStore { signers: vec![Trusted { public_key: signer.public_key(), packages: vec![name.to_owned()], minimum_release: 0 }] };
    let trust_file = build.join("trust.json");
    fs::write(&trust_file, trust.to_json()).map_err(io(&trust_file))?;
    Ok((output, bytes.len()))
}

/// # Arguments
/// * `invocation` - with `release`.
///
/// # Returns
/// `--release` as a release number.
///
/// # Errors
/// A negative number.
fn release(invocation: &Invocation) -> Result<u64, Failure> {
    invocation.values.int("release").and_then(|n| u64::try_from(n).ok()).ok_or_else(|| Failure::usage("--release must be a non-negative number"))
}

/// Reads the project's development key, creating it owner-only on first use.
///
/// # Arguments
/// * `root` - the project directory.
///
/// # Returns
/// The key.
///
/// # Errors
/// An unreadable or unwritable `.lunar/dev.der`.
fn development_key(root: &Path) -> Result<Signer, Failure> {
    let path = root.join(".lunar").join("dev.der");
    if let Ok(der) = fs::read(&path) {
        return Signer::from_pkcs8_der(&der).map_err(|e| Failure::failed(format!("{}: {e}", path.display())));
    }
    let signer = Signer::generate().map_err(|e| Failure::failed(e.to_string()))?;
    let der = signer.to_pkcs8_der().map_err(|e| Failure::failed(e.to_string()))?;
    fs::create_dir_all(path.parent().expect(".lunar")).map_err(io(&path))?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(&path).map_err(io(&path))?, &der).map_err(io(&path))?;
    Ok(signer)
}

/// The `compile` handler: `build/<name>.jbn` signed with the development key.
///
/// # Errors
/// See [`pack`] and [`development_key`].
fn compile(invocation: &mut Invocation) -> Result<i32, Failure> {
    let root = invocation.values.path("dir").unwrap_or_else(|| PathBuf::from("."));
    let project = Project::open(&root)?;
    project.packable()?;
    let signer = development_key(&root)?;
    let (output, bytes) = pack(&project, release(invocation)?, &signer, &[])?;
    eprintln!("{}: {bytes} bytes, development key; run with `lunar run --trust {} {}`", output.display(), output.with_file_name("trust.json").display(), output.display());
    Ok(0)
}

/// The `sign` handler: `build/<name>.jbn` signed with an issued developer key,
/// its certificate in the manifest metadata.
///
/// # Errors
/// A key not issued by Lunarstorm, an expired key, or see [`pack`].
fn sign(invocation: &mut Invocation) -> Result<i32, Failure> {
    let root = invocation.values.path("dir").unwrap_or_else(|| PathBuf::from("."));
    let key_file = invocation.values.path("key").ok_or_else(|| Failure::usage("sign needs --key <developer.json>"))?;
    let bytes = fs::read(&key_file).map_err(io(&key_file))?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let key = developer::DeveloperKey::open(&bytes, &developer::ROOT, now).map_err(|e| Failure::failed(format!("{}: {e}", key_file.display())))?;
    let project = Project::open(&root)?;
    project.packable()?;
    let (output, size) = pack(&project, release(invocation)?, &key.signer, &key.metadata())?;
    eprintln!("{}: {size} bytes, signed by {}", output.display(), key.developer);
    Ok(0)
}
