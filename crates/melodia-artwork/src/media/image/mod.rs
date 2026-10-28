//! Decode, resize, store and sample a cover.
//!
//! [`material_you`] turns a decoded cover into a `themes::Palette` and lives on this side because
//! its colours move with this tier's decode and resampling. [`decoded`] reports a missing stored
//! file over `melodia_core`'s bridge, the one way a cache here reaches the library restoring it.

pub mod artwork;
pub mod cover_thumbs;
pub mod decoded;
pub mod image_decode;
pub mod logo_tile;
pub mod material_you;
