//! The directory page on screen: what has been loaded, what is being asked for, and the one path
//! that turns the first into the second.
//!
//! **The filter is the query**, so there is no unfiltered list to narrow: a change is a fresh page
//! off the network and the cache holds only what was fetched. **A section leave keeps it**, a
//! re-query being a directory round trip; the leave releases the logo tier ([`super::covers`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use slint::{ComponentHandle, Weak};

use crate::ui::callbacks::card_selection;
use crate::ui::grid_rows::chunk_built_rows;
use crate::ui::model_diff::replace_rows;
use crate::ui::util::len_as_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::radio::{
    DEFAULT_PAGE_LIMIT, DirectoryStation, StationPage, StationSearch,
};
use melodia_ui::{AppWindow, CardSelection, Radio, RadioStationGridRow, RadioStationRow};

use super::state::KeptAnswers;
use super::{RadioUi, covers, logos, rows};

/// Everything loaded for the query currently on screen.
#[derive(Debug)]
pub struct BrowseState {
    /// The query every loaded page answers. Its `offset` is where the *next* page starts.
    search: StationSearch,
    stations: Vec<DirectoryStation>,
    has_more: bool,
    /// Bumped by every fresh query; a page landing under an older one is dropped.
    generation: u64,
    loading: bool,
}

impl Default for BrowseState {
    fn default() -> Self {
        Self {
            // The limit is spelled because the offset advances by it.
            search: StationSearch { limit: DEFAULT_PAGE_LIMIT, ..StationSearch::default() },
            stations: Vec::new(),
            has_more: false,
            generation: 0,
            loading: false,
        }
    }
}

impl BrowseState {
    /// Whether anything has been loaded for the current query.
    pub fn is_empty(&self) -> bool {
        self.stations.is_empty()
    }

    /// Changes the query, reporting whether it moved. The offset is left out of the comparison, so
    /// a filter re-picked to its current value doesn't reset paging.
    fn edit_query(&mut self, edit: impl FnOnce(&mut StationSearch)) -> bool {
        let before = self.identity();
        edit(&mut self.search);
        self.identity() != before
    }

    /// The query minus where it is paged to.
    fn identity(&self) -> StationSearch {
        StationSearch { offset: 0, ..self.search.clone() }
    }

    /// Takes a page's worth of request, or `None` where it would be redundant. Only an append
    /// refuses: a fresh query exists to supersede one in flight.
    fn begin(&mut self, append: bool) -> Option<(StationSearch, u64)> {
        if append {
            if self.loading || !self.has_more {
                return None;
            }
            self.search.offset = self.search.offset.saturating_add(self.search.limit);
        } else {
            self.generation = self.generation.wrapping_add(1);
            self.search.offset = 0;
        }
        self.loading = true;
        Some((self.search.clone(), self.generation))
    }

    /// Fold a landed page in, or drop it as superseded.
    fn finish(&mut self, generation: u64, page: StationPage, append: bool) -> bool {
        if self.generation != generation {
            return false;
        }
        self.loading = false;
        if !append {
            self.stations.clear();
        }
        self.stations.extend(page.stations);
        self.has_more = page.has_more;
        true
    }

    /// Gives a failed request its offset back, so a retry asks for the missed page.
    fn fail(&mut self, generation: u64, append: bool) -> bool {
        if self.generation != generation {
            return false;
        }
        self.loading = false;
        if append {
            self.search.offset = self.search.offset.saturating_sub(self.search.limit);
        }
        true
    }
}

/// What Browse is currently filtered by, for the box the tabs share.
pub fn query_name(radio_ui: &RadioUi) -> String {
    radio_ui.browse.lock().search.name.clone()
}

/// The whole query, for the scope suggestions, which skip a scope the page is filtered by. Cloned
/// so the caller is off this lock before it takes the facet index.
pub fn query(radio_ui: &RadioUi) -> StationSearch {
    radio_ui.browse.lock().search.clone()
}

