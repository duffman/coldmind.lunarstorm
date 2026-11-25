//! `lunar project` through the real command-line host, and the developer-key
//! certificate chain with a throwaway root.

use lunarstorm_cli_host::Cli;
use lunarstorm_jbn::{Limits, Package, Signer, TrustStore};
use lunarstorm_project::developer::{self, DeveloperKey};
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Runs `lunar <words>` with only the project package mounted.
///
/// # Arguments
/// * `words` - the command line after the program name.
///
/// # Returns
/// The exit status.
fn lunar(words: &[&str]) -> i32 {
    Cli::new("lunar", "lunar", "0", "test").package(lunarstorm_project::command::package()).run(words.iter().map(|w| (*w).to_owned()).collect())
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
fn init_writes_a_program_or_a_module_for_lunar_or_node() {
    let dir = tempfile::tempdir().expect("temp dir");
    let app = dir.path().join("Hello App");
    let lib = dir.path().join("shapes");
    assert_eq!(lunar(&["project", "init", app.to_str().unwrap()]), 0);
    assert_eq!(lunar(&["project", "init", lib.to_str().unwrap(), "--type", "module", "--mode", "node"]), 0);

    let program = json(&app.join("package.json"));
    assert_eq!(program["name"], "hello-app");
    assert_eq!(program["main"], "src/main.ts");
    assert_eq!(program["lunar"]["type"], "program");
    assert_eq!(program["lunar"]["mode"], "lunar");
    assert_eq!(program["scripts"]["start"], "lunar src/main.ts");
    assert!(app.join("src/main.ts").is_file());
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
fn init_refuses_an_existing_project_and_bad_choices() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().to_str().unwrap();
    assert_eq!(lunar(&["project", "init", path]), 0);
    let before = fs::read(dir.path().join("package.json")).unwrap();
    assert_eq!(lunar(&["project", "init", path, "--type", "module"]), 1);
    assert_eq!(fs::read(dir.path().join("package.json")).unwrap(), before);
    assert_eq!(lunar(&["project", "init", path, "--mode", "deno"]), 2);
}

#[test]
fn insight_needs_a_project() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().to_str().unwrap();
    assert_eq!(lunar(&["project", "insight", path]), 1);
    assert_eq!(lunar(&["project", "init", path]), 0);
    assert_eq!(lunar(&["project", "insight", path]), 0);
}

#[test]
fn compile_writes_a_package_its_trust_file_opens() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("calc");
    assert_eq!(lunar(&["project", "init", path.to_str().unwrap()]), 0);
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
    assert_eq!(lunar(&["project", "init", path.to_str().unwrap()]), 0);
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
    let mut builder = lunarstorm_host_cli::compile(&dir.path().join("main.ts"), "app", "1.0.0", 1).expect("compile");
    for (k, v) in key.metadata() {
        builder.metadata(k, &v);
    }
    let bytes = builder.build(&key.signer).unwrap();
    let trust = TrustStore { signers: vec![lunarstorm_jbn::Trusted { public_key: key.signer.public_key(), packages: Vec::new(), minimum_release: 0 }] };
    let package = Package::open(&bytes, &trust, &Limits::default()).unwrap();
    assert_eq!(developer::signed_by(&package, &root).unwrap(), "acme");
    assert!(developer::signed_by(&package, &test_root().1).is_err());

    // The certified metadata copied onto a package signed with another key.
    let other = Signer::generate().unwrap();
    let mut builder = lunarstorm_host_cli::compile(&dir.path().join("main.ts"), "app", "1.0.0", 1).expect("compile");
    for (k, v) in key.metadata() {
        builder.metadata(k, &v);
    }
    let trust = TrustStore { signers: vec![lunarstorm_jbn::Trusted { public_key: other.public_key(), packages: Vec::new(), minimum_release: 0 }] };
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
