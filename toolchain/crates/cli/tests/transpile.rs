mod support;

use std::fs;
use support::{Cli, Completed, Fixture, assert_success, project_arg};

fn assert_failure(output: &Completed) {
    assert!(
        !output.status.success(),
        "unexpected success: {}",
        output.stdout
    );
    assert!(
        output.stdout.is_empty(),
        "failure polluted stdout: {}",
        output.stdout
    );
    assert!(output.stderr.starts_with("error:"), "{}", output.stderr);
}

/// Inspects the inline map without requiring Node/Bun in native-only tests.
fn inline_map(code: &str) -> String {
    let encoded = code
        .split("//# sourceMappingURL=data:application/json;base64,")
        .nth(1)
        .expect("inline source map")
        .trim();
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0;
    for byte in encoded.bytes().take_while(|byte| *byte != b'=') {
        let value = alphabet
            .iter()
            .position(|candidate| *candidate == byte)
            .expect("source map must contain valid base64") as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    String::from_utf8(bytes).expect("UTF-8 source map JSON")
}

#[test]
fn file_transpilation_emits_only_javascript_without_loading_project_configuration() {
    let fixture = Fixture::new();
    fixture.write(".devtools/config.toml", "not valid TOML = [");
    fixture.write(
        "vite.config.js",
        "throw Error('configuration must not execute');",
    );
    fixture.write("src/entry.ts", "import {value} from './missing.ts';\ninterface Options { n: number }\nexport const answer: number = 42; console.log(value, answer);");
    let input = fixture.0.join("src/entry.ts");
    let output = Cli::spawn_in(&["--transpile", project_arg(&input)], true, &fixture.0).finish();
    assert_success(&output);
    assert!(output.stderr.is_empty(), "{}", output.stderr);
    assert!(output.stdout.contains("answer = 42"), "{}", output.stdout);
    assert!(
        output.stdout.contains("./missing.ts"),
        "imports must remain unchanged: {}",
        output.stdout
    );
    for absent in [
        "interface Options",
        ": number",
        "sourceMappingURL",
        "$RefreshReg$",
        "import.meta.hot",
        "devtools built",
        "ready at",
    ] {
        assert!(
            !output.stdout.contains(absent),
            "{absent}: {}",
            output.stdout
        );
    }
    assert!(!fixture.0.join("dist").exists());
    assert!(!input.with_extension("js").exists());
}

#[test]
fn tsx_lowers_jsx_without_development_refresh_code() {
    let fixture = Fixture::new();
    fixture.write("App.tsx", "interface Props { label: string }\nexport default function App({label}: Props) { return <main>{label}</main>; }");
    let input = fixture.0.join("App.tsx");
    let output = Cli::spawn(&["--transpile", project_arg(&input)], true).finish();
    assert_success(&output);
    assert!(
        output.stdout.contains("react/jsx-runtime"),
        "{}",
        output.stdout
    );
    for absent in [
        "interface Props",
        ": Props",
        "<main>",
        "$RefreshReg$",
        "import.meta.hot",
        "sourceMappingURL",
    ] {
        assert!(
            !output.stdout.contains(absent),
            "{absent}: {}",
            output.stdout
        );
    }
}

#[test]
fn all_supported_file_extensions_are_compiled_natively() {
    let fixture = Fixture::new();
    for extension in ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"] {
        let name = format!("entry.{extension}");
        let source = if ["ts", "tsx", "mts", "cts"].contains(&extension) {
            "const answer: number = 42; console.log(answer);"
        } else {
            "const answer = 42; console.log(answer);"
        };
        fixture.write(&name, source);
        let input = fixture.0.join(name);
        let output = Cli::spawn(&["--transpile", project_arg(&input)], true).finish();
        assert_success(&output);
        assert!(
            output.stdout.contains("answer = 42"),
            "{extension}: {}",
            output.stdout
        );
        assert!(
            !output.stdout.contains(": number"),
            "{extension}: {}",
            output.stdout
        );
    }
}

