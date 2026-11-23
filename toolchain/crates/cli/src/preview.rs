//! Serves an existing production build on loopback; never builds or clears output.
use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use std::path::{Path, PathBuf};

/// Resolves native configuration (or the compatibility fallback) before binding.
/// An explicit output directory wins; preview deliberately binds loopback only.
pub async fn run(root: PathBuf, out: Option<PathBuf>, port: u16) -> anyhow::Result<()> {
    let config = devermind_core::config::load_preview(&root, "production")?;
    let root = config
        .as_ref()
        .and_then(|config| config.root.clone())
        .unwrap_or(root);
    let out = out
        .or_else(|| config.and_then(|config| config.build_out_dir))
        .unwrap_or_else(|| "dist".into());
    let dir = if out.is_absolute() {
        out
    } else {
        root.join(out)
    };
    let dir = dir.canonicalize().map_err(|error| {
        anyhow::anyhow!("{}: {error} (run `jetbunny --build` first)", dir.display())
    })?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!(
        "devermind preview of {} at http://{}/",
        dir.display(),
        listener.local_addr()?
    );
    axum::serve(listener, Router::new().fallback(serve).with_state(dir)).await?;
    Ok(())
}

/// Limits lexical traversal and falls back to index.html for extensionless SPA routes.
async fn serve(State(dir): State<PathBuf>, request: Request<Body>) -> Response {
    let path = request.uri().path();
    let mut file = dir.clone();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        if part == ".." || part.contains('\\') {
            return StatusCode::FORBIDDEN.into_response();
        }
        file.push(part);
    }
    if file.is_dir() {
        file.push("index.html");
    }
    // SPA fallback: an extensionless path is a client route.
    if !file.is_file() && Path::new(path).extension().is_none() {
        file = dir.join("index.html");
    }
    match tokio::fs::read(&file).await {
        Ok(bytes) => {
            let mime = mime_of(&file);
            ([(header::CONTENT_TYPE, mime)], bytes).into_response()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn mime_of(file: &Path) -> &'static str {
    match file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}
