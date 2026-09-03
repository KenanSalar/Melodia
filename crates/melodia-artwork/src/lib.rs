//! Decode, resize, the content-addressed artwork store, the thumbnail cache and the palette
//! extraction over it.
//!
//! The tier the other two halves of `media/` lean on, and it points at nothing but core. Its one
//! renderer-adjacent dependency is `slint`'s `SharedPixelBuffer` — a refcounted pixel buffer, the
//! same category of thing as `bytes::Bytes`, and never the event loop or a widget. The argument
//! for that, and for the two alternatives that cost more than they save, is in
//! `docs/plans/WORKSPACE_SPLIT.md`.

pub mod media {
    pub mod image;
}
