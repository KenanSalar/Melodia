//! The cached entities → the Slint model: the filtered walk, the chunk, and the
//! two ways an apply reaches the UI thread.

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
    /// Which tab these rows were built for. Carried rather than re-derived
    /// because [`build_filtered_grid`] may run on a worker and
    /// [`write_filtered_grid`] always runs on the UI thread, so a pick can land
    /// in the gap — the same shape as `warmed_tab`, and checked the same way.
    tab: RecentlyPlayedTab,
    /// Empty unless [`Self::tab`] is `MostPlayed`.
    pub(super) most_played: Vec<UiEntityStripRow>,
    /// The filtered count, published on **either** tab, unlike the rows. It
    /// gates the `GridEmptyState` and feeds the hero band, so the tab that isn't
    /// mounted still has to publish one — and counting costs nothing extra, the
    /// walk that hashes runs either way.
    pub(super) most_played_count: usize,
    /// Hash of everything that reaches a card, taken from the **source**
    /// entities rather than the built rows — which is what lets the rows above
    /// be built for one tab while the signature stays answerable for both.
    /// `#[derive(Hash)]` keeps it complete when a field is added, where a
    /// hand-listed set would quietly go stale.
    pub(super) most_played_content: u64,
}

/// Re-walk the cached `most_played` Vec through the current
/// `RecentlyPlayed.filter`, hashing and counting the survivors as they go. Runs
/// entirely in memory and touches no Slint state, so either thread can call it.
///
/// **The cache is walked on both tabs; the rows are built only on the mounted
/// one.** The two sub-views are mutually exclusive `if`s, so a row built for the
/// other reaches a grid nothing can scroll and is dropped by
/// [`write_filtered_grid`] anyway. What makes the split possible is that the hash
/// comes off the *source* entities rather than the built rows, so the walk still
/// answers the signature for the tab it didn't build. Which tab that is comes off
/// the [`RecentlyPlayedUi`] shadow, the only form of the answer a worker can read.
pub(super) fn build_filtered_grid(rp_ui: &RecentlyPlayedUi) -> PreparedGrid {
    let needle = rp_ui.state().filter.lock().clone();
    let tab = rp_ui.active_tab();
    let mut hasher = DefaultHasher::new();

    let (most_played, most_played_count) = {
        let cache = rp_ui.state().most_played.lock();
        let matching = cache
            .iter()
            .filter(|t| most_played_matches(t, &needle))
            .inspect(|t| t.hash(&mut hasher));
        if tab == RecentlyPlayedTab::MostPlayed {
            let rows: Vec<UiEntityStripRow> = matching.map(to_slint_most_played_row).collect();
            let count = rows.len();
            (rows, count)
        } else {
            (Vec::new(), matching.count())
        }
    };

    PreparedGrid {
        tab,
        most_played,
        most_played_count,
        most_played_content: hasher.finish(),
    }
}

/// Chunk the prepared rows into cards and push them into the grid's model. UI
/// thread only.
///
/// The count is published unchunked beside the model: `rows.length` is a row
/// count where the hero's band and the empty-state gate want cards. It rides
/// along with the model rather than being written unconditionally, which is safe
/// only because every reader of it is inside the Most Played branch or gated on
/// `tab-idx` — so a count left stale under the skip below is one nothing can
/// render, and picking the tab is itself a signature change that refreshes it.
///
/// Three things short-circuit it. A hidden section is never written to — the
/// leave teardown emptied this model deliberately, and refilling it behind that
/// holds a card row per track for a view nobody can see. An apply carrying
/// another tab's rows is dropped: [`build_filtered_grid`] materializes only the
/// mounted tab, so a pick landing between the build and this write would empty
/// the grid it just filled — and there is nothing to salvage, because that pick
/// ran [`apply_filtered_grid_now`] synchronously against the same cache on its
/// way through. And an apply that would repaint what is already on screen is
/// dropped: `write_grid` is a `set_vec` reset, so it tears down and rebuilds
/// every mounted card, and a `stats_changed` tick reaches both tabs while only
/// this one is ranked by play count.
fn write_filtered_grid(ui: &AppWindow, rp_ui: &RecentlyPlayedUi, prepared: &PreparedGrid) {
    if !rp_ui.section_active() {
        return;
    }

    let g = ui.global::<RecentlyPlayed>();
    let columns = g.get_columns();
    let tab = tab_from_index(&g, g.get_tab_idx());
    if tab != prepared.tab {
        return;
    }

    let signature = grid_signature(tab, columns, mounted_content(tab, prepared));
    if rp_ui.state().last_grid_signature.lock().replace(signature) == Some(signature) {
        return;
    }

    g.set_most_played_count(len_as_i32(prepared.most_played_count));
    // Covers a tab pick as well as a count change, because the signature above
    // hashes the tab — so anything that moves what the band should say has
    // already got past that early return.
    crate::ui::hero_chips::publish_recently_played(ui, rp_ui);

    // The Songs tab empties this rather than leaving it holding its last rows:
    // keeping them would pin one `SharedString` per field of every card behind a
    // tab the user has left. No branch is needed to do it — `build_filtered_grid`
    // materialized rows only for `MostPlayed`, so the other's Vec is already
    // empty and chunks to nothing.
    write_grid(
        &g.get_most_played_rows(),
        chunk_entity_rows(&prepared.most_played, columns),
        "RecentlyPlayed.most-played-rows",
    );
}

/// Apply from a worker thread, hopping to the event loop to write.
///
/// `warmed_tab` is the tab whose tier [`super::fetch::refresh_grid`] decoded, and
/// it rides in the same closure as the rows so the grid can never mount against a
/// bumped counter and a tier nobody warmed — the case `refresh_grid` hits when
/// the user leaves the section while its query is still in flight.
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
        write_filtered_grid(&ui, &rp_ui, &prepared);
        if should_announce_warm(warmed_tab, rp_ui.section_active(), rp_ui.active_tab()) {
            mark_covers_warm(&ui);
        }
    });
}

/// Apply from the UI thread, with no event-loop hop — the rows land in the model
/// before Slint re-evaluates the `if` that mounts the entering tab.
///
/// Posting them instead races the redraw, and a redraw that wins paints a bare
/// panel: the hidden tab's model is emptied on every apply, and its
/// `GridEmptyState` is suppressed by a count that is already non-zero.
pub fn apply_filtered_grid_now(ui: &AppWindow, rp_ui: &RecentlyPlayedUi) {
    write_filtered_grid(ui, rp_ui, &build_filtered_grid(rp_ui));
}

/// Let the mounted grid's card bindings start decoding on a miss again — see
/// `RecentlyPlayed.covers-generation`.
pub fn mark_covers_warm(ui: &AppWindow) {
    let g = ui.global::<RecentlyPlayed>();
    g.set_covers_generation(g.get_covers_generation().saturating_add(1));
}
