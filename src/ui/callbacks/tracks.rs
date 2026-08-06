//! `Tracks.*` callbacks: sort, filter, play, favorite, selection,
//! column-visibility persistence.

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use super::{collect_track_ids, next_sort};
use super::macros::{spawn_blocking_logged, spawn_logged, wire_row_flag};
use crate::library;
use crate::state::AppState;
use crate::ui::my_library::{MyLibraryTab, tab_is_mounted};
use crate::ui::track_list_view::{TrackListColumnState, view_id};
use crate::ui::tracks::{self as tracks_ui_mod, TracksUi};
use crate::{AppWindow, Tracks};

/// Wire every `Tracks.*` callback to its `library::*` counterpart and the
/// `tracks_ui` shared state. Call once after `wire_all` and after
/// `tracks::install_tracks_model`.
pub fn wire_tracks(ui: &AppWindow, state: &AppState, tracks_ui: &Arc<TracksUi>) {
    let tracks = ui.global::<Tracks>();
    let weak = ui.as_weak();

    // Seed the sort header from the persisted `view_sort["tracks"]` so the
    // arrow matches the order `spawn_initial_tracks_fetch` fetches with.
    if let Some((field, dir)) = super::persisted_sort(state, view_id::TRACKS) {
        tracks.set_sort_field(SharedString::from(field.as_str()));
        tracks.set_sort_dir(SharedString::from(dir));
    }

    // section-active-changed: mirror visibility into the synchronous shadow,
    // and on re-enter run the deferred refresh. **A tab leave is one of those
    // leaves now** — the Songs tab has its own `SectionActiveGate` — so seed the
    // shadow from the mounted tab, not from the nav index: the gate's
    // `ChangeTracker` baselines inside `AppWindow::new()` and fires only on a
    // later difference, so a view the boot doesn't land on gets no edge at all,
    // and the one it does land on gets its edge a frame late — after boot has
    // already read this shadow. See the `SectionActiveGate` bullet in
    // `.claude/rules/ui-patterns.md`.
    tracks_ui.set_section_active(tab_is_mounted(ui, MyLibraryTab::Songs));
    {
        let tu = tracks_ui.clone();
        let weak = weak.clone();
        tracks.on_section_active_changed(move |active| {
            tu.set_section_active(active);
            // **The leave does nothing else, and Tracks is the one library view
            // that can say that.** Its four siblings empty their models on the
            // way out, so each owes the `UNFETCHED_COUNT` rewind that stops a
            // count outliving the rows it numbers — and, having rewound, owes
            // the `mark_dirty` that answers it. This model *survives* the leave,
            // so the count is never stale over an empty list and there is
            // nothing to answer. Marking dirty anyway made a tab pick a full
            // `get_tracks` plus a library-sized row build on the event loop,
            // every time the user came back to Songs. Freshness is unaffected:
            // `boot::ui_setup::install_library_changed_refresher` already folds
            // every bump arriving while this tab is unmounted into the same
            // flag.
            if !active {
                return;
            }
            if tu.take_dirty() {
                let Some(ui) = weak.upgrade() else { return };
                // Reuse the request-refresh path so the deferred fetch
                // picks up the current sort + filter.
                ui.global::<Tracks>().invoke_request_refresh();
            }
        });
    }

    // request-sort: clicking a header column. Same field flips dir; new field
    // resets to ascending. Re-sort is done entirely in memory — no DB
    // re-fetch and no `RowSearchKey` rebuild (`resort_and_apply`); the new
    // sort is persisted so it survives a restart.
    {
        let s = state.clone();
        let tu = tracks_ui.clone();
        let weak = weak.clone();
        tracks.on_request_sort(move |field| {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Tracks>();
            let (new_field, new_dir) =
                next_sort(g.get_sort_field().as_str(), g.get_sort_dir().as_str(), &field);
            g.set_sort_field(SharedString::from(new_field.as_str()));
            g.set_sort_dir(SharedString::from(new_dir.as_str()));
            let filter = g.get_filter().to_string();

            tracks_ui_mod::resort_and_apply(&weak, &tu, &new_field, new_dir.as_str(), filter);
            super::persist_view_sort(&s, view_id::TRACKS, new_field, new_dir);
        });
    }

    // apply-filter: client-side; no DB hit.
    {
        let tu = tracks_ui.clone();
        let weak = weak.clone();
        tracks.on_apply_filter(move |text| {
            tracks_ui_mod::refilter(&weak, &tu, text.to_string());
        });
    }

    // play-row: double-click loads the current view into the queue and starts
    // on the clicked track — the standard music-player contract.
    {
        let s = state.clone();
        let tu = tracks_ui.clone();
        let weak = weak.clone();
        tracks.on_play_row(move |track_id, idx| {
            let Some(ui) = weak.upgrade() else { return };
            let filter = ui.global::<Tracks>().get_filter().to_string();
            let ids = tu.current_ids_filtered(&filter);
            if ids.is_empty() {
                return;
            }
            let start = super::play_row_start(&ids, i64::from(track_id), idx);
            let s = s.clone();
            spawn_logged!(s, "tracks::play_row",
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start));
        });
    }

    {
        let s = state.clone();
        tracks.on_play_next(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(s, "play_next",
                library::queue::queue_play_next_many(&s, id_vec));
        });
    }

    {
        let s = state.clone();
        tracks.on_add_to_queue(move |ids| {
            let id_vec: Vec<i64> = ids.iter().map(i64::from).collect();
            let s = s.clone();
            spawn_logged!(s, "add_to_queue", library::queue::queue_add_tracks(&s, id_vec));
        });
    }

    // toggle-row-favorite / set-row-rating: write through, then surgically
    // update each affected row (no list re-fetch, so scroll position holds
    // and there's no flash). Rating never changes list membership.
    {
        let tu = tracks_ui.clone();
        wire_row_flag!(tracks, on_toggle_row_favorite, state, "set_favorite",
            library::favorites::set_favorite, collect_track_ids,
            captures: [weak, tu],
            after: |id_vec, fav| {
                for id in &id_vec {
                    tu.flip_favorite(*id, fav);
                    tracks_ui_mod::apply_row_favorite(&weak, *id, fav);
                }
            });
    }
    {
        let tu = tracks_ui.clone();
        wire_row_flag!(tracks, on_set_row_rating, state, "set_rating",
            library::ratings::set_rating, collect_track_ids,
            captures: [weak, tu],
            after: |id_vec, rating| {
                for id in &id_vec {
                    tu.flip_rating(*id, rating);
                    tracks_ui_mod::apply_row_rating(&weak, *id, rating);
                }
            });
    }

    // select-row: modifier-aware click handler. Computes the new selected
    // set in Rust (Slint expressions can't iterate a model for a membership
    // check) and walks the visible VecModel to flip per-row `selected` flags.
    {
        let weak = weak.clone();
        let tu = tracks_ui.clone();
        tracks.on_select_row(move |idx, id, shift, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            tracks_ui_mod::handle_select_row(&ui, &tu, idx, id, shift, ctrl);
        });
    }

    {
        let weak = weak.clone();
        tracks.on_clear_selection(move || {
            let Some(ui) = weak.upgrade() else { return };
            tracks_ui_mod::clear_selection(&ui);
        });
    }

    // request-refresh: re-fetch with current sort + filter (e.g. after a scan
    // completes — wired up later when scan-complete events surface).
    {
        let s = state.clone();
        let tu = tracks_ui.clone();
        let weak = weak.clone();
        tracks.on_request_refresh(move || {
            let Some(ui) = weak.upgrade() else { return };
            let g = ui.global::<Tracks>();
            let field = g.get_sort_field().to_string();
            let dir = g.get_sort_dir().to_string();
            let filter = g.get_filter().to_string();
            let s = s.clone();
            let tu = tu.clone();
            let weak = weak.clone();
            spawn_logged!(s, "tracks::refresh",
                tracks_ui_mod::fetch_and_apply(&s, &tu, weak, field, dir, filter));
        });
    }

    // toggle-column: the popup has already flipped the matching `show-*`
    // flag for instant visual feedback. We persist the new visible-column
    // list to `views.json`'s `view_columns["tracks"]` so the choice survives
    // restarts. The Slint flag is the source of truth here — we read it
    // through `TrackListColumnState::snapshot_visible` so the on-disk shape
    // stays consistent with the shutdown-snapshot path.
    {
        let s = state.clone();
        let weak = weak.clone();
        tracks.on_toggle_column(move |_id| {
            let Some(ui) = weak.upgrade() else { return };
            let columns = ui.global::<Tracks>().snapshot_visible();
            let s = s.clone();
            spawn_blocking_logged!(s, "tracks::toggle_column",
                library::settings::update_view_columns(&s, "tracks".to_string(), columns));
        });
    }
}
