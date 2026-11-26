//! `lunar project` through the real command-line host, and the developer-key
//! certificate chain with a throwaway root.

use lunarstorm_cli_host::Cli;
use lunarstorm_jbn_package_format::{Limits, Package, Signer, TrustStore};
use lunarstorm_project::developer::{self, DeveloperKey};
use lunarstorm_project::{create, Type};
use serde_json::Value;
use std::fs;
use std::path::Path;

/// The files of the fixture types, embedded the way a product crate embeds its own:
/// `("<type>/<path>", bytes)`.
static FILES: &[(&str, &[u8])] = &[
    ("app/README.md", b"# {{name}}\n"),
    ("app/gitignore", b"build/\n.lunar/\nnode_modules/\ndist/\n"),
    ("app/src/app.tsx", b"export default {};\n"),
    ("module/src/index.ts", b"export const name = '{{name}}';\n"),
    ("program/src/main.ts", b"console.log('hello from {{name}}');\n"),
];

/// The fixture types, registered as the cli and desktop products register theirs.
///
/// # Returns
/// `program` and `module` (lunar or node) and `app` (lunar only), each with `create`.
fn registered() -> Vec<Type> {
    vec![
        Type::new("program", "a program lunar runs").action(create("program", "src/main.ts", &["lunar", "node"], FILES)),
        Type::new("module", "a module other code imports").action(create("module", "src/index.ts", &["lunar", "node"], FILES)),
        Type::new("app", "a desktop app").action(create("app", "src/app.tsx", &["lunar"], FILES)),
    ]
}

/// Runs `lunar <words>` with only the project package mounted, holding the fixture
/// types.
///
/// # Arguments
/// * `words` - the command line after the program name.
///
/// # Returns
/// The exit status.
fn lunar(words: &[&str]) -> i32 {
    Cli::new("lunar", "lunar", "0", "test").package(lunarstorm_project::package(vec![registered])).run(words.iter().map(|w| (*w).to_owned()).collect())
}

/// # Arguments
/// * `path` - a JSON file.
///
/// # Returns
/// Its value.
fn json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read")).expect("JSON")
}

/// A throwaway root: its PKCS#8 key and public key.
///
/// # Returns
/// `(der, public)`.
fn test_root() -> (Vec<u8>, [u8; 32]) {
    let root = Signer::generate().expect("root");
    (root.to_pkcs8_der().expect("der").to_vec(), root.public_key())
}

#[test]
fn create_writes_a_program_or_a_module_for_lunar_or_node() {
    let dir = tempfile::tempdir().expect("temp dir");
    let app = dir.path().join("Hello App");
    let lib = dir.path().join("shapes");
    assert_eq!(lunar(&["project", "program", "create", app.to_str().unwrap()]), 0);
    assert_eq!(lunar(&["project", "module", "create", lib.to_str().unwrap(), "--mode", "node"]), 0);

    let program = json(&app.join("package.json"));
    assert_eq!(program["name"], "hello-app");
    assert_eq!(program["main"], "src/main.ts");
    assert_eq!(program["lunar"]["type"], "program");
    assert_eq!(program["lunar"]["mode"], "lunar");
    assert_eq!(program["scripts"]["start"], "lunar src/main.ts");
    assert_eq!(fs::read_to_string(app.join("src/main.ts")).unwrap(), "console.log('hello from hello-app');\n");
    assert!(fs::read_to_string(app.join(".gitignore")).unwrap().contains(".lunar/"));

    let module = json(&lib.join("package.json"));
    assert_eq!(module["main"], "src/index.ts");
    assert_eq!(module["lunar"]["type"], "module");
    assert_eq!(module["lunar"]["mode"], "node");
    assert_eq!(module["engines"]["node"], ">=22.18");
    assert!(module.get("scripts").is_none());
    assert!(lib.join("src/index.ts").is_file());
}

#[test]
fn create_refuses_an_existing_project_and_bad_choices() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().to_str().unwrap();
    assert_eq!(lunar(&["project", "program", "create", path]), 0);
    let before = fs::read(dir.path().join("package.json")).unwrap();
    assert_eq!(lunar(&["project", "module", "create", path]), 1);
    assert_eq!(fs::read(dir.path().join("package.json")).unwrap(), before);
    assert_eq!(lunar(&["project", "program", "create", path, "--mode", "deno"]), 2);
}

/// `types` lists the registered types; an unregistered type or action is a usage
/// error.
#[test]
fn types_are_listed_and_unknown_types_or_actions_are_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("x");
    let path = path.to_str().unwrap();
    assert_eq!(lunar(&["project", "types"]), 0);
    assert_eq!(lunar(&["project", "desktop", "create", path]), 2);
    assert_eq!(lunar(&["project", "program", "publish", path]), 2);
    assert!(!dir.path().join("x").exists());
}

#[test]
fn insight_needs_a_project() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().to_str().unwrap();
    assert_eq!(lunar(&["project", "insight", path]), 1);
    assert_eq!(lunar(&["project", "program", "create", path]), 0);
    assert_eq!(lunar(&["project", "insight", path]), 0);
}