/// The cached station behind a row and its session logo, by uuid: the model is chunked and
/// rebuilt whenever anything moves, so a position is only true for the frame it was read on.
pub fn resolve(radio_ui: &RadioUi, uuid: &str) -> Option<(DirectoryStation, Option<String>)> {
    let browse = radio_ui.browse.lock();
    let station = browse.stations.iter().find(|station| station.station_uuid == uuid)?.clone();
    let kept = radio_ui.kept_answers.lock();
    let logo = logo_for(radio_ui, kept.get(&station.station_uuid), &station);
    Some((station, logo))
}

/// The cached stations a set of cards' `select_key`s name, each with its logo, skipping a key that
/// names none. A key is a place in the answer, which an appended page keeps and a fresh search
/// renumbers, dropping the selection as it lands ([`paint`]).
pub fn resolve_keys(
    radio_ui: &RadioUi,
    keys: impl IntoIterator<Item = i32>,
) -> Vec<(DirectoryStation, Option<String>)> {
    let browse = radio_ui.browse.lock();
    let kept = radio_ui.kept_answers.lock();
    keys.into_iter()
        .filter_map(|key| {
            let place = usize::try_from(key).ok()?.checked_sub(1)?;
            let station = browse.stations.get(place)?;
            let logo = logo_for(radio_ui, kept.get(&station.station_uuid), station);
            Some((station.clone(), logo))
        })
        .collect()
}

/// 1-based, so no place lands on the `0` a selection refuses.
fn select_key_at(place: usize) -> i32 {
    len_as_i32(place.saturating_add(1))
}

/// The logo a browsed station draws: the `favicon_url` memo first, a moved logo being what it is
/// for; then the kept row, which differs wherever the logo came off the station's own site or the
/// memo is in a backoff; last, a site logo a narrow search discovered.
///
/// `kept` is looked up by the caller, which takes that lock once per pass.
fn logo_for(
    radio_ui: &RadioUi,
    kept: Option<&KeptAnswers>,
    station: &DirectoryStation,
) -> Option<String> {
    station
        .favicon_url
        .as_deref()
        .and_then(|url| radio_ui.logos.path_for(url))
        .or_else(|| kept?.logo.clone())
        .or_else(|| site_logo(radio_ui, station))
}

/// The format a browsed station is drawn with, preferring what an open measured (`OGG/VORBIS`)
/// over the directory's container alone (`OGG`), so it matches the kept tabs.
pub(super) fn format_for(kept: Option<&KeptAnswers>) -> Option<String> {
    kept?.format.clone()
}

/// What this session read off the station's own site, for a row the directory gave no usable
/// favicon. Keyed on the origin, the same spelling [`logos::discover_missing`] records under.
fn site_logo(radio_ui: &RadioUi, station: &DirectoryStation) -> Option<String> {
    let homepage = station.homepage.as_deref().unwrap_or_default();
    let origin = library::radio::site_origin(homepage, &station.stream_url)?;
    radio_ui.logos.path_for(origin.as_str())
}

/// Rebuilds the grid from the cache, the favorites and the logos found so far: the one write path,
/// so nothing needs a second one knowing where in which chunk a station sits.
pub fn apply(ui: &AppWindow, radio_ui: &RadioUi) {
    let g = ui.global::<Radio>();
    let (station_rows, has_more) = {
        let browse = radio_ui.browse.lock();
        let favorite_uuids = radio_ui.favorite_uuids.lock();
        let kept = radio_ui.kept_answers.lock();
        let station_rows: Vec<_> = browse
            .stations
            .iter()
            .enumerate()
            .map(|(place, station)| {
                let answers = kept.get(&station.station_uuid);
                let local = rows::LocalAnswers {
                    is_favorite: favorite_uuids.contains(&station.station_uuid),
                    logo: logo_for(radio_ui, answers, station),
                    format: format_for(answers),
                };
                RadioStationRow {
                    select_key: select_key_at(place),
                    ..rows::to_slint_radio_station_row(station, &local)
                }
            })
            .collect();
        (station_rows, browse.has_more)
    };

    // Above the write: a late count strands the empty state, and `set` is value-compared.
    g.set_browse_count(len_as_i32(station_rows.len()));
    g.set_browse_has_more(has_more);

    // Patched where the page is the same stations in the same places, which is most repaints.
    let columns = g.get_grid_columns();
    if !rows::patch_grid(&g.get_browse_rows(), &station_rows, columns) {
        let grid =
            chunk_built_rows(station_rows, columns, |stations| RadioStationGridRow { stations });
        replace_rows(&g.get_browse_rows(), grid, "radio::browse");
    }

    // The station page draws off the same cache, so it is stamped from the write path.
    super::detail::restamp(ui, radio_ui);
}

