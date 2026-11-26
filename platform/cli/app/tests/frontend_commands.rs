mod support;

use support::{Cli, Fixture, assert_success, get, project_arg};

fn native_project() -> Fixture {
    let project = Fixture::new();
    project.write(
        ".devtools/config.toml",
        r#"root = "client"
[server]
port = 0
[build]
outDir = "native-output"
minify = false
[resolve.alias]
"@message" = "./client/src/message.ts"
[define]
__VALUE__ = '"native-value"'
"#,
    );
    project.write(
        "vite.config.js",
        "throw Error('native config must take precedence')",
    );
    project.write("client/index.html", "<html><head></head><body><h1>native-app</h1><script type=\"module\" src=\"/src/main.ts\"></script></body></html>");
    project.write("client/src/message.ts", "export default 'alias-resolved';");
    project.write("client/src/main.ts", "import message from '@message'; import './style.css'; import image from './pixel.png'; console.log(message, __VALUE__, image);");
    project.write(
        "client/src/style.css",
        "body { color: red; background: url('./pixel.png'); }",
    );
    project.write(
        "client/src/pixel.png",
        [137, 80, 78, 71, 13, 10, 26, 10, 255, 0, 128],
    );
    project.write(
        "client/public/robots.txt",
        "User-agent: *\nDisallow: /private\n",
    );
    project
}

#[test]
fn help_describes_each_command_and_exits_successfully() {
    for args in [&["--help"][..], &["-h"], &["help"]] {
        let output = Cli::spawn(args, true).finish();
        assert_success(&output);
        assert!(output.stderr.is_empty(), "{}", output.stderr);
        assert!(output.stdout.contains("USAGE"), "{}", output.stdout);
        for name in ["serve", "bundle", "preview", "transpile"] {
            assert!(output.stdout.contains(name), "{}", output.stdout);
        }
    }
    for (name, option) in [
        ("serve", "--host"),
        ("bundle", "--no-minify"),
        ("preview", "--port"),
        ("transpile", "--sourcemap"),
    ] {
        {
            let args = [name, "--help"];
            let output = Cli::spawn(&args, true).finish();
            assert_success(&output);
            assert!(output.stderr.is_empty(), "{}", output.stderr);
            assert!(output.stdout.contains("USAGE"), "{}", output.stdout);
            assert!(
                output.stdout.contains(&format!("lunar {name}")),
                "{}",
                output.stdout
            );
            assert!(output.stdout.contains(option), "{}", output.stdout);
        }
    }
}

#[test]
fn malformed_arguments_fail_without_starting_a_listener() {
    for args in [
        &["serve", "--port"][..],
        &["bundle", "--out"],
        &["preview", "--port", "invalid"],
        &["serve", "--define", "=value"],
        &["bundle", "--host", "localhost"],
    ] {
        let output = Cli::spawn(args, true).finish();
        assert!(!output.status.success(), "unexpected success: {args:?}");
        assert!(
            !output.stderr.trim().is_empty(),
            "missing diagnostic for {args:?}"
        );
        assert!(!output.stdout.contains("ready at"));
    }
}

#[test]
fn dev_and_shorthand_apply_native_config_without_javascript() {
    let project = native_project();
    {
        let args = vec!["serve", project_arg(&project.0), "--no-watch"];
        let mut process = Cli::spawn(&args, true);
        let address = process.ready();
        let page = get(address, "/");
        assert!(page.headers.starts_with("HTTP/1.1 200"));
        assert!(page.text().contains("native-app"));
        let source = get(address, "/src/main.ts").text();
        assert!(source.contains("native-value"), "{source}");
        assert!(source.contains("/src/message.ts"), "{source}");
        assert!(!source.contains("__VALUE__"), "{source}");
        let asset = get(address, "/src/pixel.png");
        assert!(
            asset
                .headers
                .to_lowercase()
                .contains("content-type: image/png")
        );
        assert_eq!(
            asset.body,
            std::fs::read(project.0.join("client/src/pixel.png")).unwrap()
        );
    }
}

#[test]
fn cli_build_then_preview_serves_generated_javascript_styles_assets_and_spa_routes() {
    let project = native_project();
    let output = Cli::spawn(&["bundle", project_arg(&project.0)], true).finish();
    assert_success(&output);
    let root = project.0.join("client/native-output");
    let page = std::fs::read(root.join("index.html")).unwrap();
    let mut preview = Cli::spawn(&["preview", project_arg(&project.0), "--port", "0"], true);
    let address = preview.ready();
    assert_eq!(get(address, "/").body, page);
    assert_eq!(get(address, "/nested/client-route").body, page);
    assert_eq!(
        get(address, "/robots.txt").body,
        std::fs::read(root.join("robots.txt")).unwrap()
    );
    let mut kinds = Vec::new();
    for entry in std::fs::read_dir(root.join("assets")).unwrap() {
        let path = entry.unwrap().path();
        let extension = path.extension().unwrap().to_str().unwrap();
        let url = format!("/assets/{}", path.file_name().unwrap().to_str().unwrap());
        let response = get(address, &url);
        assert!(
            response.headers.starts_with("HTTP/1.1 200"),
            "{url}: {}",
            response.headers
        );
        assert_eq!(response.body, std::fs::read(&path).unwrap(), "{url}");
        let headers = response.headers.to_lowercase();
        match extension {
            "js" => {
                assert!(headers.contains("text/javascript"), "{headers}");
                let code = response.text();
                assert!(
                    code.contains("native-value") && code.contains("alias-resolved"),
                    "{code}"
                );
                assert!(
                    !code.contains("__VALUE__") && !code.contains("@message"),
                    "{code}"
                );
                assert!(
                    code.lines().count() > 3,
                    "native config disabled minification"
                );
            }
            "css" => {
                assert!(headers.contains("text/css"), "{headers}");
                assert!(response.text().contains("color: red"));
                assert!(response.text().contains("/assets/"));
            }
            "png" => {
                assert!(headers.contains("image/png"), "{headers}");
                assert_eq!(
                    response.body,
                    std::fs::read(project.0.join("client/src/pixel.png")).unwrap()
                );
            }
            other => panic!("unexpected generated asset type {other}"),
        }
        kinds.push(extension.to_string());
    }
    for kind in ["js", "css", "png"] {
        assert!(
            kinds.iter().any(|value| value == kind),
            "missing generated {kind}"
        );
    }
    let missing = get(address, "/missing.js");
    assert!(
        missing.headers.starts_with("HTTP/1.1 404"),
        "{}",
        missing.headers
    );
}

