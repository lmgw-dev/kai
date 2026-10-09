//! Kai's configuration window (WP21b, K29): the page the GTK toplevel shows on `kai://settings/`.
//! It reaches Kai only through the scheme handler's routes (`wire`), shows the tool policy, the
//! desktop providers and every setting, and writes through the same operations as the owner tools.
//! Its Personality page mounts lmgw's profile editor (the kit's `ProfilesPanel`), whose requests go
//! to lmgw through the scheme handler too (under [`LMGW_BASE`]). `wire`, `model` and `conversation`
//! are plain Rust and run natively in tests; the DOM side is wasm only.

pub mod conversation;
pub mod model;
pub mod wire;

#[cfg(target_arch = "wasm32")]
mod api;
#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod personality;

/// The kit's HTTP base: the scheme handler forwards lmgw's routes under it, with Kai's device key
/// (`src/settings/window/profiles.rs`'s `PREFIX`).
pub const LMGW_BASE: &str = "/lmgw";

/// Mounts the page on `<body>`.
#[cfg(target_arch = "wasm32")]
pub fn mount() {
    console_error_panic_hook::set_once();
    // No unauthorized hook: lmgw's `session_required` is a dashboard session's, and a device key
    // it refuses is said by the refusal itself, inline where the request was made.
    lmgw_ui_kit::http::configure(lmgw_ui_kit::http::Config {
        base: LMGW_BASE.to_owned(),
        on_unauthorized: None,
    });
    leptos::mount::mount_to_body(app::App);
}
