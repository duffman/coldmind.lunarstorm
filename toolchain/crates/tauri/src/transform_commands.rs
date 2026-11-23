//! Tauri v2 adapter for Devtools's shared Rust transformation engine.
//!
//! Register [`crate::init`] and grant `devtools:default` in the window's capability.
//! The `plugin:devtools|transform` command accepts source text, transforms it
//! on a blocking worker, and returns source text. It never reads files, starts
//! a server, executes code, or transpiles TypeScript/JSX into JavaScript.

use std::collections::BTreeMap;

use devtools_native::{DefinePair, Loader, ReplacePair, TransformOptions, Transformer};
use serde::{Deserialize, Serialize};

/// Parser syntax for supplied source text. This does not select an emitter.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceLoader {
    Js,
    Jsx,
    Ts,
    Tsx,
}

impl From<SourceLoader> for Loader {
    fn from(value: SourceLoader) -> Self {
        match value {
            SourceLoader::Js => Self::Js,
            SourceLoader::Jsx => Self::Jsx,
            SourceLoader::Ts => Self::Ts,
            SourceLoader::Tsx => Self::Tsx,
        }
    }
}

/// A literal substitution, including occurrences in strings and comments.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replacement {
    pub from: String,
    pub to: String,
}

/// Whether dependency injection applies to every class instead of marked classes.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InjectAll {
    #[serde(default)]
    pub all: bool,
}

/// `false` disables injection; `true` enables marked classes; `{ "all": true }` enables all classes.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Inject {
    Enabled(bool),
    Options(InjectAll),
}

/// Transform options using the same field names and semantics as the Bun plugin.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub replace: Option<Vec<Replacement>>,
    pub define: Option<BTreeMap<String, String>>,
    pub inject: Option<Inject>,
}

impl From<Options> for TransformOptions {
    fn from(value: Options) -> Self {
        Self {
            replace: value.replace.map(|pairs| {
                pairs
                    .into_iter()
                    .map(|pair| ReplacePair {
                        from: pair.from,
                        to: pair.to,
                    })
                    .collect()
            }),
            define: value.define.map(|pairs| {
                pairs
                    .into_iter()
                    .map(|(key, value)| DefinePair { key, value })
                    .collect()
            }),
            inject_all: match value.inject {
                None | Some(Inject::Enabled(false)) => None,
                Some(Inject::Enabled(true)) => Some(false),
                Some(Inject::Options(options)) => Some(options.all),
            },
        }
    }
}

/// Input to `plugin:devtools|transform`. `filename` labels warnings; it is never opened.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformRequest {
    pub source: String,
    pub loader: SourceLoader,
    #[serde(default = "default_filename")]
    pub filename: String,
    #[serde(default)]
    pub options: Options,
}

fn default_filename() -> String {
    "<source>".into()
}

/// Transformed text and counters for this request only.
#[derive(Debug, Serialize)]
pub struct TransformResult {
    pub code: String,
    pub changed: bool,
    pub parsed: bool,
    pub replaced: u64,
    pub defined: u64,
    pub injected: u64,
    pub warnings: Vec<String>,
}

/// Run the command's transformation without a Tauri runtime.
pub fn transform_source(request: TransformRequest) -> Result<TransformResult, String> {
    let transformer = Transformer::new(request.options.into())?;
    let output =
        transformer.transform_source(&request.filename, &request.source, request.loader.into());
    Ok(TransformResult {
        code: output.code,
        changed: output.changed,
        parsed: output.parsed,
        replaced: output.replaced,
        defined: output.defined,
        injected: output.injected,
        warnings: output.warnings,
    })
}