#[test]
fn explicit_build_and_preview_output_overrides_native_directory() {
    let project = native_project();
    let config = project.0.join(".devtools/config.toml");
    std::fs::write(
        &config,
        std::fs::read_to_string(&config)
            .unwrap()
            .replace("minify = false", "minify = true"),
    )
    .unwrap();
    let output = Cli::spawn(
        &[
            "bundle",
            project_arg(&project.0),
            "--out",
            "explicit-output",
            "--no-minify",
        ],
        true,
    )
    .finish();
    assert_success(&output);
    assert!(!project.0.join("client/native-output").exists());
    let root = project.0.join("client/explicit-output");
    let mut preview = Cli::spawn(
        &[
            "preview",
            project_arg(&project.0),
            "--out",
            "explicit-output",
            "--port",
            "0",
        ],
        true,
    );
    let address = preview.ready();
    assert_eq!(
        get(address, "/").body,
        std::fs::read(root.join("index.html")).unwrap()
    );
    let js = std::fs::read_dir(root.join("assets"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "js"))
        .unwrap();
    assert!(
        std::fs::read_to_string(js).unwrap().lines().count() > 3,
        "--no-minify overrides TOML true"
    );
}

#[test]
fn invalid_native_config_fails_all_commands_without_falling_back_or_clearing_output() {
    let project = native_project();
    project.write(".devtools/config.toml", "[server]\nport = 'invalid'\n");
    project.write("client/native-output/sentinel", "keep");
    for command in ["serve", "bundle", "preview"] {
        let output = Cli::spawn(&[command, project_arg(&project.0)], true).finish();
        assert!(
            !output.status.success(),
            "{command} accepted invalid config"
        );
        assert!(output.stderr.contains("config.toml"), "{}", output.stderr);
        assert!(
            !output.stderr.contains("requires Bun"),
            "must not fall back to Vite: {}",
            output.stderr
        );
        assert_eq!(
            std::fs::read_to_string(project.0.join("client/native-output/sentinel")).unwrap(),
            "keep"
        );
    }
}

#[test]
fn preview_compatibility_config_receives_production_preview_environment() {
    let project = Fixture::new();
    project.write(
        "frontend/configured-preview/index.html",
        "configured-preview-ok",
    );
    project.write("vite.config.mjs", "export default ({command,mode,isPreview}) => ({root: command === 'serve' && mode === 'production' && isPreview ? 'frontend' : 'wrong', build:{outDir:'configured-preview'}})");
    let mut child = Cli::spawn(
        &["preview", project_arg(&project.0), "--port", "0"],
        false,
    );
    let response = get(child.ready(), "/");
    assert!(response.headers.starts_with("HTTP/1.1 200"));
    assert_eq!(response.text(), "configured-preview-ok");
}

#[test]
fn build_reports_source_and_dependency_failures_as_nonzero_exit() {
    let project = native_project();
    for (source, expected) in [
        ("export const = ;", "main.ts"),
        (
            "import value from 'missing-cli-integration-package'; console.log(value);",
            "missing-cli-integration-package",
        ),
    ] {
        project.write("client/src/main.ts", source);
        let output = Cli::spawn(&["bundle", project_arg(&project.0)], true).finish();
        assert!(
            !output.status.success(),
            "invalid source built successfully: {source}"
        );
        assert!(output.stderr.contains(expected), "{}", output.stderr);
        assert!(
            !output.stdout.contains("devtools built"),
            "{}",
            output.stdout
        );
    }
}

#[test]
fn unsafe_output_override_fails_before_deleting_project_sources() {
    let project = native_project();
    let original = std::fs::read(project.0.join("client/src/main.ts")).unwrap();
    for directory in [".", ".."] {
        let output = Cli::spawn(
            &["bundle", project_arg(&project.0), "--out", directory],
            true,
        )
        .finish();
        assert!(
            !output.status.success(),
            "unsafe output override accepted: {directory}"
        );
        assert!(
            output.stderr.contains("would replace the app root"),
            "{}",
            output.stderr
        );
        assert_eq!(
            std::fs::read(project.0.join("client/src/main.ts")).unwrap(),
            original
        );
        assert!(project.0.join(".devtools/config.toml").is_file());
    }
}
