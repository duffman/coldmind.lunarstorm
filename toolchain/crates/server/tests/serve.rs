//! Dev server end to end: pages, modules, dependency interop, and HMR frames over the socket.

use std::path::{Path, PathBuf};
use std::time::Duration;

use devtools_server::{AppMessage, Config, DevServer, HMR_PATH};
use futures_util::StreamExt;
use serde_json::Value;

/// Copies the fixture app into a temp dir so tests can edit files.
fn app() -> tempfile::TempDir {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixtures.join("app"), dir.path());
    copy_dir(&fixtures.join("packages"), &dir.path().join("node_modules"));
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn start(root: &Path, watch: bool) -> DevServer {
    let mut config = Config::new(root);
    config.port = 0;
    config.watch = watch;
    config
        .define
        .push(("__APP__".into(), "\"fixture-app\"".into()));
    DevServer::start(config).unwrap()
}

async fn get(server: &DevServer, path: &str) -> (u16, String) {
    let url = format!("http://{}{path}", server.addr());
    let response = reqwest_lite(&url).await;
    response
}

#[tokio::test]
async fn unknown_env_keys_read_from_a_defined_env_object() {
    let dir = app();
    std::fs::write(dir.path().join("env.ts"), "export const server = import.meta.env.VITE_SERVER_URL ?? ''; export const mode = import.meta.env.MODE;").unwrap();
    let server = start(dir.path(), false);
    let (status, code) = get(&server, "/env.ts").await;
    assert_eq!(status, 200, "{code}");
    let source = code.split("//# sourceMappingURL=").next().unwrap();
    assert!(!source.contains("import.meta.env"), "{source}");
    assert!(source.contains("development"), "{source}");
}

/// `public/` is served at `/`, the same URLs `build` gives it by copying it into the output root.
#[tokio::test]
async fn public_files_are_served_at_the_root_like_the_build() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html><body></body></html>").unwrap();
    std::fs::create_dir_all(dir.path().join("public")).unwrap();
    std::fs::write(dir.path().join("public/robots.txt"), "User-agent: *\n").unwrap();
    let server = start(dir.path(), false);
    assert_eq!(
        get(&server, "/robots.txt").await,
        (200, "User-agent: *\n".to_string())
    );
}

/// Minimal HTTP GET over tokio (keeps the test free of an HTTP client dependency).
async fn reqwest_lite(url: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest
        .split_once('/')
        .map(|(h, p)| (h, format!("/{p}")))
        .unwrap();
    let mut stream = tokio::net::TcpStream::connect(host).await.unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(body)
    } else {
        body.to_string()
    };
    (status, body)
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = body.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 {
            break;
        }
        out.push_str(&rest[..n]);
        body = &rest[n + 2..];
    }
    out
}

#[tokio::test]
async fn serves_page_and_compiled_modules() {
    let dir = app();
    let server = start(dir.path(), false);

    let (status, html) = get(&server, "/").await;
    assert_eq!(status, 200);
    assert!(html.contains("/@devtools/client.js"), "{html}");
    assert!(html.contains("/@devtools/refresh-preamble.js"), "{html}");

    let (status, main) = get(&server, "/src/main.tsx").await;
    assert_eq!(status, 200, "{main}");
    assert!(main.contains("\"/src/Counter.tsx\""), "{main}");
    assert!(main.contains("\"/src/styles.css?import\""), "{main}");
    assert!(main.contains("\"/src/data.json?import\""), "{main}");
    assert!(main.contains("/node_modules/cjs-lib/index.js"), "{main}");
    assert!(main.contains("\"fixture-app\""), "define applied: {main}");
    assert!(!main.contains("__APP__"), "{main}");

    let (_, counter) = get(&server, "/src/Counter.tsx").await;
    assert!(
        counter.contains("registerExportsForReactRefresh"),
        "refresh boundary: {counter}"
    );
    assert!(
        counter.contains("import.meta.hot = __dm_createHotContext(\"/src/Counter.tsx\")"),
        "{counter}"
    );

    let (_, dep) = get(&server, "/node_modules/cjs-lib/index.js").await;
    assert!(dep.contains("export const __cjs_exports"), "{dep}");
    assert!(dep.contains("\"dev\""), "NODE_ENV folded: {dep}");

    let (_, css) = get(&server, "/src/styles.css?import").await;
    assert!(
        css.contains("rebeccapurple") && css.contains("__dm_updateStyle"),
        "{css}"
    );
    let (_, raw_css) = get(&server, "/src/styles.css").await;
    assert_eq!(raw_css.trim(), "body { color: rebeccapurple; }");

    let (_, json) = get(&server, "/src/data.json?import").await;
    assert!(json.contains("fixture"), "{json}");

    let (status, spa) = get(&server, "/some/client/route").await;
    assert_eq!(status, 200);
    assert!(spa.contains("<div id=\"root\">"), "{spa}");

    assert_eq!(get(&server, "/missing.ts").await.0, 404);
    assert_eq!(get(&server, "/@fs/etc/passwd").await.0, 403);

    let graph = server.state().graph.lock().unwrap();
    let main_node = graph.node(Path::new("/src/main.tsx")).unwrap();
    assert!(main_node.imports.contains(Path::new("/src/Counter.tsx")));
    assert!(
        graph
            .node(Path::new("/src/Counter.tsx"))
            .unwrap()
            .is_self_accepting
    );
}

