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

use std::collections::{HashMap, HashSet};
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

use super::{RadioTab, RadioUi, covers, mounted_tab, rows};

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
/// The card's own three lines plus the fields behind them the user is most likely to reach for —
/// a station is remembered by its country or its genre at least as often as by its name. An empty
/// needle matches everything, so an unfiltered list needs no branch.
///
/// Through the resolvers, so the box searches what the card draws: a genre the user typed over a
/// blank directory entry is on screen, and a directory value an override replaced is not.
///
/// **Bitrate is `Option` here because `0` is the directory saying it does not know**, not a
/// station that streams at zero, and a large share of live rows carry it — matched raw, the needle
/// `0` would select all of them. It is also the one field matched whole rather than as a
/// substring: the values come off a short ladder that shares digits, where a single one selects
/// most of the list.
fn station_matches(station: &RadioStation, needle: &Needle) -> bool {
    needle.contains(&station.name)
        || needle.contains(station.genre().unwrap_or_default())
        || needle.contains(station.country_name().unwrap_or_default())
        || needle.contains(&station.language)
        || needle.contains(&station.codec)
        || needle.equals_number((station.bitrate > 0).then_some(station.bitrate))
}

/// The Favorites tab's sort, lifted off the global by whoever holds the UI thread.
///
/// A snapshot rather than a live read because the two prewarm sites are on a worker by the time
/// they need it, and `Radio` is a Slint global. `None` is Recently Played, whose order is the
/// query's.
struct KeptSort {
    field: String,
    dir: SortDir,
}

impl KeptSort {
    fn of(g: &Radio<'_>, tab: RadioTab) -> Option<Self> {
        (tab == RadioTab::Favorites).then(|| Self {
            field: g.get_sort_field().to_string(),
            dir: SortDir::from_token(g.get_sort_dir().as_str()),
        })
    }
}

/// What a tab shows, in the order it shows it. The one walk the grid and the prewarm share, so
/// neither can end up describing a list the other isn't drawing.
fn in_display_order<'a>(state: &'a KeptState, sort: Option<&KeptSort>) -> Vec<&'a RadioStation> {
    let mut matched: Vec<&RadioStation> = state.matches().collect();
    if let Some(sort) = sort {
        sort_stations(&mut matched, &sort.field, sort.dir);
    }
    matched
}

/// The logos a tab's visible list points at, in display order.
///
/// Order is what a prewarm spends its capacity on — it keeps the prefix the tier can hold — and
/// the tier is shared with Browse, so a long kept list is not guaranteed the whole of it.
fn warm_targets(radio_ui: &RadioUi, tab: RadioTab, sort: Option<&KeptSort>) -> Vec<String> {
    let state = cache(radio_ui, tab).lock();
    in_display_order(&state, sort)
        .into_iter()
        .filter_map(|station| station.artwork_path.clone())
        .collect()
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
    match mounted_tab(g) {
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
    // `None` on Recently Played, which takes the query's own order: newest first *is* the page.
    let sort = KeptSort::of(&g, tab);
    let (station_rows, held): (Vec<_>, usize) = {
        let state = cache(radio_ui, tab).lock();
        let rows = in_display_order(&state, sort.as_ref())
            .into_iter()
            .map(rows::to_slint_kept_station_row)
            .collect();
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

    let model = if tab == RadioTab::Favorites {
        g.get_favorites_rows()
    } else {
        g.get_recent_rows()
    };
    // `browse::apply`'s reason: a refetch that moved one row must not reset a grid the pointer is
    // sitting on. Recently Played is the tab that genuinely reshapes — a play moves its station to
    // the front — and there the full write is what the reorder asks for anyway.
    let columns = g.get_grid_columns();
    if !rows::patch_grid(&model, &station_rows, columns) {
        let grid =
            chunk_built_rows(station_rows, columns, |stations| RadioStationGridRow { stations });
        write_grid(&model, grid, "radio::kept");
    }

    // `browse::apply`'s reason: the station page draws a station this cache owns, so it follows
    // the write path rather than each caller that could have moved a field.
    super::detail::restamp(ui, radio_ui);
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
    // Read on the way out, the sort with it: the background task cannot reach a Slint global, and
    // a tab picked mid-fetch only means the warm targets the one that was up — the pick warms its
    // own, and both tabs share the tier anyway.
    let (mounted, sort) = {
        let g = ui.global::<Radio>();
        let mounted = local_tab(&g);
        (mounted, mounted.and_then(|tab| KeptSort::of(&g, tab)))
    };
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

        // A stored path outlives its file more easily than the row outlives the path, so the
        // column is not evidence on its own. Blanked here rather than in the converter: the
        // projection stays a projection, and the walk lands off the UI thread. A card with no path
        // draws its monogram; one with a dead path drew nothing.
        //
        // Ahead of everything derived from these lists, so nothing downstream can hand a card a
        // path this pass already knows is gone. On the blocking pool because it is a `stat` per
        // row, and a large Favorites tab is a runtime worker parked on the filesystem.
        let Ok((favorites, recent)) = tokio::task::spawn_blocking(move || {
            (forget_absent_artwork(favorites), forget_absent_artwork(recent))
        })
        .await
        else {
            return;
        };

        let starred: HashSet<String> =
            favorites.iter().filter_map(|station| station.station_uuid.clone()).collect();
        *ru.starred.lock() = starred;
        ru.kept.lock().stations = favorites;
        ru.recent.lock().stations = recent;
        remember_logos(&ru);

        paint_mounted(&weak, &ru);
        if let Some(tab) = mounted {
            warm(&ru, &weak, warm_targets(&ru, tab, sort.as_ref())).await;
        }
        heal_logos(&s, &ru, &weak).await;
    });
}

