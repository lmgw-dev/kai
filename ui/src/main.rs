//! The wasm entry point trunk builds. Natively there is nothing to run: the page is Kai's.

fn main() {
    #[cfg(target_arch = "wasm32")]
    kai_ui::mount();
}
