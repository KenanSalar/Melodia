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
/// strings, preserving first-seen order and stopping at `cap` — fed to
/// `CoverThumbs::prewarm`.
///
/// `cap` is what keeps a detail view over a huge entity from allocating a
/// path per track only for `prewarm` to discard all but the LRU's worth:
/// pass the cache capacity for a full-list prewarm, or the screenful count
/// for a grid. Paths must arrive in **display order** so the prefix that
/// survives the cap is the one that paints first.
pub fn unique_artwork_paths<'a>(
    paths: impl Iterator<Item = Option<&'a str>>,
    cap: usize,
) -> Vec<PathBuf> {
    // Reserve against the input, not the cap — the full-list callers pass a
    // whole cache capacity, and a twelve-track album detail shouldn't lay out
    // hundreds of buckets to fill twelve. An upper hint bounds the items, so
    // it bounds the kept paths too; it's the exact count for the plain `map`
    // callers and merely an over-estimate through a `filter_map`. Either way
    // `.min(cap)` keeps it no worse than the cap it replaced, which is what
    // an iterator with no upper hint falls back to.
    let prealloc = paths.size_hint().1.unwrap_or(cap).min(cap);
    let mut seen: HashSet<&str> = HashSet::with_capacity(prealloc);
    let mut out: Vec<PathBuf> = Vec::with_capacity(prealloc);
    for p in paths.flatten() {
        if out.len() >= cap {
            break;
        }
        if !p.is_empty() && seen.insert(p) {
            out.push(PathBuf::from(p));
        }
    }
    out
}

#[cfg(test)]
#[path = "tests/grid_prewarm_tests.rs"]
mod tests;
