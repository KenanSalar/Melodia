//! The compiled Slint UI, split out of `melodia` so it builds once.
//!
//! `slint::include_modules!()` pulls in the component tree the Slint compiler
//! generates from `ui/app-window.slint` — one enormous compilation unit. While
//! it lived in `melodia` it was built twice per `cargo test` (once as the rlib
//! the bin and the integration tests link, once as the `--test` harness) and
//! rebuilt whenever any Rust file in that crate changed. Here it builds once,
//! and only when a `.slint` source or a translation catalog moves.
//!
//! `melodia` re-exports all of it with `pub use melodia_ui::*;`, so call sites
//! keep naming the generated types as `crate::AppWindow`, `crate::TrackRow` and
//! so on.

#[allow(
    unsafe_code,
    clippy::all,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::dbg_macro,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::await_holding_lock,
)]
mod generated_ui {
    slint::include_modules!();
}
pub use generated_ui::*;