/// Changes what Browse asks for and fetches the first page; a no-op where the query didn't move,
/// which makes a reseat or a re-picked chip free.
pub fn edit_query(
    ui: &AppWindow,
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    edit: impl FnOnce(&mut StationSearch),
) {
    if !radio_ui.browse.lock().edit_query(edit) {
        return;
    }
    fetch(ui, state, radio_ui, false);
}

/// Point Browse at a new name filter, which is what the page's search box does.
pub fn set_query(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, needle: &str) {
    edit_query(ui, state, radio_ui, |search| needle.clone_into(&mut search.name));
}

/// Fetch the next page onto the end of what is loaded.
pub fn load_more(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    fetch(ui, state, radio_ui, true);
}

/// Loads the first page on a section enter, if nothing is loaded for the current query.
pub fn ensure_loaded(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let needs_page = {
        let browse = radio_ui.browse.lock();
        browse.is_empty() && !browse.loading
    };
    if needs_page {
        fetch(ui, state, radio_ui, false);
    }
}

/// Re-run the current query from the top, for the retry the unreachable state offers.
pub fn retry(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    fetch(ui, state, radio_ui, false);
}

/// Decodes the retained page's logos again after a section leave dropped them. The counterpart of
/// [`ensure_loaded`], each covering the other's case; it costs no traffic, the memo answering all.
pub fn rewarm(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let generation = {
        let browse = radio_ui.browse.lock();
        if browse.is_empty() {
            return;
        }
        browse.generation
    };
    let (s, ru, weak) = (state.clone(), radio_ui.clone(), ui.as_weak());
    state.runtime.spawn(async move {
        // Not fresh: the harder effort a narrow query earns would re-ask what the memo holds.
        warm_page(&s, &ru, &weak, generation, false).await;
    });
}

/// Asks the directory for a page, then paints it and its logos. On the UI thread, so the loading
/// flag is up on the frame the click lands.
fn fetch(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, append: bool) {
    let Some((search, generation)) = radio_ui.browse.lock().begin(append) else {
        return;
    };

    let g = ui.global::<Radio>();
    g.set_browse_loading(true);
    g.set_browse_failed(false);

    let (s, ru, weak) = (state.clone(), radio_ui.clone(), ui.as_weak());
    state.runtime.spawn(async move {
        let outcome = library::radio::search(&s, &search).await;

        let landed = match outcome {
            Ok(page) => ru.browse.lock().finish(generation, page, append),
            Err(e) => {
                log::warn!("radio: directory search failed: {}", melodia_core::error::describe(&e));
                let current = ru.browse.lock().fail(generation, append);
                if current {
                    let _ = weak.upgrade_in_event_loop(|ui| {
                        let g = ui.global::<Radio>();
                        g.set_browse_loading(false);
                        g.set_browse_failed(true);
                    });
                }
                return;
            }
        };
        if !landed {
            return;
        }

        paint(&s, &weak, &ru, !append);
        warm_page(&s, &ru, &weak, generation, !append).await;
    });
}

/// Pushes the cache onto the grid and lowers the spinner. `fresh` marks a new query: an append
/// coming back empty is the end of the results, not a search that found nothing.
fn paint(state: &AppState, weak: &Weak<AppWindow>, radio_ui: &Arc<RadioUi>, fresh: bool) {
    let (s, ru) = (state.clone(), radio_ui.clone());
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.global::<Radio>().set_browse_loading(false);
        apply(&ui, &ru);
        if fresh {
            // A fresh answer renumbers every place; the kept tabs select by row id.
            let selection = ui.global::<CardSelection>();
            if selection.get_scope() == card_selection::scope::RADIO_BROWSE {
                selection.invoke_clear();
            }
            super::suggest::adopt_only_scope(&ui, &s, &ru);
        }
    });
}

