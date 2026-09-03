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
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use slint::{ComponentHandle, Weak};

use crate::ui::grid_rows::{chunk_built_rows, write_grid};
use crate::ui::util::len_as_i32;
use crate::{AppWindow, Radio, RadioStationGridRow};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::radio::{DirectoryStation, StationPage, StationSearch};

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

/// The whole query, for the scope suggestions — they read the needle *and* every chip already set,
/// a scope the page is filtered by not being worth offering back. Cloned rather than borrowed
/// because the caller holds the UI thread and must not still be under this lock when it takes the
/// facet index.
pub fn query(radio_ui: &RadioUi) -> StationSearch {
    radio_ui.browse.lock().search.clone()
}

/// The cached station behind a row, and whatever logo this session found for it.
///
/// A row identifies a browsed station by its uuid rather than by an index: the model is chunked
/// and rebuilt whenever anything moves, so a position is only true for the frame it was read on.
pub fn resolve(radio_ui: &RadioUi, uuid: &str) -> Option<(DirectoryStation, Option<String>)> {
    let browse = radio_ui.browse.lock();
    let station = browse.stations.iter().find(|station| station.station_uuid == uuid)?.clone();
    let logo = logo_for(radio_ui, &station);
    Some((station, logo))
}

/// The logo a browsed station draws, from the three places this install can know one.
///
/// The `favicon_url` answer comes first because a *moved* logo is exactly what the URL-keyed memo
/// is for. **The row is the better answer wherever the two differ**, and they differ whenever the
/// logo did not come from `favicon_url` at all — a station whose favicon 404s and whose logo was
/// found on its own site has a row and can have no memo entry, and one inside a backoff has a memo
/// entry of `None`. Either way the two local tabs paint it and Browse was painting a monogram.
/// Last is what a narrow search discovered on the site of a station that has no row yet.
fn logo_for(radio_ui: &RadioUi, station: &DirectoryStation) -> Option<String> {
    station
        .favicon_url
        .as_deref()
        .and_then(|url| radio_ui.logos.path_for(url))
        .or_else(|| radio_ui.known_logos.lock().get(&station.station_uuid).cloned())
        .or_else(|| site_logo(radio_ui, station))
}

