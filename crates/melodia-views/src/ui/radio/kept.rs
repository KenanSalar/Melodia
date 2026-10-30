//! The two local tabs: the stations the user kept, and the ones they played. One list at two
//! filters, drawn by the same grid; the query and whether a sort applies are a [`RadioTab`] away.
//!
//! **These are `SQLite`**, so the section enter refetches unconditionally with no dirty flag; the
//! leave still keeps them ([`super::covers`]).
//!
//! The needle is stored raw and folded per apply: reseating the box from a [`Needle`] would hand
//! the user back a lowercased, unaccented spelling of what they typed.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::ui::grid_rows::chunk_built_rows;
use crate::ui::model_diff::replace_rows;
use crate::ui::util::len_as_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::radio::RadioStation;
use melodia_core::entities::track_list::SortDir;
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::{AppWindow, Radio, RadioStationGridRow};

use super::state::KeptAnswers;
use super::{RadioTab, RadioUi, covers, mounted_tab, rows};

/// One local list: what was fetched, and what the box has narrowed it to.
#[derive(Debug, Default)]
pub struct KeptState {
    stations: Vec<RadioStation>,
    filter: String,
}

impl KeptState {
    fn matches(&self) -> impl Iterator<Item = &RadioStation> {
        let needle = fold_needle(&self.filter);
        self.stations.iter().filter(move |station| station_matches(station, &needle))
    }
}

/// What a station row is searchable by: the card's three lines and the fields behind them, through
/// the resolvers so the box searches what the card draws. An empty needle matches everything.
///
/// Format is read as its two columns, [`RadioStation::format`] allocating per row per keystroke.
/// Bitrate is `None` at `0`, the directory's "unknown", and matched whole: its values share
/// digits, so a substring would select most of the list.
fn station_matches(station: &RadioStation, needle: &Needle) -> bool {
    needle.contains(&station.name)
        || needle.contains(station.genre().unwrap_or_default())
        || needle.contains(station.country_name().unwrap_or_default())
        || needle.contains(&station.language)
        || needle.contains(&station.codec)
        || needle.contains(station.probed_codec.as_deref().unwrap_or_default())
        || needle.equals_number((station.bitrate > 0).then_some(station.bitrate))
}

/// The Favorites tab's sort, snapshotted on the UI thread for the prewarm sites on a worker.
/// `None` is Recently Played, whose order is the query's.
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

/// What a tab shows, in order: the one walk the grid and the prewarm share.
fn in_display_order<'a>(state: &'a KeptState, sort: Option<&KeptSort>) -> Vec<&'a RadioStation> {
    let mut matched: Vec<&RadioStation> = state.matches().collect();
    if let Some(sort) = sort {
        sort_stations(&mut matched, &sort.field, sort.dir);
    }
    matched
}

/// The logos a tab's list points at in display order, the prefix being what a prewarm keeps.
fn warm_targets(radio_ui: &RadioUi, tab: RadioTab, sort: Option<&KeptSort>) -> Vec<String> {
    let state = cache(radio_ui, tab).lock();
    in_display_order(&state, sort)
        .into_iter()
        .filter_map(|station| station.artwork_path.clone())
        .collect()
}

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

/// The station behind a row, by database id, for the reason [`super::browse::resolve`] goes by
/// uuid.
pub fn resolve(radio_ui: &RadioUi, tab: RadioTab, id: i64) -> Option<RadioStation> {
    cache(radio_ui, tab).lock().stations.iter().find(|station| station.id == id).cloned()
}

/// The stations a set of cards' ids name, in the order given, skipping an id that names none:
/// [`resolve`] over a selection under one lock, as [`super::browse::resolve_keys`] is for Browse.
pub fn resolve_keys(
    radio_ui: &RadioUi,
    tab: RadioTab,
    ids: impl IntoIterator<Item = i64>,
) -> Vec<RadioStation> {
    let state = cache(radio_ui, tab).lock();
    ids.into_iter()
        .filter_map(|id| state.stations.iter().find(|station| station.id == id).cloned())
        .collect()
}