#[tokio::test]
async fn compile_errors_are_500_with_the_file_name() {
    let dir = app();
    std::fs::write(dir.path().join("src/broken.ts"), "export const = ;").unwrap();
    let server = start(dir.path(), false);
    let (status, body) = get(&server, "/src/broken.ts").await;
    assert_eq!(status, 500);
    assert!(body.contains("broken.ts"), "{body}");
}

#[tokio::test]
async fn edits_become_hmr_frames() {
    let dir = app();
    let root = dir.path().canonicalize().unwrap();
    let server = start(&root, true);
    // Load the modules so they are in the graph.
    for path in [
        "/src/main.tsx",
        "/src/Counter.tsx",
        "/src/styles.css?import",
        "/src/data.json?import",
    ] {
        assert_eq!(get(&server, path).await.0, 200, "{path}");
    }
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{}{HMR_PATH}", server.addr()))
            .await
            .unwrap();
    assert_eq!(next_frame(&mut socket).await["type"], "connected");

    // A refresh boundary: hot update of the component itself.
    write(
        &root.join("src/Counter.tsx"),
        "export function Counter() { return <b>changed</b>; }\n",
    );
    let frame = next_frame(&mut socket).await;
    assert_eq!(frame["type"], "update", "{frame}");
    assert_eq!(frame["updates"][0]["path"], "/src/Counter.tsx", "{frame}");
    // The importer's next compile stamps the updated import.
    let ts = frame["updates"][0]["timestamp"].as_u64().unwrap();
    let (_, main) = get(&server, "/src/main.tsx").await;
    assert!(main.contains(&format!("/src/Counter.tsx?t={ts}")), "{main}");

    // CSS accepts itself under its ?import URL.
    write(&root.join("src/styles.css"), "body { color: teal; }\n");
    let frame = next_frame(&mut socket).await;
    assert_eq!(
        frame["updates"][0]["path"], "/src/styles.css?import",
        "{frame}"
    );

    // JSON has no boundary of its own; main.tsx is the entry and does not accept: reload.
    write(&root.join("src/data.json"), "{ \"name\": \"edited\" }\n");
    let frame = next_frame(&mut socket).await;
    assert_eq!(frame["type"], "full-reload", "{frame}");

    // index.html reloads its page.
    write(
        &root.join("index.html"),
        "<!doctype html><html><head></head><body>v2</body></html>\n",
    );
    let frame = next_frame(&mut socket).await;
    assert_eq!(frame["type"], "full-reload", "{frame}");
    assert_eq!(frame["path"], "/index.html", "{frame}");
}

/// The fixture app with a stand-in `@coldmind/devtools-shell` installed.
fn shell_app() -> tempfile::TempDir {
    let dir = app();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    copy_dir(
        &fixtures.join("shell"),
        &dir.path().join("node_modules/@coldmind/devtools-shell"),
    );
    dir
}

#[tokio::test]
async fn shell_apps_get_the_shell_runtime_as_client() {
    let dir = shell_app();
    let server = start(dir.path(), false);

    let (_, html) = get(&server, "/").await;
    assert!(html.contains("/@devtools/refresh-preamble.js"), "{html}");
    assert!(
        !html.contains("/@devtools/client.js"),
        "the shell opens the socket: {html}"
    );

    let (status, client) = get(&server, "/@devtools/client.js").await;
    assert_eq!(status, 200);
    assert_eq!(
        client.trim(),
        "export * from \"/node_modules/@coldmind/devtools-shell/hot.js\";"
    );

    // Apps without the shell keep the built-in client.
    let plain = app();
    let plain_server = start(plain.path(), false);
    let (_, builtin) = get(&plain_server, "/@devtools/client.js").await;
    assert!(builtin.contains("new WebSocket"), "built-in client");
}

