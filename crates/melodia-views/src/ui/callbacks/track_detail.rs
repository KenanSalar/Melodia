//! The callbacks the four track-list details share, wired once over [`DetailGlobal`] and
//! [`TrackDetail`]. Each view's own `callbacks/detail.rs` keeps close-detail, whose routing
//! and release policy differ per view, and whatever only that view has.
//!
//! Every closure holds the global's own weak handle, `slint::Global::as_weak`'s `'static` one,
//! so none of them needs the window.

use std::sync::Arc;

use slint::Weak;

use super::macros::{spawn_logged, wire_row_flag};
use super::{
    collect_track_ids, next_sort_with_natural, persist_view_sort, play_row_start,
    spawn_play_then_shuffle,
};
use crate::ui::track_detail::{DetailGlobal, DetailRows, TrackDetail, selection};
use crate::ui::track_list_view::persist_visible;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::AppWindow;

/// Wires the shared callbacks of `view`'s detail global. A no-op if the window is gone.
pub(in crate::ui) fn wire<V: TrackDetail>(weak: &Weak<V::Global>, state: &AppState, view: &Arc<V>) {
    let Some(g) = weak.upgrade() else { return };
    wire_playback(&g, state, view);
    wire_row_flags(&g, state, weak, view);
    wire_selection(&g, weak, view);
    wire_sort_and_columns(&g, state, weak, view);
    wire_filter(&g, weak, view);
}

/// What every close owes once the view has routed itself: the detail id back to `-1`, the needle
/// cleared on both sides, the Rust state dropped, the persisted id forgotten so the next launch
/// lands on the grid, and the post-close history entry.
///
/// Call it **before** spawning the view's release: a grid prewarm in that spawn reads the id. The
/// hero slots aren't touched here, the band morphing out over them, and
/// `MyLibrary.hero-collapsed` hands them back.
pub(in crate::ui) fn forget_closed<V: TrackDetail>(
    ui: &AppWindow,
    state: &AppState,
    view: &V,
    g: &impl DetailGlobal,
) {
    g.set_detail_id(-1);
    g.clear_filter();
    view.clear_detail();

    let s = state.clone();
    state.runtime.spawn_blocking(move || {
        if let Err(e) = library::settings::set_last_detail_id(&s.paths, V::VIEW_ID, None) {
            log::warn!("{}::close_detail persist: {}", V::VIEW_ID, describe(&e));
        }
    });

    // After any origin restore the caller ran, so the entry names the section it landed on. A
    // no-op while a Mouse-4/5 replay drives this callback itself.
    crate::ui::nav_history::record_current(ui);
}

/// Play-row, shuffle, play-next and add-to-queue. A row activation replaces the queue with the
/// displayed rows; the menu's two entries append the ids they are handed.
fn wire_playback<V: TrackDetail>(g: &V::Global, state: &AppState, view: &Arc<V>) {
    {
        let s = state.clone();
        let view = view.clone();
        g.bind_shuffle(move || {
            spawn_play_then_shuffle(&s, V::VIEW_ID, view.cache().track_ids());
        });
    }
    {
        let s = state.clone();
        let view = view.clone();
        g.bind_play_row(move |track_id, idx| {
            let ids = view.cache().track_ids();
            if ids.is_empty() {
                return;
            }
            let start = play_row_start(&ids, i64::from(track_id), idx);
            let s = s.clone();
            spawn_logged!(
                s,
                format_args!("{}::play_row", V::VIEW_ID),
                library::playback::player_play_tracks(&s.playback_ctx(), ids, start)
            );
        });
    }
    {
        let s = state.clone();
        g.bind_play_next(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(
                s,
                format_args!("{}::play_next", V::VIEW_ID),
                library::queue::queue_play_next_many(&s, id_vec)
            );
        });
    }
    {
        let s = state.clone();
        g.bind_add_to_queue(move |ids| {
            let id_vec = collect_track_ids(&ids);
            let s = s.clone();
            spawn_logged!(
                s,
                format_args!("{}::add_to_queue", V::VIEW_ID),
                library::queue::queue_add_tracks(&s, id_vec)
            );
        });
    }
}

/// Heart and stars write through, then patch the rows they touched.
fn wire_row_flags<V: TrackDetail>(
    g: &V::Global,
    state: &AppState,
    weak: &Weak<V::Global>,
    view: &Arc<V>,
) {
    let rows = DetailRows::new(view, weak.clone());
    wire_row_flag!(g, bind_toggle_row_favorite, state,
    format_args!("{}::set_favorite", V::VIEW_ID),
    library::favorites::set_favorite, collect_track_ids,
    captures: [rows],
    after: |id_vec, fav| {
        for id in &id_vec {
            rows.mirror_favorite(*id, fav);
        }
    });
    wire_row_flag!(g, bind_set_row_rating, state,
    format_args!("{}::set_rating", V::VIEW_ID),
    library::ratings::set_rating, collect_track_ids,
    captures: [rows],
    after: |id_vec, rating| {
        for id in &id_vec {
            rows.mirror_rating(*id, rating);
        }
    });
}

fn wire_selection<V: TrackDetail>(g: &V::Global, weak: &Weak<V::Global>, view: &Arc<V>) {
    {
        let weak = weak.clone();
        let view = view.clone();
        g.bind_select_row(move |idx, id, shift, ctrl| {
            let Some(g) = weak.upgrade() else { return };
            selection::handle_select_row(&g, view.cache(), idx, id, shift, ctrl);
        });
    }
    {
        let weak = weak.clone();
        let view = view.clone();
        g.bind_select_all(move || {
            let Some(g) = weak.upgrade() else { return };
            selection::select_all(&g, view.cache());
        });
    }
    {
        let weak = weak.clone();
        let view = view.clone();
        g.bind_clear_selection(move || {
            let Some(g) = weak.upgrade() else { return };
            selection::clear_selection(&g, view.cache());
        });
    }
}

/// A header click re-sorts in memory and persists the pick, one sort shared by every entity of
/// the view. The column popup has already flipped its `show-*` flag, so a toggle only persists.
fn wire_sort_and_columns<V: TrackDetail>(
    g: &V::Global,
    state: &AppState,
    weak: &Weak<V::Global>,
    view: &Arc<V>,
) {
    {
        let s = state.clone();
        let weak = weak.clone();
        let view = view.clone();
        g.bind_request_sort(move |clicked| {
            let Some(g) = weak.upgrade() else { return };
            let (field, dir) = g.sort();
            let (new_field, new_dir) =
                next_sort_with_natural(&field, &dir, &clicked, V::NATURAL_SORT);
            g.set_sort(&new_field, new_dir.as_str());
            view.resort(&g);
            persist_view_sort(&s, V::VIEW_ID, new_field, new_dir);
        });
    }
    {
        let s = state.clone();
        let weak = weak.clone();
        g.bind_toggle_column(move |_id| {
            let Some(g) = weak.upgrade() else { return };
            persist_visible(&s, &g);
        });
    }
}

/// A settled keystroke re-walks the cached rows: an in-memory filter, no database round trip.
fn wire_filter<V: TrackDetail>(g: &V::Global, weak: &Weak<V::Global>, view: &Arc<V>) {
    let weak = weak.clone();
    let view = view.clone();
    g.bind_filter_changed(move |text| {
        let Some(g) = weak.upgrade() else { return };
        view.cache().set_filter(&text);
        view.refilter(&g);
    });
}
