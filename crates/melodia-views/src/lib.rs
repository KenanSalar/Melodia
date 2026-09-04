//! The Slint bridge: twenty view slices, the shared component library under them, and the
//! callbacks that wire the two together.
//!
//! It sits above every other crate and below nothing, which is what lets the exclusion be a
//! manifest rather than a convention: **no `melodia-store`, no `melodia-net`**. The UI reaches
//! the database through `melodia-app`'s library API, and it opens no socket at all. Nothing
//! below re-exports either, so `melodia_store::database` does not resolve here at all rather
//! than resolving to something private.
//!
//! Not split further, and the argument is in
//! `docs/adr/0018-what-is-not-split-and-zero-features.md`: the slices are a dense mesh, the
//! component library imports fourteen of them, and cutting it needs a view registry nothing else
//! in the tree wants.

pub mod ui;