#[tokio::test]
async fn app_channels_reach_the_host_and_the_other_pages() {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    let dir = app();
    let server = start(dir.path(), false);
    let mut messages = server.messages();
    let url = format!("ws://{}{HMR_PATH}", server.addr());
    let (mut a, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut b, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    assert_eq!(next_frame(&mut a).await["type"], "connected");
    assert_eq!(next_frame(&mut b).await["type"], "connected");

    let chat = r#"{"type":"custom","event":"app:chat","data":{"text":"hi"}}"#;
    a.send(Message::Text(chat.into())).await.unwrap();
    let relayed = next_frame(&mut b).await;
    assert_eq!(relayed["event"], "app:chat", "{relayed}");
    assert_eq!(relayed["data"]["text"], "hi", "{relayed}");
    let message = tokio::time::timeout(Duration::from_secs(5), messages.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        message,
        AppMessage {
            channel: "chat".into(),
            data: serde_json::json!({ "text": "hi" })
        }
    );

    // The sender does not get its own message back: its next frame is the host's.
    server.publish("news", serde_json::json!(1));
    let news = next_frame(&mut a).await;
    assert_eq!(
        news,
        serde_json::json!({ "type": "custom", "event": "app:news", "data": 1 })
    );
    assert_eq!(next_frame(&mut b).await["event"], "app:news");
}

fn write(path: &PathBuf, text: &str) {
    std::fs::write(path, text).unwrap();
}

async fn next_frame<S>(socket: &mut S) -> Value
where
    S: futures_util::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("frame within 5 s");
    let text = message.unwrap().unwrap().into_text().unwrap();
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn vite_config_changes_root_alias_and_defines_in_served_modules() {
    let dir = app();
    std::fs::create_dir(dir.path().join("client")).unwrap();
    std::fs::write(
        dir.path().join("client/index.html"),
        "<h1>configured root</h1>",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("client/main.ts"),
        "import {value} from '@value'; console.log(value,__CONFIG__)",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("client/value.ts"),
        "export const value = 'alias applied'",
    )
    .unwrap();
    let value = serde_json::to_string(&dir.path().join("client/value.ts")).unwrap();
    std::fs::write(dir.path().join("vite.config.ts"), format!("export default async ({{command,mode}}) => ({{root:'client',server:{{port:0}},resolve:{{alias:{{'@value':{value}}}}},define:{{__CONFIG__:JSON.stringify(command+':'+mode)}}}})")).unwrap();
    let config = Config::new(dir.path());
    let server = DevServer::start(config).unwrap();
    assert_ne!(server.addr().port(), 5173);
    let (_, page) = get(&server, "/").await;
    assert!(page.contains("configured root"), "{page}");
    let (status, module) = get(&server, "/main.ts").await;
    assert_eq!(status, 200, "{module}");
    assert!(module.contains("serve:development"), "{module}");
    assert!(module.contains("value.ts"), "{module}");
    server.stop();
}

#[tokio::test]
async fn invalid_vite_config_prevents_server_start() {
    let dir = app();
    std::fs::write(dir.path().join("vite.config.js"), "export default null").unwrap();
    assert!(DevServer::start(Config::new(dir.path())).is_err());
}

#[tokio::test]
async fn native_toml_config_is_applied_before_vite_config() {
    let dir = app();
    std::fs::create_dir(dir.path().join(".devtools")).unwrap();
    std::fs::create_dir(dir.path().join("client")).unwrap();
    std::fs::write(
        dir.path().join("client/index.html"),
        "<h1>native configured root</h1>",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("client/main.ts"),
        "import {value} from '@value'; console.log(value,__CONFIG__)",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("client/value.ts"),
        "export const value = 'native alias'",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("vite.config.js"),
        "throw Error('native config must take precedence')",
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".devtools/config.toml"),
        r#"
root = "client"
[server]
host = "127.0.0.1"
port = 0
[resolve.alias]
"@value" = "./client/value.ts"
[define]
__CONFIG__ = '"native-config"'
"#,
    )
    .unwrap();
    let server = DevServer::start(Config::new(dir.path())).unwrap();
    assert!(server.addr().ip().is_loopback());
    assert_ne!(server.addr().port(), 0);
    let (_, page) = get(&server, "/").await;
    assert!(page.contains("native configured root"), "{page}");
    let (status, code) = get(&server, "/main.ts").await;
    assert_eq!(status, 200, "{code}");
    assert!(code.contains("native-config"), "{code}");
    assert!(code.contains("value.ts"), "{code}");
    assert_eq!(get(&server, "/value.ts").await.0, 200);
}
