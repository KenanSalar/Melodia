//! The two local tabs: the stations the user kept, and the ones they played.
//!
//! One module for both because they are one list at two filters — `is_favorite` against
//! `last_played` — drawn by the same grid through the same converter. What differs is the query
//! and whether a sort applies, and both fit in a [`RadioTab`] argument.
//!
//! **Unlike Browse, these are `SQLite` and cost nothing to re-ask**, so the section enter refetches
//! unconditionally and no dirty flag rides with them. Unlike every other grid page, the leave
//! still keeps them: see [`super::covers`] for what a Radio leave actually hands back.
//!
//! The needle is stored raw and folded per apply. A [`Needle`] holds its text case- and
//! accent-folded, which is right for matching and wrong for the box: reseating from it would put
//! a lowercased, unaccented spelling of what the user typed back in front of them.

use std::collections::HashSet;
use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::entities::radio::RadioStation;
use crate::library;
use crate::services::settings::SortDir;
use crate::state::AppState;
use crate::ui::grid_rows::{chunk_built_rows, write_grid};
use crate::ui::row_match::{self, Needle};
use crate::ui::util::len_as_i32;
use crate::{AppWindow, Radio, RadioStationGridRow};

use super::{RadioTab, RadioUi, covers, rows, tab_from_index};

/// One local list: what was fetched, and what the box has narrowed it to.
#[derive(Debug, Default)]
pub struct KeptState {
    stations: Vec<RadioStation>,
    filter: String,
}

impl KeptState {
    fn matches(&self) -> impl Iterator<Item = &RadioStation> {
        let needle = row_match::fold_needle(&self.filter);
        self.stations.iter().filter(move |station| station_matches(station, &needle))
    }
}

/// What a station row is searchable by.
///
/// The card's own three lines plus the two fields behind them the user is most likely to reach
/// for — a station is remembered by its country or its genre at least as often as by its name.
/// An empty needle matches everything, so an unfiltered list needs no branch.
fn station_matches(station: &RadioStation, needle: &Needle) -> bool {
    needle.contains(&station.name)
        || needle.contains(&station.tags)
        || needle.contains(&station.country)
        || needle.contains(&station.codec)
}

/// The cache a tab draws from.
fn cache(radio_ui: &RadioUi, tab: RadioTab) -> &parking_lot::Mutex<KeptState> {
    match tab {
        RadioTab::Recent => &radio_ui.recent,
        _ => &radio_ui.kept,
    }
}

/// Whichever of the two local tabs is mounted, or `None` on Browse.
fn local_tab(g: &Radio<'_>) -> Option<RadioTab> {
    match tab_from_index(g, g.get_tab_idx()) {
        RadioTab::Browse => None,
        tab => Some(tab),
    }
}

/// What a tab is currently filtered by, for the box the three tabs share.
pub fn filter_text(radio_ui: &RadioUi, tab: RadioTab) -> String {
    cache(radio_ui, tab).lock().filter.clone()
}

/// Point a tab at a new needle and repaint it.
pub fn set_filter(ui: &AppWindow, radio_ui: &RadioUi, tab: RadioTab, filter: &str) {
    {
        let mut state = cache(radio_ui, tab).lock();
        if state.filter == filter {
            return;
        }
        filter.clone_into(&mut state.filter);
    }
    apply(ui, radio_ui, tab);
}

/// The station behind a row, by database id.
///
/// By id rather than by index for the reason Browse resolves by uuid: the model is chunked and
/// rebuilt whenever anything moves, so a position is only true for the frame it was read on.
pub fn resolve(radio_ui: &RadioUi, tab: RadioTab, id: i64) -> Option<RadioStation> {
    cache(radio_ui, tab).lock().stations.iter().find(|station| station.id == id).cloned()
}

/// Rebuild one tab's grid from its cache.
///
/// **The one write path**, as `browse::apply` is for the directory: a landed fetch, a keystroke, a
/// sort pick and a column change all come through here rather than each patching rows in place.
///
/// UI thread only.
pub fn apply(ui: &AppWindow, radio_ui: &RadioUi, tab: RadioTab) {
    let g = ui.global::<Radio>();
    let (station_rows, held): (Vec<_>, usize) = {
        let state = cache(radio_ui, tab).lock();
        let mut matched: Vec<&RadioStation> = state.matches().collect();
        // Recently Played takes the query's own order: newest first *is* the page.
        if tab == RadioTab::Favorites {
            sort_stations(
                &mut matched,
                g.get_sort_field().as_str(),
                SortDir::from_token(g.get_sort_dir().as_str()),
            );
        }
        let rows = matched.into_iter().map(rows::to_slint_kept_station_row).collect();
        (rows, state.stations.len())
    };

    // **The count is pre-filter**, which is what the global declares and what lets the tab body
    // tell "nothing kept" from "the box matched nothing" — the two want different empty states,
    // and a filtered count reads as the first when it is the second.
    //
    // Above the write, for the reason every tabbed page writes its count above its signature
    // guard: a count that arrives late strands the empty state.
    let count = len_as_i32(held);
    if tab == RadioTab::Favorites {
        g.set_favorites_count(count);
    } else {
        g.set_recent_count(count);
    }

    let grid = chunk_built_rows(station_rows, g.get_grid_columns(), |stations| {
        RadioStationGridRow { stations }
    });
    let model = if tab == RadioTab::Favorites {
        g.get_favorites_rows()
    } else {
        g.get_recent_rows()
    };
    write_grid(&model, grid, "radio::kept");
}

