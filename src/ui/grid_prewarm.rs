//! Small shared helper for the entity grids' cover-prewarm paths.
//!
//! Every entity grid (`albums`, `artists`, `playlists`) and the genre detail
//! view turns an iterator of optional artwork-path strings into a
//! deduplicated, display-ordered `Vec<PathBuf>` to hand to
//! `CoverThumbs::prewarm`. The per-entity `first_screenful_paths` wrappers
//! still own the entity-specific projection (which field, how many ahead);
//! only this dedup core is shared.

use std::collections::HashSet;
use std::path::PathBuf;

/// Deduplicated, non-empty artwork paths from an iterator of optional path
/// strings, preserving first-seen order — fed to `CoverThumbs::prewarm`
/// (which itself caps work at the LRU capacity, so passing paths in display
/// order keeps the kept prefix the one that paints first).
pub fn unique_artwork_paths<'a>(paths: impl Iterator<Item = Option<&'a str>>) -> Vec<PathBuf> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths.flatten() {
        if !p.is_empty() && seen.insert(p) {
            out.push(PathBuf::from(p));
        }
    }
    out
}
