//! Recently-Played section lifecycle: the `section-active-changed` enter/leave
//! handler (cache release + re-fetch), the joined `library_changed` +
//! `stats_changed` subscriber, and the first-enter initial fetch. See
//! [`super::wire`].

use std::sync::Arc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::ui::detail_view::{release_hero_slots, release_shared_hero};
use crate::ui::list_selection;
use crate::ui::model_diff::clear_vec_model;
use crate::ui::recently_played::NAV_RECENTLY_PLAYED;
use crate::ui::recently_played::{
    self as recently_played_ui_mod, RecentlyPlayedTab, RecentlyPlayedUi,
};
use crate::ui::tab_bar::UNFETCHED_COUNT;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, CardSelection, Nav, RecentlyPlayed, TrackListRow as UiTrackListRow};

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
                // UI-thread teardown: hand back the banner's cover and blur slots so their
                // `SharedPixelBuffer` Arcs release immediately, and empty the
                // models so their `SharedString`s drop on the same tick as the
                // LRU release below.
                let g = ui.global::<RecentlyPlayed>();
                release_hero_slots(&g);
                // Unconditional, on the same tick as the wipe — see the
                // matching call in `favorites/lifecycle.rs`.
                ru.forget_mosaic();
                // Six heroes share one colour set and one chip row, so hand
                // both back rather than leaving this banner's solve and this
                // view's counts for the next hero to paint under.
                release_shared_hero(&ui);
                // Rewind both counts to "not fetched yet" on the same tick
                // as the models they number, for the reason the folds are reset
                // beside their caches: a count that outlives its model is the
                // one thing these surfaces can state that is *wrong* rather
                // than merely absent. `track-count` is the visible one — the
                // hero square reads it to pick its fallback glyph.
                g.set_track_count(UNFETCHED_COUNT);
                g.set_most_played_count(UNFETCHED_COUNT);
                clear_vec_model::<UiTrackListRow>(&g.get_tracks(), "recently_played: clear tracks");
                recently_played_ui_mod::release_grid(&ui, &ru);
                list_selection::clear_row_selection(&g);
                // The card grid's set goes back with the models it describes;
                // `card-selection.slint` argues why it cannot outlive the leave.
                ui.global::<CardSelection>().invoke_clear();
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

    // --- library_changed + stats_changed subscriber ---------
    // `library_changed` is bumped by scans / imports / favorite toggles;
    // `stats_changed` after every play-count flush (which writes both this
    // view's ordering keys — `last_played` for Songs, `play_count` for Most
    // Played), so it listens to `stats_changed` as Favorites and Playlists
    // do. Visible ⇒ refetch grid + tracks in place; hidden ⇒ mark dirty
    // for the next enter. Its own loop for Favorites' reason: it selects over
    // both channels, and `ui::signal::on_signal` takes one.
    {
        let s = state.clone();
        let ru = rp_ui.clone();
        let weak = weak.clone();
        let mut library_rx = state.library_changed.subscribe();
        let mut stats_rx = state.stats_changed.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
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

/// Fetch the recency list, and reload the Most Played grid only when its tab is the one
/// mounted.
///
/// `refresh_tracks` always runs: it answers the count, running time and collage the band states
/// on both tabs. The grid reloads on every `stats_changed` tick, i.e. once per finished track, for
/// as long as it is on screen; on the Songs tab that is a grid nobody can see. My Library answers
/// this with a `SectionActiveGate` per tab; this page has one gate for both tabs, so the tick that
/// was skipped is remembered on [`RecentlyPlayedUi::mark_grid_dirty`] and the pick that mounts the
/// grid reloads instead.
async fn kick_full_refresh(
    state: &AppState,
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &slint::Weak<AppWindow>,
) {
    if rp_ui.active_tab() == RecentlyPlayedTab::MostPlayed {
        // This *is* the load the flag schedules, so it settles it here rather than
        // leaving the next pick to reload a grid this tick just filled. Seeded
        // `true`, so without it a boot onto this tab pays for its own load twice.
        rp_ui.take_grid_dirty();
        let s = state.clone();
        let ru = Arc::clone(rp_ui);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            recently_played_ui_mod::reload_grid(&ui, &s, &ru);
        });
    } else {
        rp_ui.mark_grid_dirty();
    }
    if let Err(e) = recently_played_ui_mod::refresh_tracks(state, rp_ui, weak).await {
        log::warn!("recently_played::refresh_tracks: {}", describe(&e));
    }
}
