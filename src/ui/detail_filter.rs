//! Shared detail-view track filtering.
//!
//! The Album / Genre / Playlist / Artist detail views each carry a hero
//! `SearchBar` that filters the track list in memory. The filter walk,
//! the selection re-stamp, and the displayed-cache bookkeeping are
//! byte-identical across them — the only differences are the per-view
//! Slint global type and the concrete `*Ui` struct holding the caches.
//! This module captures that logic once, parameterised over the
//! [`DetailSelectionView`] trait (the same one the selection logic uses)
//! and a [`FilterRefs`] borrow of the four detail caches.
//!
//! Album / Genre / Playlist Detail run the whole pass on the UI thread
//! and call [`apply_filtered_detail`] directly. Artist Detail does its
//! own worker-thread row prep (it also rebuilds an Albums strip), so it
//! reuses only [`track_matches`] and [`restamp_selection`].

use std::collections::HashSet;

use parking_lot::Mutex;
use slint::{Model, ModelRc, VecModel};

use crate::entities::track::TrackListRow as RsTrackListRow;
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::detail_selection::DetailSelectionView;
use crate::TrackListRow as UiTrackListRow;

/// Lowered-needle case-insensitive substring match across a track row's
/// title + artist + album. `needle` must already be lowercased (the
/// caller stores it that way in `*DetailState::filter`).
pub fn track_matches(r: &RsTrackListRow, needle: &str) -> bool {
    field_contains(&r.title, needle)
        || r.artist.as_deref().is_some_and(|a| field_contains(a, needle))
        || r.album.as_deref().is_some_and(|a| field_contains(a, needle))
}

/// Case-insensitive substring check without the per-call `to_lowercase`
/// heap allocation on the (overwhelmingly common) all-ASCII path. The
/// filter walk runs per throttled keystroke over every row of the open
/// detail — a "Rock" genre detail can hold thousands of rows, and the
/// old three-allocations-per-row walk dominated its keystroke cost.
/// Non-ASCII text falls back to the allocating Unicode-correct path.
/// `needle` must already be lowercased.
fn field_contains(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        let h = haystack.as_bytes();
        let n = needle.as_bytes();
        if n.len() > h.len() {
            return false;
        }
        return h
            .windows(n.len())
            .any(|w| w.iter().zip(n).all(|(a, b)| a.to_ascii_lowercase() == *b));
    }
    haystack.to_lowercase().contains(needle)
}

/// Re-apply selection from the view's `selected-ids` onto freshly-built
/// rows before they're swapped into the Slint model —
/// `to_slint_track_list_row` always defaults `selected` to `false`, so a
/// filter rebuild would otherwise drop every checkbox + accent highlight.
pub fn restamp_selection<V: DetailSelectionView>(view: &V, rows: &mut [UiTrackListRow]) {
    let selected: HashSet<i32> = view.selected_ids().iter().collect();
    if selected.is_empty() {
        return;
    }
    for row in rows {
        if selected.contains(&row.id) {
            row.selected = true;
        }
    }
}

/// Borrows of the four Rust-side caches a detail view keeps for its
/// track filter. `all_tracks` is the canonical full set the filter walks;
/// `tracks` is the displayed (filter-applied) subset, kept in lockstep
/// with the Slint model so the selection/sort logic stays valid.
pub struct FilterRefs<'a> {
    /// Canonical full track set, in display-sort order.
    pub all_tracks: &'a Mutex<Vec<RsTrackListRow>>,
    /// Displayed (filter-applied) subset — overwritten by every apply.
    pub tracks: &'a Mutex<Vec<RsTrackListRow>>,
    /// Selection set currently stamped onto the Slint row model.
    pub applied: &'a Mutex<HashSet<i32>>,
    /// Live lowercased filter needle, mirroring the Slint `filter` prop.
    pub filter: &'a Mutex<String>,
}

/// Re-walk the canonical `all_tracks` cache through the current filter
/// needle, push the filtered rows into the Slint model, and store the
/// filtered subset back into the displayed `tracks` cache so it stays in
/// lockstep with the model. Selection is re-stamped onto the surviving
/// rows; the shift-range anchor is dropped (the model changed shape) and
/// the applied-selection shadow is re-synced. Runs on the UI thread.
///
/// Used directly by Album / Genre / Playlist Detail. Artist Detail keeps
/// its own variant (worker-thread prep + Albums strip) but reuses
/// [`track_matches`] / [`restamp_selection`] from this module.
pub fn apply_filtered_detail<V: DetailSelectionView>(
    view: &V,
    refs: &FilterRefs<'_>,
    thumbs: &CoverThumbs,
) {
    let needle = refs.filter.lock().clone();

    let displayed: Vec<RsTrackListRow> = {
        let all = refs.all_tracks.lock();
        all.iter()
            .filter(|r| needle.is_empty() || track_matches(r, &needle))
            .cloned()
            .collect()
    };
    let mut rows: Vec<UiTrackListRow> = displayed
        .iter()
        .map(|t| crate::ui::tracks::to_slint_track_list_row(t, thumbs))
        .collect();
    restamp_selection(view, &mut rows);
    *refs.tracks.lock() = displayed;
    install_tracks(view, rows);
    // The displayed model changed shape — a stale shift-range anchor
    // would now index the wrong row, so drop it. `restamp_selection`
    // already stamped the rows, so the `applied` shadow just mirrors the
    // current `selected-ids` set for the next incremental click diff.
    view.set_anchor(-1);
    *refs.applied.lock() = view.selected_ids().iter().collect();
}

/// Swap the view's `tracks` `VecModel` contents in place, falling back to
/// a fresh model if the downcast fails — never expected in practice (the
/// model is always installed as a `VecModel` at startup). Mirrors the
/// `replace_tracks_model` detail-view helper so the model never desyncs
/// from the `tracks` cache on the dead path.
fn install_tracks<V: DetailSelectionView>(view: &V, rows: Vec<UiTrackListRow>) {
    let model = view.track_rows();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<UiTrackListRow>>() {
        vm.set_vec(rows);
    } else {
        view.replace_track_rows(ModelRc::new(VecModel::from(rows)));
    }
}

#[cfg(test)]
#[path = "tests/detail_filter_tests.rs"]
mod tests;
