//! Browser host (spec 6.3, Browser): a model project in a browser tab with its own
//! in-page host.
//!
//! A loopback HTTP server hands the tab the build: `index.html` with the in-page host
//! and the shell injected, the frontend files, and under `/__zebra/` the backend Worker,
//! the backend (its bare `@desktop/*` imports rewritten) and the two built-in modules.
//! Everything else runs in the page: see `host.js` and `worker.js`.
//!
//! Decisions:
//! - std `TcpListener`, one thread per connection, `Connection: close`: a dev tool for one
//!   tab needs no HTTP crate.
//! - Bound to loopback only; responses are `no-store`, like the custom protocol.

use std::borrow::Cow;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use lunarstorm_platform::assets::mime;
use lunarstorm_platform::pipeline::BuildOutput;

/// The in-page host; `__ZEBRA_CONFIG__` is replaced with the host configuration.
const HOST: &str = include_str!("host.js");
/// The backend Worker.
const WORKER: &str = include_str!("worker.js");
/// The shell every WebView gets (`__ZEBRA_BOOT__` is replaced).
const SHELL: &str = lunarstorm_platform::ui::SHELL;

/// `@desktop/bridge` as the Worker imports it: a view of the `globalThis.__zebra`
/// object `worker.js` sets up before importing the backend.
///
/// Decision: these two sources moved here from the desktop process's former JS
/// thread, whose own modules now come from the desktop app host; the Worker is
/// their only consumer.
const BRIDGE_MODULE: &str = "const b = globalThis.__zebra.bridge;\n\
export const setProperty = b.setProperty;\nexport const serve = b.serve;\nexport const restored = b.restored;\nexport default b;\n";

/// `@desktop/api` as the Worker imports it.
const API_MODULE: &str = "export const native = globalThis.__zebra.native;\nexport default native;\n";

/// Builds `index.html` with the in-page host and the shell injected first in `<head>`.
///
/// # Arguments
/// * `build` - the pipeline output.
///
/// # Returns
/// The page, or `None` when the build has no `index.html`.
pub fn page(build: &BuildOutput) -> Option<String> {
    let html = String::from_utf8_lossy(build.files.get("index.html")?).into_owned();
    let main = &build.config.application.main_form;
    // `<\/` keeps a `</script>` inside a JSON string from closing the inline script.
    let config = serde_json::to_string(&build.config).expect("config serializes").replace("</", "<\\/");
    let boot = serde_json::json!({ "windowId": main, "epoch": 1, "mode": "browser" }).to_string();
    let scripts = format!(
        "<script>{}</script><script>{}</script>",
        HOST.replace("__ZEBRA_CONFIG__", &config),
        SHELL.replace("__ZEBRA_BOOT__", &boot)
    );
    let at = html.find("<head").and_then(|start| html[start..].find('>').map(|end| start + end + 1)).unwrap_or(0);
    Some(format!("{}{}{}", &html[..at], scripts, &html[at..]))
}

/// Points the backend's bare built-in imports at the served built-in modules.
///
/// # Arguments
/// * `backend` - backend artifact (4c).
///
/// # Returns
/// The backend as a browser ES module.
pub fn backend_module(backend: &str) -> String {
    let mut out = backend.to_owned();
    for (name, url) in [("@desktop/bridge", "/__zebra/bridge.js"), ("@desktop/api", "/__zebra/api.js")] {
        for quote in ['\'', '"'] {
            out = out.replace(&format!("{quote}{name}{quote}"), &format!("{quote}{url}{quote}"));
        }
    }
    out
}

