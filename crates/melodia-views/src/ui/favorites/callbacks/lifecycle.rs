//! The Favorites section's enter and leave, its refresh subscriber and its first fetch.

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

pub(super) fn wire(ui: &AppWindow, state: &AppState, fav_ui: &Arc<FavoritesUi>) {
    let g = ui.global::<Favorites>();
    let weak = ui.as_weak();

    // Seeded here, the gate firing only on a later difference (`ui-patterns.md`'s
    // `SectionActiveGate` bullet); `install_views` hydrates the nav index before any `wire_*` runs.
    fav_ui.set_section_active(ui.global::<Nav>().get_selected_index() == NAV_FAVORITES);
    {
        let fu = fav_ui.clone();
        let s = state.clone();
        let weak = weak.clone();
        g.on_section_active_changed(move |active| {
            fu.set_section_active(active);
            if !active {
                // Before the release is spawned, so a re-enter's `take_dirty()` can't miss it.
                fu.mark_dirty();
            }
            if !active && let Some(ui) = weak.upgrade() {
                // The banner's slots hold their buffers past the LRU, so they go back here.
                let g = ui.global::<Favorites>();
                release_hero_slots(&g);
                fu.forget_mosaic();
                release_shared_hero(&ui);
                g.set_track_count(UNFETCHED_COUNT);
                g.set_most_played_count(UNFETCHED_COUNT);
                g.set_artist_count(UNFETCHED_COUNT);
                list_selection::clear_row_selection(&g);
                favorites_ui_mod::release_tracks(&g);
                favorites_ui_mod::release_grids(&ui, &fu);
                // The card selection goes back with the models (`card-selection.slint`).
                ui.global::<CardSelection>().invoke_clear();
            }
            let fu = fu.clone();
            let s = s.clone();
            let weak = weak.clone();
            if active {
                let runtime = s.runtime.clone();
                runtime.spawn(async move {
                    if fu.take_dirty() {
                        kick_full_refresh(&s, &fu, &weak, Stale::Everything).await;
                    }
                });
            } else {
                // After the teardown above, so no Slint property still holds what the LRU drops.
                let runtime = s.runtime.clone();
                runtime.spawn_blocking(move || fu.release_section_state());
            }
        });
    }

    // `stats_changed` follows a play-count flush, which reorders Most Played. Its own loop because
    // it selects over both channels, where `ui::signal::on_signal` takes one.
    {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        let mut library_rx = state.library_changed.subscribe();
        let mut stats_rx = state.stats_changed.subscribe();
        let _ = slint::spawn_local(Compat::new(async move {
            loop {
                let stale = tokio::select! {
                    changed = library_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        Stale::Everything
                    }
                    changed = stats_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        Stale::PlayStats
                    }
                };
                if !fu.section_active() {
                    fu.mark_dirty();
                    continue;
                }
                kick_full_refresh(&s, &fu, &weak, stale).await;
            }
        }));
    }

    // A boot onto Favorites gets no edge, so it fetches here; one elsewhere fetches on its first
    // enter.
    fav_ui.mark_dirty();
    if fav_ui.section_active() {
        let s = state.clone();
        let fu = fav_ui.clone();
        let weak = weak.clone();
        state.runtime.clone().spawn(async move {
            if fu.take_dirty() {
                kick_full_refresh(&s, &fu, &weak, Stale::Everything).await;
            }
        });
    }
}

/// What a refresh is for: an enter or a `library_changed` tick, or a play-count flush.
#[derive(Clone, Copy)]
enum Stale {
    Everything,
    PlayStats,
}

impl Stale {
    /// Whether `tab` can show something different. No Songs column or sort reads a play count,
    /// and a flush lands about once a track.
    fn reaches(self, tab: FavoritesTab) -> bool {
        matches!(self, Self::Everything) || tab != FavoritesTab::Songs
    }
}

/// Refreshes the band, which every tab states, and the mounted tab where `stale` reaches it. The
/// loading tab consumes its flag, so a boot onto it loads once; the others set theirs for the pick
/// that mounts them.
async fn kick_full_refresh(
    state: &AppState,
    fav_ui: &Arc<FavoritesUi>,
    weak: &slint::Weak<AppWindow>,
    stale: Stale,
) {
    let mounted = fav_ui.active_tab();
    for tab in FavoritesTab::ALL.into_iter().filter(|&tab| stale.reaches(tab)) {
        if tab == mounted {
            fav_ui.take_tab_dirty(tab);
        } else {
            fav_ui.mark_tab_dirty(tab);
        }
    }
    let hero = favorites_ui_mod::refresh_hero(state, fav_ui, weak, /* animate */ true);

    if !stale.reaches(mounted) {
        if let Err(e) = hero.await {
            log::warn!("favorites::refresh_hero: {}", describe(&e));
        }
        return;
    }
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