#[tauri::command]
pub(crate) async fn transform(request: TransformRequest) -> Result<TransformResult, String> {
    tauri::async_runtime::spawn_blocking(move || transform_source(request))
        .await
        .map_err(|error| format!("devtools transform worker failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init;
    use serde_json::{json, Value};

    fn request(value: Value) -> TransformRequest {
        serde_json::from_value(value).unwrap()
    }

    fn ipc_request(body: Value) -> tauri::webview::InvokeRequest {
        tauri::webview::InvokeRequest {
            cmd: "plugin:devtools|transform".into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.into(),
        }
    }

    fn mock_app(allow_transform: bool) -> tauri::App<tauri::test::MockRuntime> {
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        if allow_transform {
            context.runtime_authority_mut().__allow_command(
                "plugin:devtools|transform".into(),
                tauri::utils::acl::ExecutionContext::Local,
            );
        }
        tauri::test::mock_builder()
            .plugin(init())
            .build(context)
            .unwrap()
    }

    /// Runs the router returned by the actual plugin on a host-owned listener.
    /// Shutdown is RAII-managed so assertion failures cannot leave listeners running.
    #[cfg(feature = "dev-server")]
    struct HttpHost {
        address: std::net::SocketAddr,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    #[cfg(feature = "dev-server")]
    impl HttpHost {
        fn start(router: axum::Router) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let (stop, stopped) = tokio::sync::oneshot::channel();
            let thread = std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async move {
                        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                        let (shutdown, shutdown_requested) = tokio::sync::oneshot::channel();
                        let mut server = tokio::spawn(async move {
                            axum::serve(listener, router)
                                .with_graceful_shutdown(async {
                                    let _ = shutdown_requested.await;
                                })
                                .await
                        });
                        let _ = stopped.await;
                        let _ = shutdown.send(());
                        // An open client must not make Drop's thread join wait
                        // forever. Give requests a bounded drain, then cancel the
                        // listener task; dropping this runtime reaps its connections.
                        match tokio::time::timeout(std::time::Duration::from_secs(2), &mut server)
                            .await
                        {
                            Ok(result) => result.unwrap().unwrap(),
                            Err(_) => {
                                server.abort();
                                let _ = server.await;
                            }
                        }
                    });
            });
            Self {
                address,
                stop: Some(stop),
                thread: Some(thread),
            }
        }

        fn get(&self, path: &str) -> String {
            use std::io::{Read, Write};
            let mut socket = std::net::TcpStream::connect_timeout(
                &self.address,
                std::time::Duration::from_secs(5),
            )
            .unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            socket
                .write_all(
                    format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).unwrap();
            response
        }
    }

    #[cfg(feature = "dev-server")]
    impl Drop for HttpHost {
        fn drop(&mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    #[cfg(feature = "dev-server")]
    #[test]
    fn configured_plugin_serves_real_http_and_permission_controlled_ipc() {
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.runtime_authority_mut().__allow_command(
            "plugin:devtools|transform".into(),
            tauri::utils::acl::ExecutionContext::Local,
        );
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".devtools")).unwrap();
        std::fs::create_dir(root.path().join("client")).unwrap();
        std::fs::write(
            root.path().join(".devtools/config.toml"),
            "root = 'client'\n[define]\n__HOST__ = '\"tauri-http\"'\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("vite.config.js"),
            "throw Error('native config must win')",
        )
        .unwrap();
        std::fs::write(
            root.path().join("client/index.html"),
            "<h1>embedded frontend</h1><script type='module' src='/main.ts'></script>",
        )
        .unwrap();
        std::fs::write(root.path().join("client/main.ts"), "import {value} from './dependency'; const host: string = __HOST__; console.log(host,value);").unwrap();
        std::fs::write(
            root.path().join("client/dependency.ts"),
            "export const value: number = 42;",
        )
        .unwrap();
        let app = tauri::test::mock_builder()
            .plugin(crate::Builder::new().development(root.path()).build())
            .build(context)
            .unwrap();
        let host = HttpHost::start(
            crate::development_router(app.handle()).expect("configured frontend router"),
        );
        let page = host.get("/");
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(
            page.contains("embedded frontend") && page.contains("/@devtools/refresh-preamble.js"),
            "{page}"
        );
        let module = host.get("/main.ts");
        assert!(module.starts_with("HTTP/1.1 200"), "{module}");
        assert!(
            module.contains("tauri-http") && module.contains("/dependency.ts"),
            "{module}"
        );
        assert!(
            !module.contains("const host: string") && !module.contains("= __HOST__"),
            "{module}"
        );
        let dependency = host.get("/dependency.ts");
        assert!(dependency.starts_with("HTTP/1.1 200"), "{dependency}");
        assert!(
            dependency.contains("value = 42") && !dependency.contains("value: number"),
            "{dependency}"
        );
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let response = tauri::test::get_ipc_response(
            &webview,
            ipc_request(json!({
                "request": {"source":"export const value = __VALUE__;", "loader":"js",
                    "options":{"define":{"__VALUE__":"7"}}}
            })),
        )
        .unwrap()
        .deserialize::<Value>()
        .unwrap();
        assert_eq!(response["code"], "export const value = 7;");
        assert_eq!(response["defined"], 1);
        let address = host.address;
        drop(host);
        assert!(
            std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(1))
                .is_err()
        );
        drop(webview);
        drop(app);
    }

    #[cfg(feature = "dev-server")]
    #[test]
    fn invalid_native_config_fails_plugin_initialization() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".devtools")).unwrap();
        std::fs::write(
            root.path().join(".devtools/config.toml"),
            "[server]\nport = -1",
        )
        .unwrap();
        let result = tauri::test::mock_builder()
            .plugin(crate::Builder::new().development(root.path()).build())
            .build(tauri::test::mock_context(tauri::test::noop_assets()));
        assert!(
            result.is_err(),
            "invalid native configuration must fail setup"
        );
    }

    #[test]
    fn registered_plugin_dispatches_ipc_results_and_configuration_errors() {
        let app = mock_app(true);
        assert!(crate::development_router(app.handle()).is_none());
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let response = tauri::test::get_ipc_response(
            &webview,
            ipc_request(json!({
                "request": {
                    "source":"export const mode = __MODE__;", "loader":"ts",
                    "options":{"define":{"__MODE__":"1"}}
                }
            })),
        )
        .unwrap()
        .deserialize::<Value>()
        .unwrap();
        assert_eq!(response["code"], "export const mode = 1;");
        assert_eq!(response["defined"], 1);

        let error = tauri::test::get_ipc_response(
            &webview,
            ipc_request(json!({
                "request": {
                    "source":"", "loader":"ts",
                    "options":{"replace":[{"from":"", "to":"a"}]}
                }
            })),
        )
        .unwrap_err();
        assert!(error.as_str().unwrap().contains("must not be empty"));
    }

    #[test]
    fn registered_plugin_rejects_ipc_without_permission() {
        let app = mock_app(false);
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let error = tauri::test::get_ipc_response(
            &webview,
            ipc_request(json!({
                "request":{"source":"", "loader":"js"}
            })),
        )
        .unwrap_err();
        assert!(error.as_str().unwrap().contains("not allowed"));
    }

    #[test]
    fn rejects_unknown_request_options_replacement_and_inject_fields() {
        for value in [
            json!({"source":"", "loader":"ts", "path":"ignored"}),
            json!({"source":"", "loader":"ts", "options":{"unknown":true}}),
            json!({"source":"", "loader":"ts", "options":{"replace":[{"from":"a", "to":"b", "unknown":true}]}}),
            json!({"source":"", "loader":"ts", "options":{"inject":{"all":true, "unknown":true}}}),
        ] {
            assert!(serde_json::from_value::<TransformRequest>(value).is_err());
        }
    }

    #[test]
    fn rejects_unknown_loader_and_missing_source_or_loader() {
        for value in [
            json!({"source":"", "loader":"json"}),
            json!({"source":""}),
            json!({"loader":"ts"}),
        ] {
            assert!(serde_json::from_value::<TransformRequest>(value).is_err());
        }
    }

    #[test]
    fn defaults_to_no_transforms_without_reading_filename() {
        let input = request(json!({"source":"export const n: number = 1;", "loader":"ts"}));
        assert_eq!(input.filename, "<source>");
        let result = transform_source(input).unwrap();
        assert_eq!(result.code, "export const n: number = 1;");
        assert!(!result.changed && !result.parsed);
        assert_eq!(
            (result.replaced, result.defined, result.injected),
            (0, 0, 0)
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn runs_all_stages_with_the_shared_engine() {
        let result = transform_source(request(json!({
            "source":"import { Dep } from './dep'; class A { constructor(d: Dep) {} } export const m = __NAME__;",
            "loader":"ts",
            "filename":"/not/a/real/file.ts",
            "options": {
                "replace":[{"from":"__NAME__", "to":"__MODE__"}],
                "define":{"__MODE__":"\"dev\""},
                "inject":{"all":true}
            }
        }))).unwrap();
        assert!(result.changed && result.parsed);
        assert_eq!(
            (result.replaced, result.defined, result.injected),
            (1, 1, 1)
        );
        assert!(result.code.contains("export const m = \"dev\";"));
        assert!(result
            .code
            .contains("static get $inject() { return [Dep]; }"));
    }

    #[test]
    fn supports_each_source_loader() {
        for loader in ["js", "jsx", "ts", "tsx"] {
            let result = transform_source(request(json!({
                "source":"export const mode = __MODE__;", "loader":loader,
                "options":{"define":{"__MODE__":"1"}}
            })))
            .unwrap();
            assert_eq!(result.code, "export const mode = 1;");
        }
    }

    #[test]
    fn command_worker_returns_serializable_results_and_errors() {
        let result = tauri::async_runtime::block_on(transform(request(json!({
            "source":"export const mode = __MODE__;", "loader":"ts",
            "options":{"define":{"__MODE__":"1"}}
        }))))
        .unwrap();
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(
            value,
            json!({
                "code":"export const mode = 1;", "changed":true, "parsed":true,
                "replaced":0, "defined":1, "injected":0, "warnings":[]
            })
        );

        let result = tauri::async_runtime::block_on(transform(request(json!({
            "source":"", "loader":"ts",
            "options":{"replace":[{"from":"", "to":"a"}]}
        }))));
        assert!(result.unwrap_err().contains("must not be empty"));
    }

    #[test]
    fn rejects_invalid_transform_configuration() {
        for options in [
            json!({"replace":[{"from":"", "to":"a"}]}),
            json!({"define":{"a-b":"1"}}),
            json!({"define":{"__MODE__":"1 +"}}),
        ] {
            assert!(transform_source(request(
                json!({"source":"", "loader":"ts", "options":options})
            ))
            .is_err());
        }
    }

    #[test]
    fn reports_injection_warnings_with_the_supplied_label() {
        let result = transform_source(request(json!({
            "source":"@d class A { constructor(n: number) {} }", "loader":"ts",
            "filename":"input.ts", "options":{"inject":true}
        })))
        .unwrap();
        assert!(!result.changed);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].starts_with("input.ts: devtools inject: A not wired"));
    }

    #[test]
    fn false_disables_injection_and_empty_options_select_marked_classes() {
        for inject in [json!(false), json!({})] {
            let result = transform_source(request(json!({
                "source":"import { Dep } from './dep'; class A { constructor(d: Dep) {} }",
                "loader":"ts", "options":{"inject":inject}
            })))
            .unwrap();
            assert!(!result.changed);
            assert_eq!(result.injected, 0);
        }
    }
}
