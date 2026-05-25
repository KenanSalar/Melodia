//! Favorites section lifecycle: the `section-active-changed` enter/leave
//! handler (cache release + re-fetch), the `library_changed` subscriber,
//! and the first-enter initial fetch. See [`super::wire_favorites`].

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Image, Model, SharedString, VecModel};

use super::NAV_FAVORITES;
use crate::state::AppState;
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesUi};
use crate::{
    AppWindow, EntityStripRow as UiEntityStripRow, Favorites, Nav,
    TrackListRow as UiTrackListRow,
};

/// Wire the Favorites section-lifecycle callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    let weak = ui.as_weak();

    // --- Section-active mirror + cache release / re-enter --------
    // Seed the synchronous shadow from the current nav state — `changed`
    // in `AppWindow` won't fire for a session that starts on Favorites.
    fav_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_FAVORITES);
    {
        let fu = fav_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            fu.set_section_active(active);
            if !active {
                // Land synchronously on the UI thread *before* the
                // release task is even spawned — the re-enter handler
                // reads this via `take_dirty()` and must never observe
                // stale data because release hadn't run yet. Mirrors
                // `AlbumsUi::data_dirty`.
                fu.mark_dirty();
            }
            if !active && let Some(ui) = weak.upgrade() {
                // UI-thread teardown: clear Slint Image properties so
                // the `SharedPixelBuffer` Arcs the LRU is about to
                // clear release immediately (the dual-slot blur slots
                // hold their own refs even after the LRU drops). Empty
                // the strip + tracks models so their `SharedString`s
                // also drop on the same tick.
                let g = ui.global::<Favorites>();
                g.set_blur_img_a(Image::default());
                g.set_blur_img_b(Image::default());
                g.set_has_blur(false);
                clear_model::<UiTrackListRow>(&g.get_tracks(), "tracks");
                clear_model::<UiEntityStripRow>(&g.get_most_played_rows(), "most-played");
                clear_model::<UiEntityStripRow>(&g.get_artist_rows(), "artist");
                clear_model_i32(&g.get_selected_ids());
                clear_model_string(&g.get_mosaic_paths());
                g.set_selection_anchor(-1);
            }
            let fu = fu.clone();
            let s = s.clone();
            let weak = weak.clone();
            if active {
                let runtime = s.runtime.clone();
                runtime.spawn(async move {
                    if fu.take_dirty() {
                        kick_full_refresh(&s, &fu, &weak).await;
                    }
                });
            } else {
                // Off-thread heavy release — runs after UI-thread
                // teardown so the LRU drop pairs with already-released
                // Slint properties (no lingering Arc refs).
                let runtime = s.runtime.clone();
                runtime.spawn_blocking(move || fu.release_section_state());
            }
        });
    }

    // --- library_changed_tx subscriber (Phase 9) ------------------
    // Bumped after every `set_favorite` / `toggle_current_favorite`
    // (Phase 1.2) + every scan / file-event commit. While the
    // Favorites tab is visible we refetch hero + strips + tracks
    // in-place; while hidden we just mark dirty so the next enter
    // triggers `kick_full_refresh`.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        let mut rx = state.library_changed_tx.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
            rx.mark_unchanged();
            while rx.changed().await.is_ok() {
                if !fu.section_active() {
                    fu.mark_dirty();
                    continue;
                }
                kick_full_refresh(&s, &fu, &weak).await;
            }
        }));
    }

    // First section enter: kick an initial fetch so the page paints
    // with data instead of empty models when the user lands here. If
    // the session starts on a different tab, `section-active-changed`
    // will trigger the same fetch via `take_dirty()` (mark_dirty in
    // the `else` arm above would never fire on a session that *never*
    // visited Favorites). For the start-on-Favorites case, mark dirty
    // synchronously here so the subsequent `section-active-changed`
    // fire (or the next library_changed tick) re-fetches.
    fav_ui.mark_dirty();
    if fav_ui.section_active() {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        state.runtime.clone().spawn(async move {
            if fu.take_dirty() {
                kick_full_refresh(&s, &fu, &weak).await;
            }
        });
    }
}

/// Fetch hero stats + the strips + the All Songs list and apply each
/// as it lands. Concurrent — `tokio::join!` runs all three in
/// parallel. `refresh_strips` already logs its own per-section
/// errors (because Most Played + Artists are applied independently),
/// so it returns `()`; the other two return `AppResult<()>` and have
/// their errors logged here.
async fn kick_full_refresh(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &slint::Weak<AppWindow>,
) {
    let (h, _strips, t) = tokio::join!(
        favorites_ui_mod::refresh_hero(state, fav_ui, weak, /* animate */ true),
        favorites_ui_mod::refresh_strips(state, fav_ui, weak),
        favorites_ui_mod::refresh_tracks(state, fav_ui, weak),
    );
    if let Err(e) = h {
        log::warn!("favorites::refresh_hero: {e}");
    }
    if let Err(e) = t {
        log::warn!("favorites::refresh_tracks: {e}");
    }
}

fn clear_model<T: Clone + Default + 'static>(model: &slint::ModelRc<T>, label: &str) {
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<T>>() {
        vm.set_vec(Vec::new());
    } else {
        log::warn!("favorites: clear {label}: VecModel downcast failed");
    }
}

fn clear_model_i32(model: &slint::ModelRc<i32>) {
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<i32>>() {
        vm.set_vec(Vec::new());
    }
}

fn clear_model_string(model: &slint::ModelRc<SharedString>) {
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<SharedString>>() {
        vm.set_vec(Vec::new());
    }
}