/// Re-derive the uuid-keyed logos Browse falls back on, from whatever the two caches now hold.
///
/// **Off the caches rather than off the rows a caller has in hand**, so the map cannot describe a
/// list that has since moved: the bulk refresh and a landed heal both change what a row holds, and
/// a partial update is exactly Browse painting a monogram beside a tab drawing the real thing.
///
/// Both lists, since a station can be in either — a played-but-unstarred station is as likely to
/// come back in a directory page as a favorite. One the user typed in carries no uuid and no page
/// can name it, so it contributes nothing.
fn remember_logos(radio_ui: &RadioUi) {
    let mut logos = HashMap::new();
    for cache in [&radio_ui.kept, &radio_ui.recent] {
        for station in &cache.lock().stations {
            if let (Some(uuid), Some(path)) = (&station.station_uuid, &station.artwork_path) {
                logos.insert(uuid.clone(), path.clone());
            }
        }
    }
    *radio_ui.known_logos.lock() = logos;
}

/// Drop artwork paths whose file is gone, so the row says what is actually drawable.
fn forget_absent_artwork(mut stations: Vec<RadioStation>) -> Vec<RadioStation> {
    for station in &mut stations {
        if !library::radio::artwork_is_present(station.artwork_path.as_deref()) {
            station.artwork_path = None;
        }
    }
    stations
}

/// How many logo repairs to have in flight at once.
///
/// Below Browse's window: a repair can cost two requests rather than one, and it runs behind a
/// list that is already on screen rather than in front of one that is not.
const HEAL_BATCH: usize = 4;

/// Re-fetch logos for kept stations whose row has none, and repaint whatever lands.
///
/// **After the paint and the warm, never before them.** This is network work on behalf of a list
/// already on screen; a station that has been without a logo since it was kept can wait for its
/// tab to be drawn.
///
/// **Once per station per session**, which is what makes the cost bear the frequency: `refresh`
/// runs on a section enter, on every star flip — Browse's included — and on every removal, and a
/// station that failed is a stored backoff a repeat is told about without spending a round trip.
/// The set is what the backoff already says, held where a click can't spend a round trip on it.
///
/// **The stored answers for the whole set are read ahead of the flight**, one query rather than
/// two per station — `library::radio::AnswerSeed` argues why. Shared rather than handed out per
/// task, hence the `Arc`: which answers a station needs has nothing to do with its seat in the
/// window.
async fn heal_logos(state: &AppState, radio_ui: &Arc<RadioUi>, weak: &Weak<AppWindow>) {
    let logoless = logoless_stations(radio_ui);
    let seed = Arc::new(
        library::radio::AnswerSeed::for_urls(state, &library::radio::heal_seed_urls(&logoless))
            .await,
    );

    let mut pending = logoless.into_iter();
    let mut in_flight = tokio::task::JoinSet::new();
    for station in pending.by_ref().take(HEAL_BATCH) {
        spawn_heal(&mut in_flight, state, &seed, station);
    }

    let mut landed = false;
    while let Some(joined) = in_flight.join_next().await {
        if let Some(station) = pending.next() {
            spawn_heal(&mut in_flight, state, &seed, station);
        }
        let Ok(Some((id, path))) = joined else {
            continue;
        };
        adopt_logo_path(radio_ui, id, &path);
        landed = true;
    }
    if landed {
        // Browse draws from the map, not from the caches, so it has to be re-derived here too —
        // this is the one path that finds a logo *after* the refresh built it.
        remember_logos(radio_ui);
        // Both grids, because a heal moves both: the row's own `artwork_path`, and the uuid-keyed
        // map a browsed card reads. Only the paths moved, so each `apply` patches rather than
        // resetting, which is what lets this land under a pointer that is mid-click.
        paint_mounted(weak, radio_ui);
    }
}

