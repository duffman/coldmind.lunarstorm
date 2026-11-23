use devtools_bundler::{build, BuildOptions};

#[tokio::test]
async fn builds_typescript_styles_and_public_assets() {
    let root = std::env::temp_dir().join(format!("devtools-bundle-test-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("public")).unwrap();
    std::fs::write(root.join("index.html"), "<html><head></head><body><script type=\"module\" src=\"/src/main.ts\"></script></body></html>").unwrap();
    std::fs::write(
        root.join("src/main.ts"),
        "import './style.css'; const mode: string = import.meta.env.MODE; console.log(mode);",
    )
    .unwrap();
    std::fs::write(root.join("src/style.css"), "body { color: red; }").unwrap();
    std::fs::write(root.join("public/robots.txt"), "User-agent: *\n").unwrap();
    let report = build(&BuildOptions::new(&root)).await.unwrap();
    assert_eq!(report.entries.len(), 1);
    let page = std::fs::read_to_string(report.out.join("index.html")).unwrap();
    assert!(page.contains(&report.entries[0].1), "{page}");
    assert!(page.contains(report.stylesheet.as_ref().unwrap()), "{page}");
    let js = std::fs::read_to_string(report.out.join(&report.entries[0].1)).unwrap();
    assert!(js.contains("production"), "{js}");
    assert!(!js.contains("import.meta.env"), "{js}");
    assert_eq!(
        std::fs::read_to_string(report.out.join("robots.txt")).unwrap(),
        "User-agent: *\n"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn discovers_vite_config_and_applies_build_options() {
    let dir = std::env::temp_dir().join(format!("devtools-vite-build-{}", std::process::id()));
    let root = dir.join("frontend");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("index.html"),
        "<script type=\"module\" src=\"/src/main.ts\"></script>",
    )
    .unwrap();
    std::fs::write(
        root.join("src/message.ts"),
        "export default 'alias-loaded';",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.ts"),
        "import message from '@message'; console.log(message, __CONFIG_VALUE__);",
    )
    .unwrap();
    std::fs::write(dir.join("vite.config.ts"), r#"
export default async ({ command, mode }) => ({
  root: './frontend',
  resolve: { alias: { '@message': new URL('./frontend/src/message.ts', import.meta.url).pathname } },
  define: { __CONFIG_VALUE__: JSON.stringify(`${command}:${mode}`) },
  build: { outDir: 'configured-output', minify: false }
});
"#).unwrap();
    let report = build(&BuildOptions::new(&dir)).await.unwrap();
    assert_eq!(
        report.out,
        root.canonicalize().unwrap().join("configured-output")
    );
    let js = std::fs::read_to_string(report.out.join(&report.entries[0].1)).unwrap();
    assert!(js.contains("alias-loaded"), "{js}");
    assert!(js.contains("build:production"), "{js}");
    assert!(!js.contains("__CONFIG_VALUE__"), "{js}");
    assert!(js.lines().count() > 1, "config disabled minification: {js}");
    let mut explicit = BuildOptions::new(&dir);
    explicit.out = Some("dist".into());
    explicit.minify = Some(true);
    let report = build(&explicit).await.unwrap();
    assert_eq!(report.out, root.canonicalize().unwrap().join("dist"));
    let js = std::fs::read_to_string(report.out.join(&report.entries[0].1)).unwrap();
    assert!(
        js.lines().count() <= 2,
        "explicit minify overrides config: {js}"
    );
    std::fs::write(dir.join("vite.config.ts"), "export default { broken: ;").unwrap();
    assert!(build(&BuildOptions::new(&dir)).await.is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn configured_output_cannot_replace_project_or_app_root() {
    let project = std::env::temp_dir().join(format!(
        "devtools-vite-output-safety-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(project.join("app/child")).unwrap();
    std::fs::write(
        project.join("app/index.html"),
        "<script type=\"module\" src=\"/main.js\"></script>",
    )
    .unwrap();
    std::fs::write(project.join("app/main.js"), "console.log(42)").unwrap();
    std::fs::write(project.join("sentinel"), "keep").unwrap();
    for out in [".", "..", "../app", "child/.."] {
        std::fs::write(
            project.join("vite.config.js"),
            format!("export default {{ root:'app', build:{{outDir:'{out}'}} }}"),
        )
        .unwrap();
        let error = build(&BuildOptions::new(&project))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("would replace the app root"),
            "unsafe output {out}: {error}"
        );
        assert!(project.join("app/index.html").is_file());
        assert_eq!(
            std::fs::read_to_string(project.join("sentinel")).unwrap(),
            "keep"
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(project.join("app"), project.join("app/output-link")).unwrap();
        std::fs::write(
            project.join("vite.config.js"),
            "export default {root:'app',build:{outDir:'output-link'}}",
        )
        .unwrap();
        assert!(build(&BuildOptions::new(&project)).await.is_err());
    }
    std::fs::remove_dir_all(project).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn linked_source_resolves_peer_dependencies_from_application() {
    let fixture =
        std::env::temp_dir().join(format!("devtools-linked-peer-{}", std::process::id()));
    let root = fixture.join("app");
    let external = fixture.join("external-source");
    std::fs::create_dir_all(root.join("node_modules/app-peer")).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(
        root.join("index.html"),
        "<script type=\"module\" src=\"/main.js\"></script>",
    )
    .unwrap();
    std::fs::write(
        root.join("main.js"),
        "import message from 'linked'; console.log(message)",
    )
    .unwrap();
    std::fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"paths":{"linked":["./linked/index.ts"]}}}"#,
    )
    .unwrap();
    std::fs::write(
        external.join("index.ts"),
        "export { default } from 'app-peer';",
    )
    .unwrap();
    std::os::unix::fs::symlink(&external, root.join("linked")).unwrap();
    std::fs::write(
        root.join("node_modules/app-peer/package.json"),
        r#"{"type":"module","exports":"./index.js"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("node_modules/app-peer/index.js"),
        "export default 'linked-peer-resolved';",
    )
    .unwrap();
    let report = build(&BuildOptions::new(&root)).await.unwrap();
    let js = std::fs::read_to_string(report.out.join(&report.entries[0].1)).unwrap();
    assert!(js.contains("linked-peer-resolved"), "{js}");
    assert!(!js.contains("from\"app-peer\""), "{js}");
    std::fs::remove_dir_all(fixture).unwrap();
}

#[tokio::test]
async fn missing_browser_dependency_fails_instead_of_emitting_bare_import() {
    let root = std::env::temp_dir().join(format!(
        "devtools-missing-browser-dep-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("index.html"),
        "<script type=\"module\" src=\"/main.js\"></script>",
    )
    .unwrap();
    std::fs::write(
        root.join("main.js"),
        "import {missing} from 'does-not-exist-devtools-test'; console.log(missing)",
    )
    .unwrap();
    let error = build(&BuildOptions::new(&root))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("does-not-exist-devtools-test"), "{error}");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn native_toml_config_controls_build_and_preserves_explicit_overrides() {
    let project = std::env::temp_dir().join(format!(
        "devtools-native-toml-build-{}",
        std::process::id()
    ));
    let root = project.join("client");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(project.join(".devtools")).unwrap();
    std::fs::write(
        root.join("index.html"),
        "<script type=\"module\" src=\"/src/main.ts\"></script>",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.ts"),
        "import message from '@message'; console.log(message,__NATIVE__)",
    )
    .unwrap();
    std::fs::write(
        root.join("src/message.ts"),
        "export default 'native-alias-applied'",
    )
    .unwrap();
    std::fs::write(
        project.join("vite.config.js"),
        "throw Error('native config must take precedence')",
    )
    .unwrap();
    std::fs::write(
        project.join(".devtools/config.toml"),
        r#"
root = "client"
[build]
outDir = "native-output"
minify = false
[resolve.alias]
"@message" = "./client/src/message.ts"
[define]
__NATIVE__ = '"native-define-applied"'
"#,
    )
    .unwrap();
    let report = build(&BuildOptions::new(&project)).await.unwrap();
    assert_eq!(
        report.out,
        root.canonicalize().unwrap().join("native-output")
    );
    let js = std::fs::read_to_string(report.out.join(&report.entries[0].1)).unwrap();
    assert!(js.contains("native-alias-applied"), "{js}");
    assert!(js.contains("native-define-applied"), "{js}");
    assert!(!js.contains("__NATIVE__"), "{js}");
    assert!(js.lines().count() > 1, "{js}");
    let mut explicit = BuildOptions::new(&project);
    explicit.out = Some("explicit-output".into());
    explicit.minify = Some(true);
    let report = build(&explicit).await.unwrap();
    assert_eq!(
        report.out,
        root.canonicalize().unwrap().join("explicit-output")
    );
    let js = std::fs::read_to_string(report.out.join(&report.entries[0].1)).unwrap();
    assert!(js.lines().count() <= 2, "{js}");
    std::fs::remove_dir_all(project).unwrap();
}

#[tokio::test]
async fn plain_stylesheet_assets_resolve_from_each_source_directory() {
    let root = std::env::temp_dir().join(format!(
        "devtools-css-relative-assets-{}",
        std::process::id()
    ));
    for directory in ["one", "two"] {
        std::fs::create_dir_all(root.join("src").join(directory)).unwrap();
    }
    std::fs::write(
        root.join("index.html"),
        "<script type=\"module\" src=\"/src/main.js\"></script>",
    )
    .unwrap();
    // No JavaScript image imports: stylesheet processing alone must emit both binary assets.
    std::fs::write(
        root.join("src/main.js"),
        "import './one/style.css'; import './two/style.css';",
    )
    .unwrap();
    std::fs::write(root.join("src/one/style.css"), ".one{background:url('./pixel.png')} .public{background:url('/public.png')} .remote{background:url('https://example.invalid/pixel.png')} .data{background:url(data:image/png;base64,AA)}").unwrap();
    std::fs::write(
        root.join("src/two/style.css"),
        ".two{background:url('./pixel.png')}",
    )
    .unwrap();
    let first = [0_u8, 255, 1];
    let second = [2_u8, 255, 3];
    std::fs::write(root.join("src/one/pixel.png"), first).unwrap();
    std::fs::write(root.join("src/two/pixel.png"), second).unwrap();
    let report = build(&BuildOptions::new(&root)).await.unwrap();
    let css = std::fs::read_to_string(report.out.join(report.stylesheet.unwrap())).unwrap();
    let images: Vec<_> = std::fs::read_dir(report.out.join("assets"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .collect();
    assert_eq!(images.len(), 2, "{css}");
    let mut contents = Vec::new();
    for image in images {
        assert!(
            css.contains(&format!(
                "/assets/{}",
                image.file_name().unwrap().to_string_lossy()
            )),
            "{css}"
        );
        contents.push(std::fs::read(image).unwrap());
    }
    assert!(contents.contains(&first.to_vec()));
    assert!(contents.contains(&second.to_vec()));
    assert!(!css.contains("./pixel.png"), "{css}");
    for preserved in [
        "/public.png",
        "https://example.invalid/pixel.png",
        "data:image/png;base64,AA",
    ] {
        assert!(css.contains(preserved), "{css}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