/// Order the kept list.
///
/// Every field tie-breaks on `sort_key`, which is the natural-sort column the table already
/// carries — without it two stations sharing a play count swap places between refreshes. Reversed
/// rather than branched for `Desc`, mirroring `ui::favorites::grids::sort`.
fn sort_stations(stations: &mut [&RadioStation], field: &str, dir: SortDir) {
    match field {
        "added" => stations.sort_by(|a, b| {
            a.date_added.cmp(&b.date_added).then_with(|| a.sort_key.cmp(&b.sort_key))
        }),
        "plays" => stations.sort_by(|a, b| {
            a.play_count.cmp(&b.play_count).then_with(|| a.sort_key.cmp(&b.sort_key))
        }),
        // A station never played sorts below every station that was, which is what puts the
        // never-played end of the list under the ascending arrow rather than scattered through it.
        "played" => stations.sort_by(|a, b| {
            a.last_played.cmp(&b.last_played).then_with(|| a.sort_key.cmp(&b.sort_key))
        }),
        _ => stations.sort_by(|a, b| a.sort_key.cmp(&b.sort_key)),
    }
    if matches!(dir, SortDir::Desc) {
        stations.reverse();
    }
}

/// Re-read both lists and repaint whichever tab is on screen.
///
/// **One fetch answers three things**: the kept tab's rows, the recents tab's rows, and the uuid
/// set Browse fills its stars from. A separate `favorite_uuids` query used to answer the last on
/// its own and was a second statement to keep true.
pub fn refresh(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    // Read on the way out: the background task cannot reach a Slint global, and a tab picked
    // mid-fetch only means the warm targets the one that was up — the pick warms its own, and
    // both tabs share the tier anyway.
    let mounted = local_tab(&ui.global::<Radio>());
    let (s, ru, weak) = (state.clone(), radio_ui.clone(), ui.as_weak());
    state.runtime.spawn(async move {
        let favorites = match library::radio::get_favorites(&s).await {
            Ok(favorites) => favorites,
            Err(e) => {
                log::warn!("radio: kept stations: {}", crate::services::describe(&e));
                return;
            }
        };
        let recent = match library::radio::get_recent(&s).await {
            Ok(recent) => recent,
            Err(e) => {
                log::warn!("radio: recent stations: {}", crate::services::describe(&e));
                return;
            }
        };

        let starred: HashSet<String> =
            favorites.iter().filter_map(|station| station.station_uuid.clone()).collect();
        *ru.starred.lock() = starred;
        ru.kept.lock().stations = favorites;
        ru.recent.lock().stations = recent;

        paint_mounted(&weak, &ru);
        if let Some(tab) = mounted {
            warm(&ru, &weak, tab).await;
        }
    });
}

/// Repaint the mounted local tab, and Browse's stars with it.
fn paint_mounted(weak: &Weak<AppWindow>, radio_ui: &Arc<RadioUi>) {
    let ru = radio_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        // Browse draws its stars from the same set, so a favorite that moved on another tab lands
        // here rather than waiting for the next directory page.
        super::browse::apply(&ui, &ru);
        if let Some(tab) = local_tab(&ui.global::<Radio>()) {
            apply(&ui, &ru, tab);
        }
    });
}

/// Decode a screenful of one tab's logos and announce the tier.
///
/// Nothing is downloaded: a kept station's logo was stored when it was kept, so this only ever
/// reads files already on disk. The `spawn_blocking` is the decode, which must not sit on a
/// worker — the same rule every grid tier follows.
async fn warm(radio_ui: &Arc<RadioUi>, weak: &Weak<AppWindow>, tab: RadioTab) {
    let paths: Vec<String> = {
        let state = cache(radio_ui, tab).lock();
        state.matches().filter_map(|station| station.artwork_path.clone()).collect()
    };

    let ru_warm = radio_ui.clone();
    // `Some(())` only once the decode reports the tier is still its own: a leave landing inside
    // the burst hands the buffers back, and announcing anyway would bump the generation over an
    // emptied tier. A `JoinError` is the same "we don't know".
    let warmed = tokio::task::spawn_blocking(move || covers::prewarm(&ru_warm, &paths))
        .await
        .unwrap_or(false);

    let ru = radio_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if warmed && ru.section_active() {
            covers::announce_warm(&ui);
        }
    });
}

/// Bring a tab the user just picked up to date from cache.
///
/// A pick runs against whatever is already loaded — the fetch is the enter's, not the pick's — so
/// this paints and warms and asks for nothing.
pub fn on_tab_entered(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, tab: RadioTab) {
    apply(ui, radio_ui, tab);
    let (ru, weak) = (radio_ui.clone(), ui.as_weak());
    state.runtime.spawn(async move {
        warm(&ru, &weak, tab).await;
    });
}

#[cfg(test)]
#[path = "tests/kept_tests.rs"]
mod tests;
