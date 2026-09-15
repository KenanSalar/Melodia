//! `settings.json` persistence. [`data`] is the root model, [`SettingsData`]; its `*Flags`
//! substructs sit one file per Settings tab that edits them ([`playback`], [`interface`],
//! [`library`], [`services`], [`about`]); [`io`] is load / save / atomic read-mutate-write. All are
//! re-exported, so every `crate::services::settings::*` path resolves unchanged.
//!
//! Every `*Flags` substruct follows one shape: `#[serde(flatten)]`'d into
//! [`SettingsData`] so its fields still serialize at the top level of the file,
//! and carrying a whole-struct `#[serde(default)]` so an install written before
//! the feature loads anyway. The grouping exists to keep each struct under
//! clippy's `struct_excessive_bools` budget; the flatten is what keeps that
//! free of on-disk consequences. Per the shipped-app rule, anything with new
//! visible behavior defaults off. Flipping a default costs no existing install
//! either way — nothing here is `skip_serializing_if`, so a file written by any
//! previous build already spells every key and keeps its own value, and the new
//! default reaches fresh installs alone.

mod about;
mod data;
mod interface;
mod io;
mod library;
mod playback;
mod services;

pub use about::*;
pub use data::*;
pub use interface::*;
pub use io::*;
pub use library::*;
pub use playback::*;
pub use services::*;
