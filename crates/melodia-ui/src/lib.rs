//! The compiled Slint UI, a crate of its own so the generated unit rebuilds only when a `.slint`
//! source or a translation catalog moves. **Nothing re-exports it**: every call site spells
//! `melodia_ui::AppWindow`, so the import says where the type is generated.

// `warnings` covers every warn-level workspace lint, later ones included; the other three are the
// denies it can't reach, so only a new deny in `[workspace.lints]` needs a line here.
#[allow(warnings, unsafe_code, clippy::all, clippy::unwrap_used)]
mod generated_ui {
    slint::include_modules!();
}
pub use generated_ui::*;