fn spawn_heal(
    in_flight: &mut tokio::task::JoinSet<Option<(i64, String)>>,
    state: &AppState,
    seed: &Arc<library::radio::AnswerSeed>,
    station: RadioStation,
) {
    let state = state.clone();
    let seed = Arc::clone(seed);
    in_flight.spawn(async move {
        let path = library::radio::heal_station_logo(&state, &seed, &station).await?;
        Some((station.id, path))
    });
}

/// The kept and recent stations carrying no drawable logo that this session has not already tried,
/// deduplicated by id.
///
/// The two lists overlap wherever a favorite has been played, and a station healed under one is
/// the same row as the one healed under the other. Claiming the id here rather than after the
/// attempt: what the walk is skipping is *asking again*, and that is owed whether the attempt
/// found a logo or nothing.
fn logoless_stations(radio_ui: &RadioUi) -> Vec<RadioStation> {
    let mut tried = radio_ui.healed.lock();
    let mut out = Vec::new();
    for cache in [&radio_ui.kept, &radio_ui.recent] {
        for station in &cache.lock().stations {
            if station.artwork_path.is_none() && tried.insert(station.id) {
                out.push(station.clone());
            }
        }
    }
    out
}

/// Point both caches' copies of a station at the logo that just landed.
fn adopt_logo_path(radio_ui: &RadioUi, id: i64, path: &str) {
    for cache in [&radio_ui.kept, &radio_ui.recent] {
        for station in &mut cache.lock().stations {
            if station.id == id {
                station.artwork_path = Some(path.to_owned());
            }
        }
    }
}

/// Repaint the mounted local tab, and Browse's stars with it.
///
/// Twice per [`refresh`]: once on the landing, once more if a heal found a logo behind it.
fn paint_mounted(weak: &Weak<AppWindow>, radio_ui: &Arc<RadioUi>) {
    let ru = radio_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        // Browse draws its stars from the same set, so a favorite that moved on another tab lands
        // here rather than waiting for the next directory page.
        super::browse::apply(&ui, &ru);
        if let Some(tab) = local_tab(&ui.global::<Radio>()) {
            apply(&ui, &ru, tab);
        }
        // **From this path and no other**: both caches were rebuilt from the database above and a
        // heal only ever stamps a logo onto one, so a station in neither is a station that is
        // gone. Everywhere else a miss is a cache that has not been filled yet.
        super::detail::close_if_gone(&ui, &ru);
    });
}

/// Decode a screenful of one tab's logos and announce the tier.
///
/// Nothing is downloaded: a kept station's logo was stored when it was kept, so this only ever
/// reads files already on disk. Paths arrive already ordered, since only a caller on the UI thread
/// can read the sort they are ordered by. There are no rows to repaint beside the announce — the
/// apply that produced this order has already run.
async fn warm(radio_ui: &Arc<RadioUi>, weak: &Weak<AppWindow>, paths: Vec<String>) {
    covers::warm_and_announce(radio_ui, weak, paths, |_| {}).await;
}

/// Bring a tab the user just picked up to date from cache.
///
/// A pick runs against whatever is already loaded — the fetch is the enter's, not the pick's — so
/// this paints and warms and asks for nothing.
pub fn on_tab_entered(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, tab: RadioTab) {
    apply(ui, radio_ui, tab);
    let sort = KeptSort::of(&ui.global::<Radio>(), tab);
    let paths = warm_targets(radio_ui, tab, sort.as_ref());
    let (ru, weak) = (radio_ui.clone(), ui.as_weak());
    state.runtime.spawn(async move {
        warm(&ru, &weak, paths).await;
    });
}

#[cfg(test)]
#[path = "tests/kept_tests.rs"]
mod tests;
