//! Recently-Played section lifecycle: the `section-active-changed` enter/leave
//! handler (cache release + re-fetch), the joined `library_changed` +
//! `stats_changed` subscriber, and the first-enter initial fetch. See
//! [`super::wire_recently_played`].

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Image, SharedString};

use super::NAV_RECENTLY_PLAYED;
use crate::state::AppState;
use crate::ui::model_diff::clear_vec_model;
use crate::ui::recently_played::{self as recently_played_ui_mod, RecentlyPlayedUi};
use crate::{
    AppWindow, EntityStripRow as UiEntityStripRow, Nav, RecentlyPlayed,
    TrackListRow as UiTrackListRow,
};

/// Wire the Recently-Played section-lifecycle callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    let weak = ui.as_weak();

    // --- Section-active mirror + cache release / re-enter ---------
    // Seed the synchronous shadow — `changed` in `AppWindow` won't fire for a
    // session that starts on this tab.
    rp_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_RECENTLY_PLAYED);
    {
        let ru = rp_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            ru.set_section_active(active);
            if !active {
                // Land synchronously before the release task is spawned so the
                // re-enter handler's `take_dirty()` never observes stale data.
                ru.mark_dirty();
            }
            if !active && let Some(ui) = weak.upgrade() {
                // UI-thread teardown: clear the hero blur Image slots so their
                // `SharedPixelBuffer` Arcs release immediately, and empty the
                // models so their `SharedString`s drop on the same tick as the
                // LRU release below.
                let g = ui.global::<RecentlyPlayed>();
                g.set_blur_img_a(Image::default());
                g.set_blur_img_b(Image::default());
                g.set_has_blur(false);
                // Six heroes share one colour set, so hand it back to the
                // floor rather than leaving this mosaic's solve for the
                // next hero to paint under.
                crate::ui::hero_backdrop::reset(&ui);
                clear_vec_model::<UiTrackListRow>(&g.get_tracks(), "recently_played: clear tracks");
                clear_vec_model::<UiEntityStripRow>(
                    &g.get_most_played_rows(),
                    "recently_played: clear most-played",
                );
                clear_vec_model::<SharedString>(
                    &g.get_mosaic_paths(),
                    "recently_played: clear mosaic-paths",
                );
                clear_vec_model::<i32>(
                    &g.get_selected_ids(),
                    "recently_played: clear selected-ids",
                );
                g.set_selection_anchor(-1);
            }
            let ru = ru.clone();
            let s = s.clone();
            let weak = weak.clone();
            if active {
                let runtime = s.runtime.clone();
                runtime.spawn(async move {
                    if ru.take_dirty() {
                        kick_full_refresh(&s, &ru, &weak).await;
                    }
                });
            } else {
                let runtime = s.runtime.clone();
                runtime.spawn_blocking(move || ru.release_section_state());
            }
        });
    }

    // --- library_changed_tx + stats_changed_tx subscriber ---------
    // `library_changed` is bumped by scans / imports / favorite toggles;
    // `stats_changed` after every play-count flush (which also writes
    // `last_played`, this view's ordering key). So it is the second
    // subscriber to `stats_changed` (Favorites is the first). Visible ⇒
    // refetch strips + tracks in place; hidden ⇒ mark dirty for the next
    // enter.
    {
        let s = state.clone();
        let ru = rp_ui.clone();
        let weak = weak.clone();
        let mut library_rx = state.library_changed_tx.subscribe();
        let mut stats_rx = state.stats_changed_tx.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
            library_rx.mark_unchanged();
            stats_rx.mark_unchanged();
            loop {
                // Both senders live in `AppState` for the process lifetime, so
                // an `Err` only happens during teardown — exit the loop.
                tokio::select! {
                    changed = library_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                    changed = stats_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                }
                if !ru.section_active() {
                    ru.mark_dirty();
                    continue;
                }
                kick_full_refresh(&s, &ru, &weak).await;
            }
        }));
    }

    // First section enter: kick an initial fetch so the page paints with data.
    // If the session starts elsewhere, `section-active-changed` triggers the
    // same fetch via `take_dirty()`.
    rp_ui.mark_dirty();
    if rp_ui.section_active() {
        let s = state.clone();
        let ru = rp_ui.clone();
        let weak = weak.clone();
        state.runtime.clone().spawn(async move {
            if ru.take_dirty() {
                kick_full_refresh(&s, &ru, &weak).await;
            }
        });
    }
}

/// Fetch the Most Played strip + the recency list and apply each as it lands.
/// Concurrent — `tokio::join!` runs both in parallel. `refresh_strips` logs its
/// own error (returns `()`); `refresh_tracks` returns `AppResult<()>`.
async fn kick_full_refresh(
    state: &AppState,
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &slint::Weak<AppWindow>,
) {
    let (_strips, t) = tokio::join!(
        recently_played_ui_mod::refresh_strips(state, rp_ui, weak),
        recently_played_ui_mod::refresh_tracks(state, rp_ui, weak),
    );
    if let Err(e) = t {
        log::warn!("recently_played::refresh_tracks: {e}");
    }
}
