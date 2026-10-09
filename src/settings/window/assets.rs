//! The page's files (`ui/dist/`, built by trunk): embedded in a release build, read from disk in a
//! debug build (rust-embed's default), so `trunk watch` beside a debug Kai shows a change at once.
//! A build without `ui/dist/` (a plain `cargo build`, the unit tests) embeds nothing and the
//! window says so; `packaging/build-rpm.sh` and CI refuse to build a package without it.

use std::borrow::Cow;

use base64::Engine;
use sha2::{Digest, Sha256};

#[derive(rust_embed::RustEmbed)]
#[folder = "ui/dist/"]
#[allow_missing = true]
struct Dist;

/// One file of the page.
pub struct Asset {
    pub mime: &'static str,
    pub body: Cow<'static, [u8]>,
}

/// The file at `path` (the URL's path, with its leading slash; `/` is the page). Nothing hidden
/// (trunk's `.stage`), nothing above `dist/`.
pub fn get(path: &str) -> Option<Asset> {
    let name = match path.strip_prefix('/').unwrap_or(path) {
        "" => "index.html",
        other => other,
    };
    if name.split('/').any(|s| s.is_empty() || s.starts_with('.')) {
        return None;
    }
    Dist::get(name).map(|f| Asset {
        mime: mime_of(name),
        body: f.data,
    })
}

/// Whether the page is there at all.
pub fn built() -> bool {
    Dist::get("index.html").is_some()
}

fn mime_of(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        // Streaming compilation of the module needs exactly this type.
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// The page's CSP: K10's (nothing but the page's own files, no base, no form), widened by what the
/// WASM build needs and nothing else: `wasm-unsafe-eval` to compile the module, `connect-src 'self'`
/// for Kai's routes and the profile editor's requests (lmgw's answers come through the scheme
/// handler, so no network origin is named), `media-src blob:` for the editor's "Speak a sample"
/// (an `<audio>` on an object URL of the WAV lmgw answered), and the hash of each inline script the
/// document carries (trunk's one line that starts the module), so no inline script can be added
/// without the hash changing.
pub fn csp(index: &[u8]) -> String {
    let mut script = String::from("'self' 'wasm-unsafe-eval'");
    for hash in inline_script_hashes(index) {
        script.push_str(&format!(" 'sha256-{hash}'"));
    }
    format!(
        "default-src 'none'; script-src {script}; style-src 'self'; connect-src 'self'; \
         img-src 'self' data:; media-src blob:; base-uri 'none'; form-action 'none'; \
         frame-ancestors 'none'"
    )
}

/// The base64 SHA-256 of the text of every `<script>` without a `src`.
fn inline_script_hashes(html: &[u8]) -> Vec<String> {
    let html = String::from_utf8_lossy(html);
    let mut hashes = Vec::new();
    let mut rest = &html[..];
    while let Some(at) = rest.find("<script") {
        rest = &rest[at + "<script".len()..];
        let Some(open_end) = rest.find('>') else {
            break;
        };
        let attrs = &rest[..open_end];
        let body = &rest[open_end + 1..];
        let Some(close) = body.find("</script>") else {
            break;
        };
        if !attrs.contains("src=") {
            let digest = Sha256::digest(&body.as_bytes()[..close]);
            hashes.push(base64::engine::general_purpose::STANDARD.encode(digest));
        }
        rest = &body[close..];
    }
    hashes
}
