//! Decode, resize, store and sample a cover.
//!
//! Four of the five modules here name nothing outside this directory, which is what lets `ingest`
//! and `fetch` both sit above it. [`material_you`] is the exception and is not an accident: it
//! reads a decoded cover and hands back a [`melodia_core::themes::Palette`], so it is the adapter
//! between the two, and it is here rather than in `themes/` because a filter change in it moves
//! every generated colour and the resampler it must agree with is this tier's.
//!
//! That one edge decides a manifest line the split has not written yet: either the artwork crate
//! depends on the one holding the theme registry, or `Palette` is plain enough to belong further
//! down. Still open.

pub mod artwork;
pub mod cover_thumbs;
pub mod image_decode;
pub mod logo_tile;
pub mod material_you;
