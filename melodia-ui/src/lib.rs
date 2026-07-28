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

// The module below is generated, so none of the workspace lints can be acted on
// inside it — a hit would land in a file that doesn't exist until build time.
// `warnings` catches every warn-level entry in `[workspace.lints]`, including
// ones added later. The rest are the deny-level entries it can't reach:
// `clippy::all` folds in the denied `correctness`/`suspicious`/`perf` groups —
// and with them `await_holding_lock` / `await_holding_refcell_ref`, which live
// in `suspicious` — leaving `unsafe_code` and `clippy::unwrap_used` to name.
// Only a *new deny* in that table needs a line here.
#[allow(warnings, unsafe_code, clippy::all, clippy::unwrap_used)]
mod generated_ui {
    slint::include_modules!();
}
pub use generated_ui::*;
