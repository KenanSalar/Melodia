//! The cached entities → the Slint model: the filtered walk, the chunk, and the two ways an apply
//! reaches the UI thread.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use super::warm::mounted_content;
use crate::ui::grid_rows::{chunk_entity_rows, write_grid};
use crate::ui::recently_played::{
    RecentlyPlayedTab, RecentlyPlayedUi, tab_from_index, to_slint_most_played_row,
};
use crate::ui::row_match::most_played_matches;
use crate::ui::tab_bar::{grid_signature, should_announce_warm};
use crate::ui::util::len_as_i32;
use crate::{AppWindow, EntityStripRow as UiEntityStripRow, RecentlyPlayed};

/// The Most Played tab's filtered rows, prepared away from the UI thread by
/// [`build_filtered_grid`] and consumed by [`write_filtered_grid`].
pub(super) struct PreparedGrid {
    /// Which tab these rows were built for. Carried rather than re-derived because
    /// [`build_filtered_grid`] may run on a worker and [`write_filtered_grid`] always runs on the
    /// UI thread, so a pick can land in the gap.
    tab: RecentlyPlayedTab,
    /// Empty unless [`Self::tab`] is `MostPlayed`.
    pub(super) most_played: Vec<UiEntityStripRow>,
    /// The filtered count gating the grid's `GridEmptyState`. `0` on Songs, where nothing reads it
    /// — the band takes its facts from `RecentlyPlayedUiState::most_played_totals` instead.
    pub(super) most_played_count: usize,
    /// Hash of everything reaching a card, off the **source** entities rather than the built rows:
    /// `#[derive(Hash)]` stays complete when a field is added, where a hand-listed set goes stale.
    pub(super) most_played_content: u64,
}

/// Re-walk the cached `most_played` through the current filter, hashing and counting the survivors
/// as it goes. Entirely in memory and touching no Slint state, so either thread can call it — the
/// mounted tab comes off the [`RecentlyPlayedUi`] shadow, the only form of that answer a worker
/// can read.
///
/// **Nothing is walked on the Songs tab**, the mounted-tab-only rule
/// `songs::build_filtered_tracks` holds from the other side. The walk is far from free: the query
/// behind this cache is uncapped and library-wide, and [`apply_filtered_grid_now`] calls this **on
/// the UI thread**. The bail is safe because [`mounted_content`] already collapses the Songs arm
/// to a constant `0`, and the tab pick rebuilds the count before the grid it gates can mount.
pub(super) fn build_filtered_grid(rp_ui: &RecentlyPlayedUi) -> PreparedGrid {
    let tab = rp_ui.active_tab();
    if tab != RecentlyPlayedTab::MostPlayed {
        return PreparedGrid {
            tab,
            most_played: Vec::new(),
            most_played_count: 0,
            most_played_content: 0,
        };
    }

    let needle = rp_ui.state().filter.lock().clone();
    let mut hasher = DefaultHasher::new();
    let cache = rp_ui.state().most_played.lock();
    // An empty needle keeps every row of a library-sized cache, where a `Filter`'s `size_hint`
    // floor of `0` would grow the `Vec` from nothing. A real needle reserves nothing — no cheap
    // thing predicts the survivor count.
    let mut most_played: Vec<UiEntityStripRow> =
        Vec::with_capacity(if needle.is_empty() { cache.len() } else { 0 });
    most_played.extend(
        cache
            .iter()
            .filter(|t| most_played_matches(t, &needle))
            .inspect(|t| t.hash(&mut hasher))
            .map(to_slint_most_played_row),
    );
    drop(cache);

    let most_played_count = most_played.len();
    PreparedGrid {
        tab,
        most_played,
        most_played_count,
        most_played_content: hasher.finish(),
    }
}