#[test]
fn transpilation_does_not_claim_to_typecheck_assignments() {
    let fixture = Fixture::new();
    fixture.write(
        "typed.ts",
        "export const mismatched: number = 'not-a-number';",
    );
    let input = fixture.0.join("typed.ts");
    let output = Cli::spawn(&["--transpile", project_arg(&input)], true).finish();
    assert_success(&output);
    assert!(output.stdout.contains("not-a-number"));
    assert!(!output.stdout.contains(": number"));
}

#[test]
fn output_file_supports_spaces_and_creates_parent_directories_without_a_default_map() {
    let fixture = Fixture::new();
    fixture.write(
        "source folder/entry file.ts",
        "export const answer: number = 42;",
    );
    let input = fixture.0.join("source folder/entry file.ts");
    let target = fixture.0.join("generated folder/nested/output.mjs");
    let output = Cli::spawn(
        &[
            "--transpile",
            project_arg(&input),
            "-o",
            project_arg(&target),
        ],
        true,
    )
    .finish();
    assert_success(&output);
    assert!(
        output.stdout.is_empty(),
        "file output must not pollute stdout: {}",
        output.stdout
    );
    assert!(output.stderr.is_empty(), "{}", output.stderr);
    let code = fs::read_to_string(&target).unwrap();
    assert!(code.contains("answer = 42"));
    assert!(!code.contains("sourceMappingURL"));
    assert!(!target.with_extension("mjs.map").exists());
    assert!(fs::read_to_string(input).unwrap().contains(": number"));
}

#[test]
fn external_map_follows_the_exact_output_filename_and_contains_original_source() {
    let fixture = Fixture::new();
    let source = "export const answer: number = 42;";
    fixture.write("entry.ts", source);
    let input = fixture.0.join("entry.ts");
    let target = fixture.0.join("generated/output.mjs");
    let output = Cli::spawn(
        &[
            "--transpile",
            project_arg(&input),
            "--out",
            project_arg(&target),
            "--sourcemap",
        ],
        true,
    )
    .finish();
    assert_success(&output);
    assert!(output.stdout.is_empty());
    let code = fs::read_to_string(&target).unwrap();
    assert!(
        code.contains("//# sourceMappingURL=output.mjs.map"),
        "{code}"
    );
    let map = fs::read_to_string(target.with_extension("mjs.map")).unwrap();
    assert!(map.contains("\"version\":3"), "{map}");
    assert!(map.contains("entry.ts") && map.contains(source), "{map}");
    assert!(!target.with_extension("js.map").exists());
}

