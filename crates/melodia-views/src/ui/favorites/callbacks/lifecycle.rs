//! Favorites section lifecycle: the `section-active-changed` enter/leave handler, the
//! `library_changed` subscriber, and the first-enter initial fetch. See [`super::wire`].

use std::sync::Arc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::ui::detail_view::{release_hero_slots, release_shared_hero};
use crate::ui::favorites::NAV_FAVORITES;
use crate::ui::favorites::{self as favorites_ui_mod, FavoritesTab, FavoritesUi};
use crate::ui::list_selection;
use crate::ui::paged_rows::track_rows;
use crate::ui::tab_bar::UNFETCHED_COUNT;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, CardSelection, Favorites, Nav};

/// Wire the Favorites section-lifecycle callbacks.
pub(super) fn wire(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    let weak = ui.as_weak();

    // --- Section-active mirror + cache release / re-enter --------
    // Seed the synchronous shadow from the current nav state — it has to be right on its own, the
    // gate firing only on a later difference (`.claude/rules/ui-patterns.md`'s `SectionActiveGate`
    // bullet). `boot::ui_setup::views::install_views` hydrates the persisted nav index before any
    // `wire_*` runs, so the read below sees it; the sibling `active_tab` shadow is
    // `favorites::seed_tab`'s.
    fav_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_FAVORITES);
    {
        let fu = fav_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            fu.set_section_active(active);
            if !active {
                // Land synchronously on the UI thread *before* the release task is spawned — the
                // re-enter handler reads this through `take_dirty()` and must never observe stale
                // data because release hadn't run yet.
                fu.mark_dirty();
            }
            if !active && let Some(ui) = weak.upgrade() {
                // UI-thread teardown: hand back the banner's cover and blur slots so their
                // `SharedPixelBuffer` Arcs release immediately — the dual blur slots hold their
                // own refs even after the LRU drops — and empty the models so their
                // `SharedString`s go on this tick.
                let g = ui.global::<Favorites>();
                release_hero_slots(&g);
                // Same tick as the wipe above, and unconditional: `release_section_state` bails
                // when the user has already come back, so leaving the guard to it can strand the
                // hero on the bare gradient floor until the next channel tick.
                fu.forget_mosaic();
                // Six heroes share one colour set and one chip row, so hand both back rather than
                // leaving this banner's solve for the next hero to paint under.
                release_shared_hero(&ui);
                // Rewind all three counts on the same tick as the models they number: a count
                // that outlives its model is the one thing these surfaces can state that is
                // *wrong* rather than merely absent. `track-count` is the visible one, the hero
                // square reading it to pick its fallback glyph.
                g.set_track_count(UNFETCHED_COUNT);
                g.set_most_played_count(UNFETCHED_COUNT);
                g.set_artist_count(UNFETCHED_COUNT);
                list_selection::clear_row_selection(&g);
                favorites_ui_mod::release_tracks(&g);
                favorites_ui_mod::release_grids(&ui, &fu);
                // The card grids' set goes back with the models it describes;
                // `card-selection.slint` argues why it cannot outlive the leave.
                ui.global::<CardSelection>().invoke_clear();
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
                // Off-thread heavy release — runs after the UI-thread teardown so the LRU drop
                // pairs with already-released Slint properties and no Arc ref lingers.
                let runtime = s.runtime.clone();
                runtime.spawn_blocking(move || fu.release_section_state());
            }
        });
    }

    // --- library_changed + stats_changed subscriber ---
    // `library_changed` is bumped after every favorite toggle and every scan / file-event commit;
    // `stats_changed` after every play-count flush, which reorders Most Played. Visible, it
    // refetches in place; hidden, it marks dirty so the next enter triggers `kick_full_refresh`.
    // Its own loop because it selects over both channels, and `ui::signal::on_signal` takes one.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        let mut library_rx = state.library_changed.subscribe();
        let mut stats_rx = state.stats_changed.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
            loop {
                // Both senders live in `AppState` for the process lifetime, so an `Err` only
                // happens during teardown — exit the loop.
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
                if !fu.section_active() {
                    fu.mark_dirty();
                    continue;
                }
                kick_full_refresh(&s, &fu, &weak).await;
            }
        }));
    }

    // First section enter: kick an initial fetch so the page paints with data rather than empty
    // models. A session starting elsewhere gets the same fetch from `section-active-changed`
    // through `take_dirty()`; the mark here is what covers a session starting *on* Favorites,
    // whose `else` arm above never fires.
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

/// Fetch the hero stats plus **whichever tab is mounted**, concurrently.
///
/// `refresh_hero` always runs: it answers the count, running time and mosaic the band states on
/// all three tabs, and it is self-contained. The tabs are gated, being mutually exclusive `if`s
/// that only the mounted one draws.
///
/// **The loading tab consumes its flag**, which is what stops a boot onto a tab loading twice;
/// the tabs that skip *set* theirs, so the pick that mounts one knows a tick went by.
async fn kick_full_refresh(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &slint::Weak<AppWindow>,
) {
    let mounted = fav_ui.active_tab();
    for tab in FavoritesTab::ALL {
        if tab == mounted {
            fav_ui.take_tab_dirty(tab);
        } else {
            fav_ui.mark_tab_dirty(tab);
        }
    }
    let hero = favorites_ui_mod::refresh_hero(state, fav_ui, weak, /* animate */ true);

    if mounted != FavoritesTab::Songs {
        let s = state.clone();
        let fu = Arc::clone(fav_ui);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            favorites_ui_mod::reload_grid(&ui, &s, &fu, mounted);
        });
        if let Err(e) = hero.await {
            log::warn!("favorites::refresh_hero: {}", describe(&e));
        }
        return;
    }

    let (h, t) = tokio::join!(hero, track_rows::reload(state, fav_ui));
    if let Err(e) = h {
        log::warn!("favorites::refresh_hero: {}", describe(&e));
    }
    if let Err(e) = t {
        log::warn!("favorites::reload_songs: {}", describe(&e));
    }
}