/// Resolves one request path.
///
/// # Arguments
/// * `build` - the pipeline output.
/// * `path` - request path without the query string.
///
/// # Returns
/// Body and content type, or `None` for 404.
pub fn resolve<'a>(build: &'a BuildOutput, path: &str) -> Option<(Cow<'a, [u8]>, &'static str)> {
    let js = "text/javascript; charset=utf-8";
    let owned = |text: String| Cow::Owned(text.into_bytes());
    match path.trim_start_matches('/') {
        "" | "index.html" => page(build).map(|html| (owned(html), "text/html; charset=utf-8")),
        "__zebra/worker.js" => Some((Cow::Borrowed(WORKER.as_bytes()), js)),
        "__zebra/backend.js" => Some((owned(backend_module(&build.backend)), js)),
        "__zebra/bridge.js" => Some((Cow::Borrowed(BRIDGE_MODULE.as_bytes()), js)),
        "__zebra/api.js" => Some((Cow::Borrowed(API_MODULE.as_bytes()), js)),
        file => build.files.get(file).map(|bytes| (Cow::Borrowed(&bytes[..]), mime(file))),
    }
}

/// Serves the build until the process ends.
///
/// # Arguments
/// * `build` - the pipeline output.
/// * `listener` - a loopback listener.
///
/// # Errors
/// An accept error.
pub fn serve(build: BuildOutput, listener: TcpListener) -> io::Result<()> {
    let build = Arc::new(build);
    for stream in listener.incoming() {
        let stream = stream?;
        let build = Arc::clone(&build);
        thread::spawn(move || {
            if let Err(error) = respond(&build, stream) {
                eprintln!("lunarstorm-web: {error}");
            }
        });
    }
    Ok(())
}

/// Answers one HTTP/1.1 request (GET and HEAD; anything else is 405).
///
/// # Arguments
/// * `build` - the pipeline output.
/// * `stream` - the connection.
///
/// # Errors
/// A read or write error on the connection.
fn respond(build: &BuildOutput, mut stream: TcpStream) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    // Drain the headers; a GET has no body.
    let mut header = String::new();
    while reader.read_line(&mut header)? > 2 {
        header.clear();
    }
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let (status, body, kind) = match (method, resolve(build, path)) {
        ("GET" | "HEAD", Some((body, kind))) => ("200 OK", body, kind),
        ("GET" | "HEAD", None) => ("404 Not Found", Cow::Borrowed(&b"not found"[..]), "text/plain"),
        _ => ("405 Method Not Allowed", Cow::Borrowed(&b"method not allowed"[..]), "text/plain"),
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if method != "HEAD" {
        stream.write_all(&body)?;
    }
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunarstorm_platform::pipeline::{OutputDirPipeline, Pipeline};

    /// The example app in `platform/examples/app`, built through the real pipeline.
    fn sample_build() -> BuildOutput {
        OutputDirPipeline::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/app")).build().expect("example builds")
    }

    /// The host runs before the shell, and both run before the page's own scripts.
    #[test]
    fn page_injects_host_then_shell_before_page_scripts() {
        let html = page(&sample_build()).unwrap();
        let host = html.find("In-page host").unwrap();
        let shell = html.find("Framework-owned shell runtime").unwrap();
        let app = html.find("main.js").unwrap();
        assert!(host < shell && shell < app);
        assert!(!html.contains("__ZEBRA_CONFIG__") && !html.contains("__ZEBRA_BOOT__"));
        assert!(html.contains(r#""mode":"browser""#));
    }

    /// Bare built-in imports point at the served modules; other text is untouched.
    #[test]
    fn backend_imports_are_rewritten() {
        let src = "import { serve } from '@desktop/bridge';\nimport { native } from \"@desktop/api\";\nconst s = 'x';";
        let out = backend_module(src);
        assert!(out.contains("from '/__zebra/bridge.js'") && out.contains("from \"/__zebra/api.js\""));
        assert!(out.ends_with("const s = 'x';"));
    }

    /// Served paths: page, internals, frontend files, 404.
    #[test]
    fn resolves_paths() {
        let build = sample_build();
        assert_eq!(resolve(&build, "/").unwrap().1, "text/html; charset=utf-8");
        assert!(resolve(&build, "/__zebra/worker.js").is_some());
        assert!(!String::from_utf8_lossy(&resolve(&build, "/__zebra/backend.js").unwrap().0).contains("'@desktop/"));
        assert!(resolve(&build, "/vcl.css").unwrap().1.starts_with("text/css"));
        assert!(resolve(&build, "/missing.js").is_none());
    }
}
