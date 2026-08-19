//! `settings.json` persistence: [`data`] is the serde model ([`SettingsData`] and
//! its substructs), first-launch defaults and the OS / desktop-environment probes
//! those rely on; [`io`] is load / save / atomic read-mutate-write. Both are
//! re-exported, so every `crate::services::settings::*` path resolves unchanged.

mod data;
mod io;

pub use data::*;
pub use io::*;
