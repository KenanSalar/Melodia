//! `settings.json`: [`data`]'s root model [`SettingsData`], its `*Flags` one file per area of
//! Settings, and [`io`]'s load, save and atomic read-mutate-write, all re-exported.
//!
//! Each `*Flags` is `#[serde(flatten)]`'d, so its keys sit at the file's top level, and carries a
//! whole-struct `#[serde(default)]`, so a file older than it loads. The split is for clippy's
//! `struct_excessive_bools`. Nothing is `skip_serializing_if`, so a file spells every key its build
//! knew, and a changed default reaches only fresh installs and files older than the key.

mod about;
mod data;
mod interface;
mod io;
mod library;
mod playback;
mod services;
mod undo;

pub use about::*;
pub use data::*;
pub use interface::*;
pub use io::*;
pub use library::*;
pub use playback::*;
pub use services::*;
pub use undo::*;