#[test]
fn compile_writes_a_package_its_trust_file_opens() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("calc");
    assert_eq!(lunar(&["project", "program", "create", path.to_str().unwrap()]), 0);
    fs::write(path.join("src/main.ts"), "import { twice } from './twice.ts';\nconsole.log(twice(21));\n").unwrap();
    fs::write(path.join("src/twice.ts"), "export const twice = (n: number): number => n * 2;\n").unwrap();
    assert_eq!(lunar(&["project", "compile", path.to_str().unwrap()]), 0);

    let trust = TrustStore::from_json(&fs::read(path.join("build/trust.json")).unwrap()).expect("trust");
    let package = Package::open(&fs::read(path.join("build/calc.jbn")).unwrap(), &trust, &Limits::default()).expect("opens");
    assert_eq!(package.manifest().name, "calc");
    assert_eq!(package.manifest().modules.len(), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(path.join(".lunar/dev.der")).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn sign_refuses_a_key_not_issued_by_lunarstorm() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("app");
    assert_eq!(lunar(&["project", "program", "create", path.to_str().unwrap()]), 0);
    let (der, _) = test_root();
    let key = dir.path().join("developer.json");
    fs::write(&key, developer::issue(&der, "acme", u64::MAX).unwrap()).unwrap();
    assert_eq!(lunar(&["project", "sign", path.to_str().unwrap(), "--key", key.to_str().unwrap()]), 1);
    assert!(!path.join("build").exists());
}

#[test]
fn an_issued_key_signs_and_the_package_names_its_developer() {
    let (der, root) = test_root();
    let file = developer::issue(&der, "acme", u64::MAX).unwrap();
    let key = DeveloperKey::open(file.as_bytes(), &root, 0).expect("issued by root");
    assert_eq!(key.developer, "acme");

    let dir = tempfile::tempdir().expect("temp dir");
    fs::write(dir.path().join("main.ts"), "console.log(1);\n").unwrap();
    let mut builder = lunarstorm_host_command_line::compile(&dir.path().join("main.ts"), "app", "1.0.0", 1).expect("compile");
    for (k, v) in key.metadata() {
        builder.metadata(k, &v);
    }
    let bytes = builder.build(&key.signer).unwrap();
    let trust = TrustStore { signers: vec![lunarstorm_jbn_package_format::Trusted { public_key: key.signer.public_key(), packages: Vec::new(), minimum_release: 0 }] };
    let package = Package::open(&bytes, &trust, &Limits::default()).unwrap();
    assert_eq!(developer::signed_by(&package, &root).unwrap(), "acme");
    assert!(developer::signed_by(&package, &test_root().1).is_err());

    // The certified metadata copied onto a package signed with another key.
    let other = Signer::generate().unwrap();
    let mut builder = lunarstorm_host_command_line::compile(&dir.path().join("main.ts"), "app", "1.0.0", 1).expect("compile");
    for (k, v) in key.metadata() {
        builder.metadata(k, &v);
    }
    let trust = TrustStore { signers: vec![lunarstorm_jbn_package_format::Trusted { public_key: other.public_key(), packages: Vec::new(), minimum_release: 0 }] };
    let forged = Package::open(&builder.build(&other).unwrap(), &trust, &Limits::default()).unwrap();
    assert!(developer::signed_by(&forged, &root).is_err());
}

#[test]
fn a_developer_key_is_refused_from_another_root_expired_or_tampered() {
    let (der, root) = test_root();
    let file = developer::issue(&der, "acme", 1_000).unwrap();
    assert!(DeveloperKey::open(file.as_bytes(), &root, 1_000).is_ok());
    assert!(DeveloperKey::open(file.as_bytes(), &root, 1_001).err().expect("refused").contains("expired"));
    assert!(DeveloperKey::open(file.as_bytes(), &test_root().1, 0).err().expect("refused").contains("not issued"));
    let renamed = file.replace("\"acme\"", "\"evil\"");
    assert!(DeveloperKey::open(renamed.as_bytes(), &root, 0).err().expect("refused").contains("not issued"));
    let extended = file.replace("1000", "9999999999");
    assert!(DeveloperKey::open(extended.as_bytes(), &root, 0).err().expect("refused").contains("not issued"));
    assert!(developer::issue(&der, "Bad Id", 1).is_err());
}

/// An `app` creates its own files, targets lunar only, and is not compiled by
/// `compile`.
#[test]
fn an_app_is_created_for_lunar_only_and_compile_refuses_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let app = dir.path().join("notes");
    let path = app.to_str().unwrap();
    assert_eq!(lunar(&["project", "app", "create", path, "--mode", "node"]), 2);
    assert!(!app.join("package.json").exists());
    assert_eq!(lunar(&["project", "app", "create", path]), 0);

    let description = json(&app.join("package.json"));
    assert_eq!(description["name"], "notes");
    assert_eq!(description["main"], "src/app.tsx");
    assert_eq!(description["lunar"]["type"], "app");
    assert!(description.get("scripts").is_none());
    assert!(app.join("src/app.tsx").is_file());
    assert!(fs::read_to_string(app.join(".gitignore")).unwrap().contains("dist/"));
    assert_eq!(fs::read_to_string(app.join("README.md")).unwrap(), "# notes\n");
    assert_eq!(lunar(&["project", "compile", path]), 1);
    assert!(!app.join("build").exists());
}