#[test]
fn stdout_sourcemap_is_inline_and_keeps_the_original_typescript() {
    let fixture = Fixture::new();
    let source = "export const answer: number = 42;";
    fixture.write("entry.ts", source);
    let input = fixture.0.join("entry.ts");
    let output = Cli::spawn(&["--transpile", project_arg(&input), "--sourcemap"], true).finish();
    assert_success(&output);
    assert_eq!(output.stdout.matches("sourceMappingURL=").count(), 1);
    let map = inline_map(&output.stdout);
    assert!(map.starts_with('{') && map.ends_with('}'), "{map}");
    assert!(
        map.contains("\"version\":3") && map.contains(source),
        "{map}"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn stdin_defaults_to_typescript_and_finishes_when_the_pipe_closes() {
    let output = Cli::spawn_with_input(
        &["--transpile", "-"],
        true,
        b"export const stdinValue: number = 7;",
    )
    .finish();
    assert_success(&output);
    assert!(
        output.stdout.contains("stdinValue = 7"),
        "{}",
        output.stdout
    );
    assert!(!output.stdout.contains(": number"));
    assert!(!output.stdout.contains("sourceMappingURL"));
    assert!(output.stderr.is_empty());
}

#[test]
fn stdin_loaders_accept_js_jsx_ts_and_tsx() {
    for (loader, source, expected) in [
        ("js", "export const value = 7;", "value = 7"),
        (
            "jsx",
            "export default <main>hello</main>;",
            "react/jsx-runtime",
        ),
        ("ts", "export const value: number = 7;", "value = 7"),
        (
            "tsx",
            "export default <main>hello</main>;",
            "react/jsx-runtime",
        ),
    ] {
        let loader_arg = format!("--loader={loader}");
        let output =
            Cli::spawn_with_input(&["--transpile", "-", &loader_arg], true, source.as_bytes())
                .finish();
        assert_success(&output);
        assert!(
            output.stdout.contains(expected),
            "{loader}: {}",
            output.stdout
        );
        assert!(output.stderr.is_empty(), "{}", output.stderr);
    }
}

#[test]
fn stdin_can_write_a_file_and_external_sourcemap() {
    let fixture = Fixture::new();
    let target = fixture.0.join("stdin-output.js");
    let output = Cli::spawn_with_input(
        &[
            "--transpile",
            "-",
            "--loader",
            "ts",
            "--out",
            project_arg(&target),
            "--sourcemap",
        ],
        true,
        b"export const stdinValue: number = 7;",
    )
    .finish();
    assert_success(&output);
    assert!(output.stdout.is_empty());
    assert!(
        fs::read_to_string(&target)
            .unwrap()
            .contains("stdinValue = 7")
    );
    let map = fs::read_to_string(target.with_extension("js.map")).unwrap();
    assert!(
        map.contains("stdin") && map.contains("stdinValue: number"),
        "{map}"
    );
}

#[test]
fn explicit_stdout_output_uses_an_inline_map() {
    let output = Cli::spawn_with_input(
        &["--transpile", "-", "--out=-", "--sourcemap"],
        true,
        b"const value: number = 7;",
    )
    .finish();
    assert_success(&output);
    assert!(output.stdout.contains("value = 7"));
    assert!(inline_map(&output.stdout).contains("value: number"));
}

#[test]
fn equals_form_output_option_writes_the_requested_file() {
    let fixture = Fixture::new();
    fixture.write("entry.ts", "export const answer: number = 42;");
    let input = fixture.0.join("entry.ts");
    let target = fixture.0.join("equals.js");
    let out_arg = format!("--out={}", project_arg(&target));
    let output = Cli::spawn(&["--transpile", project_arg(&input), &out_arg], true).finish();
    assert_success(&output);
    assert!(output.stdout.is_empty());
    assert!(fs::read_to_string(target).unwrap().contains("answer = 42"));
}

#[test]
fn delimiter_allows_a_dash_prefixed_source_filename() {
    let fixture = Fixture::new();
    fixture.write("-entry.ts", "export const answer: number = 42;");
    let output = Cli::spawn_in(&["--transpile", "--", "-entry.ts"], true, &fixture.0).finish();
    assert_success(&output);
    assert!(output.stdout.contains("answer = 42"));
}

#[test]
fn malformed_transpile_arguments_fail_with_help_and_no_stdout() {
    for args in [
        &["--transpile"][..],
        &["--transpile", "entry.ts", "second.ts"],
        &["--transpile", "--out"],
        &["--transpile", "--out", "--sourcemap"],
        &["--transpile", "-", "--loader"],
        &["--transpile", "-", "--loader", "json"],
        &["--transpile", "entry.ts", "--loader", "ts"],
        &["--transpile", "-", "--port", "5000"],
        &["--transpile", "-", "--unknown"],
    ] {
        let output = Cli::spawn(args, true).finish();
        assert_failure(&output);
        assert!(
            output.stderr.contains("help"),
            "{args:?}: {}",
            output.stderr
        );
    }
}

#[test]
fn missing_files_and_unsupported_extensions_fail_without_creating_output() {
    let fixture = Fixture::new();
    fixture.write("entry.txt", "const value: number = 7;");
    let target = fixture.0.join("output.js");
    for input in [
        fixture.0.join("missing.ts"),
        fixture.0.join("entry.txt"),
        fixture.0.clone(),
    ] {
        let output = Cli::spawn(
            &[
                "--transpile",
                project_arg(&input),
                "-o",
                project_arg(&target),
            ],
            true,
        )
        .finish();
        assert_failure(&output);
        assert!(!target.exists());
    }
}

#[test]
fn parse_errors_name_the_source_and_preserve_existing_output_and_map() {
    let fixture = Fixture::new();
    fixture.write("broken.ts", "export const = ;");
    fixture.write("output.js", "keep output");
    fixture.write("output.js.map", "keep map");
    let input = fixture.0.join("broken.ts");
    let target = fixture.0.join("output.js");
    let output = Cli::spawn(
        &[
            "--transpile",
            project_arg(&input),
            "-o",
            project_arg(&target),
            "--sourcemap",
        ],
        true,
    )
    .finish();
    assert_failure(&output);
    assert!(output.stderr.contains("broken.ts"), "{}", output.stderr);
    assert_eq!(fs::read_to_string(target).unwrap(), "keep output");
    assert_eq!(
        fs::read_to_string(fixture.0.join("output.js.map")).unwrap(),
        "keep map"
    );
    assert_eq!(fs::read_to_string(input).unwrap(), "export const = ;");
}

#[test]
fn invalid_stdin_source_fails_without_emitting_partial_javascript() {
    let output = Cli::spawn_with_input(&["--transpile", "-"], true, b"export const = ;").finish();
    assert_failure(&output);
    assert!(output.stderr.contains("stdin"), "{}", output.stderr);
}

#[test]
fn output_cannot_overwrite_the_input_file() {
    let fixture = Fixture::new();
    let source = "export const value: number = 7;";
    fixture.write("entry.ts", source);
    let input = fixture.0.join("entry.ts");
    let output = Cli::spawn(
        &[
            "--transpile",
            project_arg(&input),
            "-o",
            project_arg(&input),
        ],
        true,
    )
    .finish();
    assert_failure(&output);
    assert_eq!(fs::read_to_string(input).unwrap(), source);
}

#[cfg(unix)]
#[test]
fn output_symlinks_and_hardlinks_cannot_alias_the_input() {
    let fixture = Fixture::new();
    let source = "export const value: number = 7;";
    fixture.write("entry.ts", source);
    let input = fixture.0.join("entry.ts");
    let symlink = fixture.0.join("symlink.js");
    let hardlink = fixture.0.join("hardlink.js");
    std::os::unix::fs::symlink(&input, &symlink).unwrap();
    fs::hard_link(&input, &hardlink).unwrap();
    for target in [&symlink, &hardlink] {
        let output = Cli::spawn(
            &[
                "--transpile",
                project_arg(&input),
                "-o",
                project_arg(target),
            ],
            true,
        )
        .finish();
        assert_failure(&output);
        assert_eq!(fs::read_to_string(&input).unwrap(), source);
        assert_eq!(fs::read_to_string(target).unwrap(), source);
    }
}

#[cfg(unix)]
#[test]
fn a_sourcemap_aliasing_the_input_is_rejected_before_output_changes() {
    let fixture = Fixture::new();
    let source = "export const value: number = 7;";
    fixture.write("entry.ts", source);
    fixture.write("output.js", "keep output");
    let input = fixture.0.join("entry.ts");
    let target = fixture.0.join("output.js");
    std::os::unix::fs::symlink(&input, fixture.0.join("output.js.map")).unwrap();
    let output = Cli::spawn(
        &[
            "--transpile",
            project_arg(&input),
            "-o",
            project_arg(&target),
            "--sourcemap",
        ],
        true,
    )
    .finish();
    assert_failure(&output);
    assert_eq!(fs::read_to_string(input).unwrap(), source);
    assert_eq!(fs::read_to_string(target).unwrap(), "keep output");
}

#[test]
fn output_directories_fail_without_modifying_the_source() {
    let fixture = Fixture::new();
    let source = "export const value: number = 7;";
    fixture.write("entry.ts", source);
    let input = fixture.0.join("entry.ts");
    let output = Cli::spawn(
        &[
            "--transpile",
            project_arg(&input),
            "-o",
            project_arg(&fixture.0),
        ],
        true,
    )
    .finish();
    assert_failure(&output);
    assert_eq!(fs::read_to_string(input).unwrap(), source);
}

#[test]
fn version_flags_are_successful_clean_and_match_the_cli_package() {
    for flag in ["--version", "-V"] {
        let output = Cli::spawn(&[flag], true).finish();
        assert_success(&output);
        assert_eq!(
            output.stdout.trim(),
            concat!("jetbunny ", env!("CARGO_PKG_VERSION"))
        );
        assert!(output.stderr.is_empty(), "{}", output.stderr);
    }
}

#[test]
fn asset_url_expressions_remain_runtime_urls_without_resolving_missing_assets() {
    let fixture = Fixture::new();
    let source = "export const asset: URL = new URL('./missing-asset.png', import.meta.url);";
    fixture.write("asset.ts", source);
    let input = fixture.0.join("asset.ts");
    let output = Cli::spawn(&["--transpile", project_arg(&input), "--sourcemap"], true).finish();
    assert_success(&output);
    let code = output.stdout.split("//# sourceMappingURL=").next().unwrap();
    assert!(code.contains("new URL("), "{code}");
    assert!(
        code.contains("./missing-asset.png") && code.contains("import.meta.url"),
        "{code}"
    );
    assert!(
        !code.contains("?url"),
        "must not synthesize an asset import: {code}"
    );
    assert!(
        inline_map(&output.stdout).contains(source),
        "map must retain source"
    );
    assert!(!fixture.0.join("missing-asset.png").exists());
}

#[test]
fn glob_calls_are_preserved_instead_of_expanding_the_filesystem() {
    let fixture = Fixture::new();
    let source = "export const modules = import.meta.glob('./pages/*.ts');";
    fixture.write("glob.ts", source);
    fixture.write("pages/one.ts", "export const page: string = 'one';");
    let input = fixture.0.join("glob.ts");
    let output = Cli::spawn(&["--transpile", project_arg(&input), "--sourcemap"], true).finish();
    assert_success(&output);
    let code = output.stdout.split("//# sourceMappingURL=").next().unwrap();
    assert!(
        code.contains("import.meta.glob(") && code.contains("./pages/*.ts"),
        "{code}"
    );
    assert!(
        !code.contains("./pages/one.ts"),
        "must not expand glob matches: {code}"
    );
    assert!(
        inline_map(&output.stdout).contains(source),
        "map must retain source"
    );
}

#[test]
fn template_dynamic_imports_keep_their_runtime_expression_and_source_map() {
    let fixture = Fixture::new();
    let source =
        "const name: string = 'one'; export const load = () => import(`./pages/${name}.ts`);";
    fixture.write("dynamic.ts", source);
    fixture.write("pages/one.ts", "export const page: string = 'one';");
    let input = fixture.0.join("dynamic.ts");
    let output = Cli::spawn(&["--transpile", project_arg(&input), "--sourcemap"], true).finish();
    assert_success(&output);
    let code = output.stdout.split("//# sourceMappingURL=").next().unwrap();
    assert!(
        code.contains("import(") && code.contains("`./pages/${name}.ts`"),
        "{code}"
    );
    assert!(
        !code.contains("./pages/one.ts"),
        "must not enumerate dynamic imports: {code}"
    );
    assert!(
        inline_map(&output.stdout).contains(source),
        "map must retain source"
    );
}

#[test]
fn runtime_environment_expressions_are_not_replaced_with_frontend_production_defines() {
    let fixture = Fixture::new();
    let source = "export const env = import.meta.env; export const mode = process.env.NODE_ENV; export const development = import.meta.env.DEV;";
    fixture.write("environment.ts", source);
    let input = fixture.0.join("environment.ts");
    let output = Cli::spawn(&["--transpile", project_arg(&input), "--sourcemap"], true).finish();
    assert_success(&output);
    let code = output.stdout.split("//# sourceMappingURL=").next().unwrap();
    for expression in [
        "import.meta.env",
        "process.env.NODE_ENV",
        "import.meta.env.DEV",
    ] {
        assert!(
            code.contains(expression),
            "runtime expression {expression} was rewritten: {code}"
        );
    }
    assert!(
        inline_map(&output.stdout).contains(source),
        "map must retain source"
    );
}

#[test]
fn commonjs_module_exports_are_preserved_for_cjs_and_cts_sources() {
    let fixture = Fixture::new();
    for (extension, source) in [
        ("cjs", "const value = 7; module.exports = {value};"),
        ("cts", "const value: number = 7; module.exports = {value};"),
    ] {
        let name = format!("entry.{extension}");
        fixture.write(&name, source);
        let input = fixture.0.join(name);
        let output =
            Cli::spawn(&["--transpile", project_arg(&input), "--sourcemap"], true).finish();
        assert_success(&output);
        let code = output.stdout.split("//# sourceMappingURL=").next().unwrap();
        assert!(code.contains("module.exports"), "{extension}: {code}");
        assert!(
            !code.contains("export ") && !code.contains("import "),
            "must not convert CommonJS to ESM: {code}"
        );
        assert!(!code.contains(": number"), "{code}");
        assert!(
            inline_map(&output.stdout).contains(source),
            "map must retain source"
        );
    }
}

#[test]
fn emitted_module_runs_in_node_with_runtime_imports_urls_and_environment_intact() {
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    // Keep the runtime execution bounded and reaped even if an assertion unwinds.
    struct NodeProcess(Child);
    impl Drop for NodeProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let fixture = Fixture::new();
    fixture.write("chunks/value.mjs", "export const value = 42;");
    fixture.write("asset.txt", "hello-π\n");
    fixture.write(
        "runtime.ts",
        r#"import {readFileSync} from 'node:fs';
const name: string = 'value';
const chunk = await import(`./chunks/${name}.mjs`);
const asset = readFileSync(new URL('./asset.txt', import.meta.url), 'utf8').trim();
console.log(JSON.stringify({
  value: chunk.value,
  asset,
  runtime: process.env.NODE_ENV,
  envType: typeof import.meta.env,
}));
"#,
    );
    let input = fixture.0.join("runtime.ts");
    let target = fixture.0.join("runtime.mjs");
    let output = Cli::spawn(
        &[
            "--transpile",
            project_arg(&input),
            "--out",
            project_arg(&target),
            "--sourcemap",
        ],
        true,
    )
    .finish();
    assert_success(&output);
    assert!(output.stdout.is_empty());
    assert!(target.with_extension("mjs.map").is_file());

    // Files avoid pipe backpressure while the parent polls the process deadline.
    let stdout_path = fixture.0.join("node-stdout.txt");
    let stderr_path = fixture.0.join("node-stderr.txt");
    let mut node = NodeProcess(
        Command::new("node")
            .arg("--enable-source-maps")
            .arg(&target)
            .current_dir(&fixture.0)
            .env("NODE_ENV", "transpile-runtime-value")
            .env_remove("NODE_OPTIONS")
            .env_remove("NODE_PATH")
            .stdin(Stdio::null())
            .stdout(Stdio::from(fs::File::create(&stdout_path).unwrap()))
            .stderr(Stdio::from(fs::File::create(&stderr_path).unwrap()))
            .spawn()
            .expect("Node is required for emitted-module integration coverage"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = node.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "Node execution deadline exceeded"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let stdout = fs::read_to_string(stdout_path).unwrap();
    let stderr = fs::read_to_string(stderr_path).unwrap();
    assert!(status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    assert_eq!(
        stdout.trim(),
        r#"{"value":42,"asset":"hello-π","runtime":"transpile-runtime-value","envType":"undefined"}"#,
    );
}
