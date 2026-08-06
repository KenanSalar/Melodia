//! Recently-Played section lifecycle: the `section-active-changed` enter/leave
//! handler (cache release + re-fetch), the joined `library_changed` +
//! `stats_changed` subscriber, and the first-enter initial fetch. See
//! [`super::wire_recently_played`].

use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Image, SharedString};

use super::NAV_RECENTLY_PLAYED;
use crate::state::AppState;
use crate::ui::callbacks::macros::release_shared_hero;
use crate::ui::model_diff::clear_vec_model;
use crate::ui::recently_played::{self as recently_played_ui_mod, RecentlyPlayedUi};
use crate::ui::tab_bar::UNFETCHED_COUNT;
use crate::{
    AppWindow, EntityGridRow as UiEntityGridRow, Nav, RecentlyPlayed,
    TrackListRow as UiTrackListRow,
};

/// Wire the Recently-Played section-lifecycle callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, rp_ui: &Arc<RecentlyPlayedUi>) {
    let g = ui.global::<RecentlyPlayed>();
    let weak = ui.as_weak();

    // --- Section-active mirror + cache release / re-enter ---------
    // Seed the synchronous shadow from the current nav state: the gate's
    // `ChangeTracker` baselines inside `AppWindow::new()` and fires only on a
    // later difference, so a section the boot doesn't land on gets no edge at
    // all, and the one it does land on gets its edge a frame late — after
    // boot has already read this shadow. See the `SectionActiveGate` bullet
    // in `.claude/rules/ui-patterns.md`.
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
                // Unconditional, on the same tick as the wipe — see the
                // matching call in `favorites/lifecycle.rs`.
                ru.forget_mosaic();
                // Same tick, same reason: the models are emptied below, so a
                // surviving signature would match the identical data on re-enter
                // and skip the refill that fills them back in.
                ru.forget_grid_signature();
                // Six heroes share one colour set and one chip row, so hand
                // both back rather than leaving this mosaic's solve and this
                // view's counts for the next hero to paint under.
                release_shared_hero!(ui);
                // The grid tier goes with `release_section_state` below, so
                // rewind the counter that means "cold" — else the next enter
                // reads a leftover bump as a warm tier and decodes on mount.
                g.set_covers_generation(0);
                // And rewind both counts to "not fetched yet" on the same tick
                // as the models they number, for the reason the folds are reset
                // beside their caches: a count that outlives its model is the
                // one thing these surfaces can state that is *wrong* rather
                // than merely absent. `track-count` is the visible one — the
                // hero square reads it, so a stale non-zero drew four
                // placeholder mosaic slots over an emptied `mosaic-paths` until
                // the re-enter fetch landed.
                g.set_track_count(UNFETCHED_COUNT);
                g.set_most_played_count(UNFETCHED_COUNT);
                clear_vec_model::<UiTrackListRow>(&g.get_tracks(), "recently_played: clear tracks");
                clear_vec_model::<UiEntityGridRow>(
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
    // `stats_changed` after every play-count flush (which writes both this
    // view's ordering keys — `last_played` for Songs, `play_count` for Most
    // Played). So it is the second subscriber to `stats_changed` (Favorites is
    // the first). Visible ⇒ refetch grid + tracks in place; hidden ⇒ mark dirty
    // for the next enter.
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

/// Fetch the Most Played grid + the recency list and apply each as it lands.
/// Concurrent — `tokio::join!` runs both in parallel. `refresh_grid` logs its
/// own error (returns `()`); `refresh_tracks` returns `AppResult<()>`.
async fn kick_full_refresh(
    state: &AppState,
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &slint::Weak<AppWindow>,
) {
    let (_grid, t) = tokio::join!(
        recently_played_ui_mod::refresh_grid(state, rp_ui, weak),
        recently_played_ui_mod::refresh_tracks(state, rp_ui, weak),
    );
    if let Err(e) = t {
        log::warn!("recently_played::refresh_tracks: {e}");
    }
}
