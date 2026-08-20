//! The directory page on screen: what has been loaded, what is being asked for, and the one path
//! that turns the first into the second.
//!
//! **Unlike the library grids there is no unfiltered list to filter against.** Albums keeps every
//! row and re-derives the model on each keystroke; here the filter *is* the query, so a change of
//! it is a fresh page off the network and the cache holds only what was actually fetched.
//!
//! **A section leave keeps that cache.** Every other grid in the tree hands its rows back on the
//! way out and re-queries on the way in, which is free against `SQLite` and a directory round trip
//! here. What the leave releases is the logo tier, which is where the bytes are; see
//! [`super::covers`].

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::entities::radio::{DirectoryStation, StationPage, StationSearch};
use crate::library;
use crate::state::AppState;
use crate::ui::grid_rows::{chunk_built_rows, write_grid};
use crate::ui::util::len_as_i32;
use crate::{AppWindow, Radio, RadioStationGridRow};

use super::{RadioUi, covers, logos, rows};

/// Everything loaded for the query currently on screen.
#[derive(Debug)]
pub struct BrowseState {
    /// The query every loaded page answers. Its `offset` is where the *next* page starts.
    search: StationSearch,
    stations: Vec<DirectoryStation>,
    has_more: bool,
    /// Bumped by every fresh query. A page landing under an older one is dropped, which is what
    /// lets a new search supersede one still in flight rather than waiting for it.
    generation: u64,
    loading: bool,
}

