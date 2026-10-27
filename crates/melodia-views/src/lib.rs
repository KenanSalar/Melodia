//! The Slint bridge: the view slices, the shared component library under them, and the callbacks
//! wiring the two together.
//!
//! **No `melodia-store`, no `melodia-net`**, and the manifest is what says so: the UI reaches the
//! database through `melodia-app`'s library API and opens no socket, and nothing below re-exports
//! either, so `melodia_store::database` does not resolve here rather than resolving to something
//! private. Why it isn't split further is `docs/adr/0018-what-is-not-split-and-zero-features.md`'s.

pub mod ui;