/// Stamps the logos landed so far and announces the tier, every `LOGO_REPAINT_INTERVAL` while a
/// page fills. A landed logo moves no station, so [`apply`] patches rather than resets. The
/// announce matters too: at generation `0` a card asks cache-only and the new path would never be
/// decoded.
fn paint_landed(weak: &Weak<AppWindow>, radio_ui: &Arc<RadioUi>) {
    let ru = radio_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        apply(&ui, &ru);
        if ru.section_active() {
            covers::announce_warm(&ui);
        }
    });
}

/// Asks the sites of whatever the page still has no logo for, after the favicon burst: the order
/// [`library::radio::heal_station_logo`] takes for a kept station. Reached only on a result narrow
/// enough to be the station the user typed.
async fn discover_page(
    state: &AppState,
    radio_ui: &RadioUi,
    is_current: &impl Fn() -> bool,
    on_landed: &impl Fn(),
) -> bool {
    let sites: Vec<(String, String)> = {
        let browse = radio_ui.browse.lock();
        let kept = radio_ui.kept_answers.lock();
        browse
            .stations
            .iter()
            .filter(|station| {
                logo_for(radio_ui, kept.get(&station.station_uuid), station).is_none()
            })
            .map(|station| {
                (station.homepage.clone().unwrap_or_default(), station.stream_url.clone())
            })
            .collect()
    };

    logos::discover_missing(
        state,
        &radio_ui.logos,
        sites.iter().map(|(homepage, stream_url)| (homepage.as_str(), stream_url.as_str())),
        is_current,
        on_landed,
    )
    .await
}

/// How often a filling page repaints while its logos land: long enough to coalesce a burst of
/// whole-model writes, short enough to read as filling in rather than arriving in steps.
const LOGO_REPAINT_INTERVAL: Duration = Duration::from_millis(150);

/// Fetches the page's missing logos, decodes a screenful and repaints as they land, after the grid
/// is on screen so the second round trip doesn't delay first paint.
///
/// A search superseding `generation` stops the burst. `fresh` is a new query rather than an append
/// or a re-warm ([`logos::Effort::for_result`]).
async fn warm_page(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: &Weak<AppWindow>,
    generation: u64,
    fresh: bool,
) {
    // A station whose row holds a logo isn't asked about again. A moved logo goes unnoticed, but
    // only a keep or a play writes the row, so it couldn't have reached it either way.
    let (favicon_urls, effort) = {
        let browse = radio_ui.browse.lock();
        let known = radio_ui.kept_answers.lock();
        let urls: Vec<String> = browse
            .stations
            .iter()
            .filter(|station| {
                known.get(&station.station_uuid).is_none_or(|kept| kept.logo.is_none())
            })
            .filter_map(|station| station.favicon_url.clone())
            .collect();
        (urls, logos::Effort::for_result(fresh, browse.stations.len()))
    };

    let last_repaint: Mutex<Option<Instant>> = Mutex::new(None);
    let is_current = || radio_ui.browse.lock().generation == generation;
    let on_landed = || {
        {
            let mut last = last_repaint.lock();
            if last.is_some_and(|at| at.elapsed() < LOGO_REPAINT_INTERVAL) {
                return;
            }
            *last = Some(Instant::now());
        }
        paint_landed(weak, radio_ui);
    };

    let mut new_logos = logos::fetch_missing(
        state,
        &radio_ui.logos,
        favicon_urls.iter().map(String::as_str),
        effort,
        &is_current,
        &on_landed,
    )
    .await;

    if effort == logos::Effort::Explicit {
        new_logos |= discover_page(state, radio_ui, &is_current, &on_landed).await;
    }

    let artwork_paths: Vec<String> = {
        let browse = radio_ui.browse.lock();
        let kept = radio_ui.kept_answers.lock();
        browse
            .stations
            .iter()
            .filter_map(|station| logo_for(radio_ui, kept.get(&station.station_uuid), station))
            .collect()
    };

    let ru = radio_ui.clone();
    covers::warm_and_announce(radio_ui, weak, artwork_paths, move |ui| {
        if new_logos {
            apply(ui, &ru);
        }
    })
    .await;
}

#[cfg(test)]
#[path = "tests/browse_tests.rs"]
mod tests;