/// Chunk the prepared rows into cards and push them into the grid's model. UI thread only, and
/// takes `prepared` **by value** so the rows move into the per-row models rather than being cloned.
///
/// The count is published unchunked beside the model, `rows.length` being a row count where
/// `GridEmptyState` wants cards. On Songs it is a constant `0`, safe because every reader sits
/// inside the Most Played branch or under a `tab-idx` gate.
///
/// **It is written above the signature guard, and that is load-bearing.** A pick stamps a
/// signature against the cache it walked and rewinds the count to the sentinel; when the fetch it
/// spawned returns the same content the guard fires, and past it the sentinel would be left with
/// no answer coming — stranding the Shuffle pill as well as the empty state. When the guard fires
/// the model already holds exactly `prepared`, so the count above it is by construction the one
/// the model shows, and `Property::set` is value-compared, so hoisting costs nothing.
///
/// Three things short-circuit it. A hidden section is never written to — the leave teardown
/// emptied this model deliberately. An apply carrying another tab's rows is dropped,
/// [`build_filtered_grid`] materializing only the mounted tab, so a pick landing in the gap would
/// empty the grid it just filled. And an apply that would repaint what is already on screen is
/// dropped, `write_grid` being a `set_vec` reset that tears down every mounted card.
fn write_filtered_grid(ui: &AppWindow, rp_ui: &RecentlyPlayedUi, prepared: PreparedGrid) {
    if !rp_ui.section_active() {
        return;
    }

    let g = ui.global::<RecentlyPlayed>();
    let columns = g.get_columns();
    let tab = tab_from_index(&g, g.get_tab_idx());
    if tab != prepared.tab {
        return;
    }

    // Above the guard — see the doc comment.
    g.set_most_played_count(len_as_i32(prepared.most_played_count));

    let signature = grid_signature(tab, columns, mounted_content(tab, &prepared));
    if rp_ui.state().last_grid_signature.lock().replace(signature) == Some(signature) {
        return;
    }

    // Covers a tab pick as well as a count change, the signature above hashing the tab — so
    // anything that moves what the band says is already past the early return.
    crate::ui::hero_chips::publish_recently_played(ui, rp_ui);

    // On Songs this empties the model rather than leaving it pinning a `SharedString` per field of
    // every card behind a tab the user has left. No branch needed: `build_filtered_grid`
    // materialized rows only for `MostPlayed`, so the Vec is already empty.
    write_grid(
        &g.get_most_played_rows(),
        chunk_entity_rows(prepared.most_played, columns),
        "RecentlyPlayed.most-played-rows",
    );
}

/// Apply from a worker thread, hopping to the event loop to write.
///
/// `warmed_tab` is the tab whose tier [`super::fetch::refresh_grid`] decoded, and it rides in the
/// same closure as the rows so the grid can never mount against a bumped counter and a tier nobody
/// warmed — the case `refresh_grid` hits when the user leaves the section mid-query.
pub(super) fn apply_filtered_grid(
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &Weak<AppWindow>,
    warmed_tab: Option<RecentlyPlayedTab>,
) {
    let prepared = build_filtered_grid(rp_ui);
    let rp_ui = rp_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        write_filtered_grid(&ui, &rp_ui, prepared);
        if should_announce_warm(warmed_tab, rp_ui.section_active(), rp_ui.active_tab()) {
            mark_covers_warm(&ui);
        }
    });
}

/// Apply for a settled keystroke: build on the calling worker, then post.
///
/// **The one apply path that must not run on the UI thread.** The cache it walks is an uncapped,
/// library-wide `get_most_played`, so on the event loop a keystroke folds a needle against every
/// played track, hashes each survivor and builds three `SharedString`s for it, per debounce
/// interval while typing.
///
/// Deferring is safe here in a way it is not for a pick — the tab isn't moving, so the model still
/// holds the previous needle's rows rather than the empty set a leave writes. What it *costs* is
/// ordering: two builds can finish either way round, and [`write_filtered_grid`]'s signature check
/// reads a stale set as a change rather than as staleness. Hence `generation`, checked twice —
/// here before the post is worth making, and again on the UI thread.
pub fn apply_filtered_grid_settled(
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &Weak<AppWindow>,
    generation: u64,
) {
    let prepared = build_filtered_grid(rp_ui);
    if rp_ui.filter_generation() != generation {
        return;
    }
    let rp_ui = rp_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if rp_ui.filter_generation() != generation {
            return;
        }
        let Some(ui) = weak.upgrade() else { return };
        write_filtered_grid(&ui, &rp_ui, prepared);
    });
}

/// Apply from the UI thread, with no event-loop hop — the rows land in the model before Slint
/// re-evaluates the `if` that mounts the entering tab.
///
/// Posting them instead races the redraw, and a redraw that wins paints a bare panel: the hidden
/// tab's model is emptied on every apply, and its `GridEmptyState` is suppressed by a count that
/// is already non-zero.
pub fn apply_filtered_grid_now(ui: &AppWindow, rp_ui: &RecentlyPlayedUi) {
    write_filtered_grid(ui, rp_ui, build_filtered_grid(rp_ui));
}

/// Let the mounted grid's card bindings start decoding on a miss again — see
/// `RecentlyPlayed.covers-generation`.
pub fn mark_covers_warm(ui: &AppWindow) {
    let g = ui.global::<RecentlyPlayed>();
    g.set_covers_generation(g.get_covers_generation().saturating_add(1));
}

/// Re-run the mounted card bindings once a scheduled decode has landed — the
/// `favorites::grids::apply::repaint_covers` contract, never moving off 0.
pub fn repaint_covers(ui: &AppWindow) {
    let g = ui.global::<RecentlyPlayed>();
    let generation = g.get_covers_generation();
    if generation > 0 {
        g.set_covers_generation(generation.saturating_add(1));
    }
}