/// Rebuilds one tab's grid from its cache: the one write path, as `browse::apply` is for Browse.
pub fn apply(ui: &AppWindow, radio_ui: &RadioUi, tab: RadioTab) {
    let g = ui.global::<Radio>();
    let sort = KeptSort::of(&g, tab);
    let (station_rows, held): (Vec<_>, usize) = {
        let state = cache(radio_ui, tab).lock();
        let rows = in_display_order(&state, sort.as_ref())
            .into_iter()
            .map(rows::to_slint_kept_station_row)
            .collect();
        (rows, state.stations.len())
    };

    // Pre-filter, so the body can tell "nothing kept" from "the box matched nothing"; above the
    // write, since a late count strands the empty state.
    let count = len_as_i32(held);
    if tab == RadioTab::Favorites {
        g.set_favorites_count(count);
    } else {
        g.set_recent_count(count);
    }

    let model =
        if tab == RadioTab::Favorites { g.get_favorites_rows() } else { g.get_recent_rows() };
    // Patched as in `browse::apply`. A play reshapes Recently Played, and there the reset is what
    // the reorder asks for anyway.
    let columns = g.get_grid_columns();
    if !rows::patch_grid(&model, &station_rows, columns) {
        let grid =
            chunk_built_rows(station_rows, columns, |stations| RadioStationGridRow { stations });
        replace_rows(&model, grid, "radio::kept");
    }

    // The station page draws off this cache too.
    super::detail::restamp(ui, radio_ui);
}

/// Orders the kept list, every field tie-breaking on `sort_key` so equal stations don't swap
/// places between refreshes.
fn sort_stations(stations: &mut [&RadioStation], field: &str, dir: SortDir) {
    match field {
        "added" => stations.sort_by(|a, b| {
            a.date_added.cmp(&b.date_added).then_with(|| a.sort_key.cmp(&b.sort_key))
        }),
        "plays" => stations.sort_by(|a, b| {
            a.play_count.cmp(&b.play_count).then_with(|| a.sort_key.cmp(&b.sort_key))
        }),
        // `None` sorts first, so the never-played stations gather at the ascending end.
        "played" => stations.sort_by(|a, b| {
            a.last_played.cmp(&b.last_played).then_with(|| a.sort_key.cmp(&b.sort_key))
        }),
        _ => stations.sort_by(|a, b| a.sort_key.cmp(&b.sort_key)),
    }
    if matches!(dir, SortDir::Desc) {
        stations.reverse();
    }
}

/// Re-reads both lists and repaints the mounted tab. One fetch answers both tabs' rows and the
/// uuid set Browse fills its hearts from.
pub fn refresh(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    // Read here, out of the task's reach; a tab picked mid-fetch warms its own.
    let (mounted, sort) = {
        let g = ui.global::<Radio>();
        let mounted = local_tab(&g);
        (mounted, mounted.and_then(|tab| KeptSort::of(&g, tab)))
    };
    let (s, ru, weak) = (state.clone(), radio_ui.clone(), ui.as_weak());
    state.runtime.spawn(async move {
        let favorites = match library::radio::get_favorites(&s.db).await {
            Ok(favorites) => favorites,
            Err(e) => {
                log::warn!("radio: kept stations: {}", melodia_core::error::describe(&e));
                return;
            }
        };
        let recent = match library::radio::get_recent(&s.db).await {
            Ok(recent) => recent,
            Err(e) => {
                log::warn!("radio: recent stations: {}", melodia_core::error::describe(&e));
                return;
            }
        };

        // A stored path outlives its file, so a dead one is blanked ahead of everything derived
        // from these lists, on the blocking pool, being a `stat` per row. A card with no path
        // draws its monogram; one with a dead path drew nothing.
        let Ok((favorites, recent)) = tokio::task::spawn_blocking(move || {
            (forget_absent_artwork(favorites), forget_absent_artwork(recent))
        })
        .await
        else {
            return;
        };

        let favorite_uuids: HashSet<String> =
            favorites.iter().filter_map(|station| station.station_uuid.clone()).collect();
        *ru.favorite_uuids.lock() = favorite_uuids;
        ru.kept.lock().stations = favorites;
        ru.recent.lock().stations = recent;
        remember_kept_answers(&ru);

        paint_mounted(&weak, &ru);
        if let Some(tab) = mounted {
            warm(&ru, &weak, warm_targets(&ru, tab, sort.as_ref())).await;
        }
        heal_logos(&s, &ru, &weak).await;
    });
}

