//! `settings.json` persistence, split into two cohesive halves:
//!
//! * [`data`] — the serde data model ([`SettingsData`] and its substructs),
//!   first-launch defaults, and the OS / desktop-environment probes those
//!   defaults rely on.
//! * [`io`] — load / save / atomic read-mutate-write of the file on disk.
//!
//! Both halves are re-exported here, so every existing
//! `crate::services::settings::*` path keeps resolving unchanged.

mod data;
mod io;

pub use data::*;
pub use io::*;
