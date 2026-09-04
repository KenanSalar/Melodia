//! The compiled Slint UI, split out of `melodia` so it builds once.
//!
//! `slint::include_modules!()` pulls in one enormous generated compilation unit.
//! Inside `melodia` it was rebuilt on every Rust change and built twice per
//! `cargo test`; here it moves only when a `.slint` source or a translation
//! catalog does. **Nothing re-exports it**, so every call site spells
//! `melodia_ui::AppWindow`, `melodia_ui::TrackRow` and the rest, and the import
//! says where the type is generated.

// Generated code — a lint hit would land in a file that doesn't exist until build
// time. `warnings` covers every warn-level entry in `[workspace.lints]` including
// ones added later; `clippy::all` folds in the denied correctness / suspicious /
// perf groups. Only a *new deny* in that table needs a line here.
#[allow(warnings, unsafe_code, clippy::all, clippy::unwrap_used)]
mod generated_ui {
    slint::include_modules!();
}
pub use generated_ui::*;