impl Default for BrowseState {
    fn default() -> Self {
        Self {
            // `StationSearch::default()` is the first screen with nothing typed: most-clicked
            // first, no filters. The limit is spelled because the offset advances by it.
            search: StationSearch {
                limit: library::radio::DEFAULT_PAGE_LIMIT,
                ..StationSearch::default()
            },
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

    /// Change the query, reporting whether it actually moved.
    ///
    /// The offset is excluded from the comparison because it is where the *next* page starts
    /// rather than part of what is being asked for: a filter re-picked to the value it already
    /// has must not reset paging, and a genuine change must.
    fn edit_query(&mut self, edit: impl FnOnce(&mut StationSearch)) -> bool {
        let before = self.identity();
        edit(&mut self.search);
        self.identity() != before
    }

    /// The query minus where it is paged to.
    fn identity(&self) -> StationSearch {
        StationSearch {
            offset: 0,
            ..self.search.clone()
        }
    }

    /// Take a page's worth of request, or `None` where the request would be redundant.
    ///
    /// An append refuses while one is out and at the end of the results; a fresh query never
    /// refuses, because superseding an in-flight page is exactly what it is for.
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

    /// Give a failed request its offset back, so a retry asks for the page that was missed rather
    /// than the one after it.
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

/// The cached station behind a row, and whatever logo this session found for it.
///
/// A row identifies a browsed station by its uuid rather than by an index: the model is chunked
/// and rebuilt whenever anything moves, so a position is only true for the frame it was read on.
pub fn resolve(radio_ui: &RadioUi, uuid: &str) -> Option<(DirectoryStation, Option<String>)> {
    let browse = radio_ui.browse.lock();
    let station = browse.stations.iter().find(|station| station.station_uuid == uuid)?.clone();
    let logo = station.favicon_url.as_deref().and_then(|url| radio_ui.logos.path_for(url));
    Some((station, logo))
}

/// Rebuild the grid from the cache, this install's favorites and the logos found so far.
///
/// **The one write path.** A landed page, a landed logo batch, a favorite flip and a column change
/// all come through here, which is what keeps the chunked model from needing a second, per-row
/// patch path that would have to know where in which chunk a station sits.
///
/// UI thread only.
pub fn apply(ui: &AppWindow, radio_ui: &RadioUi) {
    let g = ui.global::<Radio>();
    let (station_rows, has_more) = {
        let browse = radio_ui.browse.lock();
        let favorites = radio_ui.favorites.lock();
        let station_rows: Vec<_> = browse
            .stations
            .iter()
            .map(|station| {
                let logo =
                    station.favicon_url.as_deref().and_then(|url| radio_ui.logos.path_for(url));
                rows::to_slint_radio_station_row(
                    station,
                    favorites.contains(&station.station_uuid),
                    logo.as_deref(),
                )
            })
            .collect();
        (station_rows, browse.has_more)
    };

    // Above the write, for the reason every tabbed page writes its count above its signature
    // guard: a count that arrives late strands the empty state, and `Property::set` is
    // value-compared so writing it unconditionally costs nothing.
    g.set_browse_count(len_as_i32(station_rows.len()));
    g.set_browse_has_more(has_more);

    let grid = chunk_built_rows(station_rows, g.get_browse_columns(), |stations| {
        RadioStationGridRow { stations }
    });
    write_grid(&g.get_browse_rows(), grid, "radio::browse");
}

/// Change what Browse is asking for and fetch the first page of the answer.
///
/// A no-op where the edit leaves the query where it was, which is what makes a filter reseat and
/// a chip re-picked to its current value free.
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

/// Load the first page if nothing has been loaded for the current query.
///
/// The section enter's call: an entry that already has stations paints them and asks for nothing.
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

/// Decode the retained page's logos again after a section leave dropped them.
///
/// The counterpart to [`ensure_loaded`], and the two are each other's guard: a re-entry either has
/// rows to warm or has none and fetches, so calling both is what covers an enter without either
/// having to know which case it is. Through the same pass a landed page takes, which is why it
/// costs no traffic — every URL it asks about is already answered in the session memo.
pub fn rewarm(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    if radio_ui.browse.lock().is_empty() {
        return;
    }
    let (s, ru, weak) = (state.clone(), radio_ui.clone(), ui.as_weak());
    state.runtime.spawn(async move {
        warm_page(&s, &ru, &weak).await;
    });
}

/// Ask the directory for a page, then paint it, its logos and their decodes.
///
/// Called from the UI thread, which is what lets the loading flag go up before the spawn: the
/// pill has to be disabled on the frame the click lands, not on the frame the request returns.
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
                log::warn!("radio: directory search failed: {}", crate::services::describe(&e));
                let current = ru.browse.lock().fail(generation, append);
                if current {
                    let weak_failed = weak.clone();
                    let _ = weak_failed.upgrade_in_event_loop(|ui| {
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

        paint(&weak, &ru);
        warm_page(&s, &ru, &weak).await;
    });
}

/// Push the cache onto the grid and lower the spinner.
fn paint(weak: &Weak<AppWindow>, radio_ui: &Arc<RadioUi>) {
    let ru = radio_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.global::<Radio>().set_browse_loading(false);
        apply(&ui, &ru);
    });
}

/// Fetch the page's missing logos, decode a screenful, and repaint whatever that moved.
///
/// **After the grid is already on screen**, deliberately: the search was one network round trip
/// and the logos are a second, so serialising them would put both in front of first paint.
async fn warm_page(state: &AppState, radio_ui: &Arc<RadioUi>, weak: &Weak<AppWindow>) {
    let favicon_urls: Vec<String> = {
        let browse = radio_ui.browse.lock();
        browse.stations.iter().filter_map(|station| station.favicon_url.clone()).collect()
    };
    let new_logos =
        logos::fetch_missing(state, &radio_ui.logos, favicon_urls.iter().map(String::as_str)).await;

    let artwork_paths: Vec<String> = {
        let browse = radio_ui.browse.lock();
        browse
            .stations
            .iter()
            .filter_map(|station| station.favicon_url.as_deref())
            .filter_map(|url| radio_ui.logos.path_for(url))
            .collect()
    };

    // `Some(())` only once the decode reports the tier is still its own: a leave landing inside
    // the burst hands the buffers back, and announcing anyway would bump the generation over an
    // emptied tier. A `JoinError` is the same "we don't know".
    let ru_warm = radio_ui.clone();
    let warmed = tokio::task::spawn_blocking(move || covers::prewarm(&ru_warm, &artwork_paths))
        .await
        .unwrap_or(false);

    let ru = radio_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if new_logos {
            apply(&ui, &ru);
        }
        // The same re-check the prewarm made, against anything that landed after it returned.
        if warmed && ru.section_active() {
            covers::announce_warm(&ui);
        }
    });
}