/// Re-derives the uuid-keyed map Browse falls back on, off the caches rather than a caller's rows
/// so it can't describe a list that has since moved. Both lists feed it; a hand-typed station has
/// no uuid for a directory page to name.
fn remember_kept_answers(radio_ui: &RadioUi) {
    let mut answers: HashMap<String, KeptAnswers> = HashMap::new();
    for cache in [&radio_ui.kept, &radio_ui.recent] {
        for station in &cache.lock().stations {
            let Some(uuid) = &station.station_uuid else {
                continue;
            };
            let format = station.format().map(Cow::into_owned);
            // Presence in the map means the station told Browse something.
            if station.artwork_path.is_none() && format.is_none() {
                continue;
            }
            // Per field: in both caches, one row's gap must not blank the other's answer.
            let answer = answers.entry(uuid.clone()).or_default();
            if let Some(path) = &station.artwork_path {
                answer.logo = Some(path.clone());
            }
            if format.is_some() {
                answer.format = format;
            }
        }
    }
    *radio_ui.kept_answers.lock() = answers;
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

/// Logo repairs in flight at once, below Browse's window: a repair can cost two requests and runs
/// behind a list already on screen.
const HEAL_BATCH: usize = 4;

/// Re-fetches logos for kept stations whose row has none and repaints what lands, after the paint
/// and the warm.
///
/// Once per station per session: `refresh` runs on every enter, favorite flip and removal. The
/// stored answers are read for the whole set up front (`library::radio::AnswerSeed` argues why)
/// and shared, a station's answers having nothing to do with its seat in the window.
async fn heal_logos(state: &AppState, radio_ui: &Arc<RadioUi>, weak: &Weak<AppWindow>) {
    let logoless = logoless_stations(radio_ui);
    let seed = Arc::new(
        library::radio::AnswerSeed::for_urls(&state.db, &library::radio::heal_seed_urls(&logoless))
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
        // The one path that finds a logo after the refresh built the map. Only paths moved, so
        // each `apply` patches rather than resets under a pointer mid-click.
        remember_kept_answers(radio_ui);
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

/// The kept and recent stations with no drawable logo this session hasn't tried, deduplicated by
/// id. Claimed before the attempt: what is skipped is asking again, whatever the answer was.
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

/// Repaints the mounted local tab and Browse's hearts: on [`refresh`]'s landing and after a heal.
fn paint_mounted(weak: &Weak<AppWindow>, radio_ui: &Arc<RadioUi>) {
    let ru = radio_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        super::browse::apply(&ui, &ru);
        if let Some(tab) = local_tab(&ui.global::<Radio>()) {
            apply(&ui, &ru, tab);
        }
        // The one path where a miss is authoritative, both caches having just been rebuilt.
        super::detail::close_if_gone(&ui, &ru);
    });
}

/// Decodes a screenful of one tab's logos, already on disk, and announces the tier. The paths
/// arrive sorted, the sort being the UI thread's to read.
async fn warm(radio_ui: &Arc<RadioUi>, weak: &Weak<AppWindow>, paths: Vec<String>) {
    covers::warm_and_announce(radio_ui, weak, paths, |_| {}).await;
}

/// Brings a picked tab up to date from cache; the fetch is the enter's, not the pick's.
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
