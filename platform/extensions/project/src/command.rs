//! The `project` package of the `lunar` command line: `project init`, `insight`,
//! `compile` and `sign`.
//!
//! Every crate that adds commands to `lunar` exposes them from `src/command.rs` as
//! `command::package()`; `apps/lunarstorm/src/main.rs` mounts this one behind the
//! `project` feature. The host (`lunarstorm-cli-host`) parses the command line,
//! renders help and dispatches; this module holds the specs and the handlers.
//!
//! ```text
//! lunar project init [dir] --type module|program --mode lunar|node
//! lunar project insight [dir]
//! lunar project compile [dir]            build/<name>.jbn, development key
//! lunar project sign [dir] --key <developer.json>
//! ```
//!
//! Decisions:
//! - The project description is `package.json`: `name`, `version`, `main` (the
//!   entry) and a `lunar` object with `type` and `mode`. No other file is needed,
//!   and no template: `init` writes the few files itself.
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

/// The project package.
struct ProjectPackage;

/// Returns the project package for mounting on the command line.
///
/// # Returns
/// The package; it exports `project`.
pub fn package() -> Box<dyn Package> {
    Box::new(ProjectPackage)
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
        let release = || Arg::option("release").value(lunarstorm_cli_contract::ValueType::Int).default("1").about("release number trust stores compare");
        vec![Command::new("project", "create, inspect, compile and sign a Lunarstorm project")
            .child(
                Command::new("init", "create a project: package.json, an entry file and .gitignore")
                    .arg(directory())
                    .arg(Arg::option("type").choices(&["module", "program"]).default("program").about("a program to run, or a module to import"))
                    .arg(Arg::option("mode").choices(&["lunar", "node"]).default("lunar").about("the runtime it targets"))
                    .run(init)
                    .example("lunar project init hello --type program --mode lunar"),
            )
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

/// The `init` handler: writes `package.json`, the entry file and `.gitignore`.
///
/// Decision: it refuses a directory that already has a `package.json` rather than
/// merging into it.
///
/// # Errors
/// An existing project, or an unwritable directory.
fn init(invocation: &mut Invocation) -> Result<i32, Failure> {
    let values = &invocation.values;
    let root = values.path("dir").unwrap_or_else(|| PathBuf::from("."));
    let kind = values.text("type").unwrap_or("program").to_owned();
    let mode = values.text("mode").unwrap_or("lunar").to_owned();
    if root.join("package.json").exists() {
        return Err(Failure::failed(format!("{} already has a package.json", root.display())));
    }
    fs::create_dir_all(root.join("src")).map_err(io(&root))?;
    let name = package_name(&root);
    let program = kind == "program";
    let entry = if program { "src/main.ts" } else { "src/index.ts" };
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
    let source = if program { format!("console.log('hello from {name}');\n") } else { format!("export const name = '{name}';\n") };
    let files = [
        ("package.json", serde_json::to_string_pretty(&description).expect("JSON") + "\n"),
        (entry, source),
        (".gitignore", String::from("build/\n.lunar/\nnode_modules/\n")),
    ];
    for (path, text) in &files {
        let path = root.join(path);
        if !path.exists() {
            fs::write(&path, text).map_err(io(&path))?;
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
    let (output, size) = pack(&project, release(invocation)?, &key.signer, &key.metadata())?;
    eprintln!("{}: {size} bytes, signed by {}", output.display(), key.developer);
    Ok(0)
}