/// What this session read off the station's own site, for a row the directory gave no usable
/// favicon. Keyed on the origin, the same spelling [`logos::discover_missing`] records under.
fn site_logo(radio_ui: &RadioUi, station: &DirectoryStation) -> Option<String> {
    let homepage = station.homepage.as_deref().unwrap_or_default();
    let origin = library::radio::site_origin(homepage, &station.stream_url)?;
    radio_ui.logos.path_for(origin.as_str())
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
        let starred = radio_ui.starred.lock();
        let station_rows: Vec<_> = browse
            .stations
            .iter()
            .map(|station| {
                let logo = logo_for(radio_ui, station);
                rows::to_slint_radio_station_row(
                    station,
                    starred.contains(&station.station_uuid),
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

    // **Onto the mounted cards where the page is the same stations in the same places**, which is
    // most repaints: a landed logo, a star flipped on another tab, a play that only stamped a
    // row. `rows::patch_grid` says why the reset is worth dodging.
    let columns = g.get_grid_columns();
    if !rows::patch_grid(&g.get_browse_rows(), &station_rows, columns) {
        let grid =
            chunk_built_rows(station_rows, columns, |stations| RadioStationGridRow { stations });
        write_grid(&g.get_browse_rows(), grid, "radio::browse");
    }

    // The station page draws the same station some card here does, off the same cache, so it is
    // re-stamped from the write path rather than from each thing that could have moved a field.
    super::detail::restamp(ui, radio_ui);
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
    let generation = {
        let browse = radio_ui.browse.lock();
        if browse.is_empty() {
            return;
        }
        browse.generation
    };
    let (s, ru, weak) = (state.clone(), radio_ui.clone(), ui.as_weak());
    state.runtime.spawn(async move {
        // Not fresh: these are the same stations a second time, so the harder effort a narrow
        // query earns would spend a request per re-entry on an answer already in the memo.
        warm_page(&s, &ru, &weak, generation, false).await;
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

/// Push the cache onto the grid and lower the spinner.
///
/// `fresh` tells a new query from a page appended onto one, which only the scope adoption below
/// cares about: an append that comes back empty is the end of the results, not a search that found
/// nothing.
fn paint(state: &AppState, weak: &Weak<AppWindow>, radio_ui: &Arc<RadioUi>, fresh: bool) {
    let (s, ru) = (state.clone(), radio_ui.clone());
    let _ = weak.upgrade_in_event_loop(move |ui| {
        ui.global::<Radio>().set_browse_loading(false);
        apply(&ui, &ru);
        if fresh {
            super::suggest::adopt_only_scope(&ui, &s, &ru);
        }
    });
}

/// Stamp the logos that have landed so far onto the page, and announce the tier with them.
///
/// This fires every `LOGO_REPAINT_INTERVAL` for as long as a page takes to fill, which is exactly
/// the moment somebody is clicking a station they just searched for — so it leans on [`apply`]
/// patching rather than resetting. A landed logo is not a fetch, so the station set cannot have
/// moved and the patch always has the shape it needs.
///
/// **The announce matters as much as the paint.** At generation `0` a card asks the tier
/// cache-only and queues no decode, which is what a released tier wants and the opposite of what a
/// page still filling in does — the path just written into the row would never be decoded at all.
fn paint_landed(weak: &Weak<AppWindow>, radio_ui: &Arc<RadioUi>) {
    let ru = radio_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        apply(&ui, &ru);
        if ru.section_active() {
            covers::announce_warm(&ui);
        }
    });
}

/// Ask the sites of whatever the page still has no logo for.
///
/// Runs after the favicon burst, mirroring the order [`library::radio::heal_station_logo`] takes
/// for a kept station: the field the directory carries first, the station's own site second. A
/// third of the directory carries no logo field at all, which is why this is reached only where
/// the result is narrow enough to be the station the user typed.
async fn discover_page(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    is_current: &impl Fn() -> bool,
    on_landed: &impl Fn(),
) -> bool {
    let sites: Vec<(String, String)> = {
        let browse = radio_ui.browse.lock();
        browse
            .stations
            .iter()
            .filter(|station| logo_for(radio_ui, station).is_none())
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

/// How often a filling page repaints while its logos land.
///
/// [`apply`] is a whole-model write, so one per station would spend more on rebuilds than the fill
/// is worth. Long enough to coalesce a burst, short enough that the grid reads as filling in
/// rather than as arriving in steps.
const LOGO_REPAINT_INTERVAL: Duration = Duration::from_millis(150);

/// Fetch the page's missing logos, decode a screenful, and repaint as they land.
///
/// **After the grid is already on screen**, deliberately: the search was one network round trip
/// and the logos are a second, so serialising them would put both in front of first paint.
///
/// `generation` is the page these logos belong to. A search that supersedes it stops the burst
/// where it stands rather than paying for a page nobody is looking at any more. `fresh` is whether
/// this page is a new query rather than a page appended or a tier re-warmed — see
/// [`logos::Effort::for_result`].
async fn warm_page(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: &Weak<AppWindow>,
    generation: u64,
    fresh: bool,
) {
    // A station whose row already holds a logo is not asked about again, which is a page of
    // requests saved wherever the user has kept much of what they browse. **The trade is a logo
    // that moved**: the URL-keyed memo is what would notice, and it is never asked. Worth it
    // because the row is only written by a keep or a play, so a moved logo could not have
    // reached it either way — nothing here regresses, it just doesn't improve.
    let (favicon_urls, effort) = {
        let browse = radio_ui.browse.lock();
        let known = radio_ui.known_logos.lock();
        let urls: Vec<String> = browse
            .stations
            .iter()
            .filter(|station| !known.contains_key(&station.station_uuid))
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
        browse.stations.iter().filter_map(|station| logo_for(radio_ui, station)).collect()
    };

    let ru = radio_ui.clone();
    covers::warm_and_announce(radio_ui, weak, artwork_paths, move |ui| {
        if new_logos {
            apply(ui, &ru);
        }
    })
    .await;
}
