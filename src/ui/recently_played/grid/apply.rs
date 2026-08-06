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
    /// The filtered count that gates the grid's `GridEmptyState`. `0` on the
    /// Songs tab, where nothing reads it: both readers of
    /// `RecentlyPlayed.most-played-count` sit inside the Most Played branch, and
    /// the band takes its facts from `RecentlyPlayedUiState::most_played_totals`
    /// rather than from here.
    pub(super) most_played_count: usize,
    /// Hash of everything that reaches a card, taken from the **source**
    /// entities rather than the built rows. `#[derive(Hash)]` keeps it complete
    /// when a field is added, where a hand-listed set would quietly go stale.
    /// `0` on the Songs tab, matching what [`mounted_content`] answers there.
    pub(super) most_played_content: u64,
}

/// Re-walk the cached `most_played` Vec through the current
/// `RecentlyPlayed.filter`, hashing and counting the survivors as they go. Runs
/// entirely in memory and touches no Slint state, so either thread can call it.
///
/// **Nothing is walked on the Songs tab**, which is the same mounted-tab-only
/// rule `songs::build_filtered_tracks` holds from the other side. The two
/// sub-views are mutually exclusive `if`s, so
/// neither the rows nor the count reaches anything there — and the walk is not
/// free: the query behind this cache is uncapped and library-wide, and
/// [`apply_filtered_grid_now`] calls this **on the UI thread** from
/// `on_filter_changed` and `on_columns_changed`, so a settled keystroke was
/// folding a needle against every played track and pushing each one's six
/// strings through a hasher for a grid nobody can see. The bail is safe because
/// [`mounted_content`] already collapses the Songs arm to a constant `0`: the
/// signature never depended on this hash there, and the tab pick itself runs
/// `apply_filtered_grid_now` on the way in, so the count is rebuilt before the
/// grid it gates can mount.
///
/// Which tab is mounted comes off the [`RecentlyPlayedUi`] shadow, the only form
/// of the answer a worker can read.
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
    let most_played: Vec<UiEntityStripRow> = rp_ui
        .state()
        .most_played
        .lock()
        .iter()
        .filter(|t| most_played_matches(t, &needle))
        .inspect(|t| t.hash(&mut hasher))
        .map(to_slint_most_played_row)
        .collect();

    let most_played_count = most_played.len();
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
/// count where the `GridEmptyState` wants cards. It is written on either tab, but
/// only stands for something on Most Played — [`build_filtered_grid`] walks
/// nothing on Songs, so there it is a constant `0`. That is safe for the same
/// reason the skip below is: every reader of it sits inside the Most Played branch
/// or under a `tab-idx` gate, so a count nothing walked is one nothing can render,
/// and picking that tab is itself a signature change that recomputes it. **What
/// the pick then owes is the sentinel**, when the cache it recomputed against is
/// one a skipped tick left empty — `callbacks::recently_played::subviews` argues it.
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
