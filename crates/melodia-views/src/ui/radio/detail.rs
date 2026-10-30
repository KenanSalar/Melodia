//! The station page: open, close, the hero it morphs the band into, and the directory refresh
//! behind it.
//!
//! **A station is opened from a row, not fetched by id**: a browsed one has no database row
//! (`id == 0`), so [`StationRef`] carries both halves and `Radio.detail-tab` is the open flag.
//!
//! **Each tab holds its own page.** The seat ([`DetailState`]) is per tab, but `Radio.detail-*`,
//! `HeroBackdrop` and `HeroChips` are one set, so only one page is painted and [`reseat`] keeps it
//! in step with the mounted tab.
//!
//! **The refresh is additive**: [`refresh_from_directory`] fills only what the table has no column
//! for (votes, state, last check), so it can't undo a `local_*` override.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::ui::detail_artwork::{DetailPair, decode_detail_pair};
use crate::ui::detail_view::apply_detail_artwork;
use crate::ui::hero_chips::{self, StationFacts};
use crate::ui::track_list_view::view_id;
use crate::ui::util::clamp_i64_to_i32;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::radio::{DirectoryStation, RadioStation};
use melodia_core::error::{AppError, AppResult, describe};
use melodia_ui::{AppWindow, NavEnterFrom, Radio, RadioStationRow};

use super::tabs::{RadioTab, mounted_tab, section_is_up};
use super::{RadioUi, browse, kept, rows};

/// What identifies a station across the fetch that opens its page: a browsed station has no `id`
/// and a hand-typed one has no `uuid`, so it takes both.
///
/// The boot restore and the Mouse-4/5 replay arrive with the id alone; [`open_station_with`]
/// completes the uuid off whatever resolved, ahead of every reader of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationRef {
    pub id: i64,
    pub uuid: String,
}

impl StationRef {
    pub fn from_row(row: &RadioStationRow) -> Self {
        Self { id: i64::from(row.id), uuid: row.uuid.to_string() }
    }

    /// Whether this station has a database row, which decides where it resolves from and whether
    /// `views.json` can name it for the next launch.
    pub fn is_kept(&self) -> bool {
        rows::station_has_row(self.id)
    }
}

/// A station page, and everything repainting it needs. Holds the resolved source because a tab
/// move repaints from here while the cache it came from can be refilled underneath.
#[derive(Clone)]
struct OpenStation {
    station: StationRef,
    source: StationSource,
    facts: StationFacts,
    /// The directory's last reachability verdict; `None` is "nobody has said".
    check: Option<bool>,
    votes: i32,
    votable: bool,
    /// The decoded hero, so a tab move repaints in its own tick. Refcounted clones of what
    /// `DetailArtwork` holds, so a seat costs a pair of its own only once the tier evicts it.
    artwork: DetailPair,
}

/// One station page per tab, a tab move not evicting another tab's page. Named fields rather than
/// an array, so no Rust file restates the Slint `tab-*` indices.
///
/// No needle: the band's box is hidden while a page is open, a station having no songs to filter.
#[derive(Default)]
pub struct DetailState {
    browse: Option<OpenStation>,
    favorites: Option<OpenStation>,
    recent: Option<OpenStation>,
    /// What `views.json` names, so a tab move that changes nothing writes nothing.
    persisted: Option<i64>,
}

impl DetailState {
    fn seat(&self, tab: RadioTab) -> Option<&OpenStation> {
        match tab {
            RadioTab::Browse => self.browse.as_ref(),
            RadioTab::Favorites => self.favorites.as_ref(),
            RadioTab::Recent => self.recent.as_ref(),
        }
    }

    fn seat_mut(&mut self, tab: RadioTab) -> &mut Option<OpenStation> {
        match tab {
            RadioTab::Browse => &mut self.browse,
            RadioTab::Favorites => &mut self.favorites,
            RadioTab::Recent => &mut self.recent,
        }
    }

    fn any_seated(&self) -> bool {
        RadioTab::ALL.into_iter().any(|tab| self.seat(tab).is_some())
    }
}

/// `Radio.detail-votes` before the directory has answered; `0` is a real count.
const VOTES_UNKNOWN: i32 = -1;

/// `Radio.detail-tab` with no station page on the mounted tab.
pub const NO_SEAT: i32 = -1;

/// The station a page was opened for, from whichever side had it: two types because they know
/// different things, which [`DirectoryStation`] argues.
#[derive(Clone)]
enum StationSource {
    Kept(RadioStation),
    /// The directory's answer, and whatever logo this session found for it.
    Browsed(DirectoryStation, Option<String>),
}

impl StationSource {
    /// The directory's own id, empty for a hand-typed station, which has no entry to vote on or
    /// refresh against.
    fn uuid(&self) -> &str {
        match self {
            Self::Kept(station) => station.station_uuid.as_deref().unwrap_or_default(),
            Self::Browsed(station, _) => &station.station_uuid,
        }
    }

    fn stream_url(&self) -> &str {
        match self {
            Self::Kept(station) => &station.stream_url,
            Self::Browsed(station, _) => &station.stream_url,
        }
    }

    fn artwork_path(&self) -> Option<String> {
        match self {
            Self::Kept(station) => station.artwork_path.clone(),
            Self::Browsed(_, logo) => logo.clone(),
        }
    }

    /// What the band's chip strip states: where the station is and what it plays. Codec, bitrate
    /// and votes are rows on the page and the country is the band's subtitle. A kept station has
    /// no `state` until [`refresh_from_directory`] fills it.
    fn facts(&self) -> StationFacts {
        match self {
            Self::Kept(station) => StationFacts {
                tags: rows::split_tags(station.genre().unwrap_or_default()),
                state: String::new(),
                language: station.language.clone(),
            },
            Self::Browsed(station, _) => StationFacts {
                tags: rows::split_tags(&station.tags),
                state: station.state.clone(),
                language: station.language.clone(),
            },
        }
    }

    /// The vote count a directory answer already carries, so the row is right on the first frame.
    fn votes(&self) -> i32 {
        match self {
            Self::Kept(_) => VOTES_UNKNOWN,
            Self::Browsed(station, _) => clamp_i64_to_i32(station.votes),
        }
    }

    /// UI thread only — the browsed arm reads `favorite_uuids`, which the grid is built from.
    fn to_row(&self, radio_ui: &RadioUi) -> RadioStationRow {
        match self {
            Self::Kept(station) => rows::to_slint_kept_station_row(station),
            Self::Browsed(station, logo) => {
                // The logo the page was seated with, not a fresh resolve. `favorite_uuids` is
                // locked before the answers, `browse::apply` nesting the two the other way round.
                let is_favorite = radio_ui.favorite_uuids.lock().contains(&station.station_uuid);
                let kept = radio_ui.kept_answers.lock();
                let local = rows::LocalAnswers {
                    is_favorite,
                    logo: logo.clone(),
                    format: browse::format_for(kept.get(&station.station_uuid)),
                };
                rows::to_slint_radio_station_row(station, &local)
            }
        }
    }
}

pub async fn open_station(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: Weak<AppWindow>,
    station: StationRef,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_station_with(state, radio_ui, weak, station, enter_from, |_ui| {}).await
}

/// Opens a station's page, running `on_applied` in the **same** UI-thread closure that raises
/// `detail-open`. A Mouse-4/5 replay moves the tab there: the body router is a pure function of
/// `(tab-idx, detail-open)`, so a nav written up front mounts the destination's grid for the
/// length of the fetch.
pub async fn open_station_with<F>(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: Weak<AppWindow>,
    station: StationRef,
    enter_from: NavEnterFrom,
    on_applied: F,
) -> AppResult<()>
where
    F: FnOnce(&AppWindow) + Send + 'static,
{
    let source = resolve(state, radio_ui, &station).await?;
    // Two of the three ways in know only the id; an empty uuid would cost the vote pill, the
    // refresh and the seat equality a vote is found by.
    let station = StationRef { id: station.id, uuid: source.uuid().to_owned() };
    let facts = source.facts();
    let votes = source.votes();
    let pair =
        decode_detail_pair(state, radio_ui.detail_artwork.clone(), source.artwork_path()).await;
    let votable = state.radio_enabled.get() && !station.uuid.is_empty();

    let landed_state = state.clone();
    let paint_ui = radio_ui.clone();
    let seated = OpenStation {
        station: station.clone(),
        source,
        facts,
        // A stale verdict under a new station is worse than none.
        check: None,
        votes,
        votable,
        artwork: pair,
    };
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let radio_ui = paint_ui;
        let g = ui.global::<Radio>();
        crate::ui::nav_transition::mark(&ui, enter_from);
        on_applied(&ui);
        // After the hook, which moves the tab on a history replay; read before it, the seat
        // and `paint_seat`'s section shadow answer for the tab being left.
        let tab = mounted_tab(&g);
        paint_seat(&ui, &g, &radio_ui, &seated, true);
        *radio_ui.detail.lock().seat_mut(tab) = Some(seated);
        // Reads the seat, so it comes after the store.
        sync_history_seat(&ui, &radio_ui);
        // **The seat property last** — everything above is what it gates.
        g.set_detail_tab(g.get_tab_idx());

        // Here rather than at the click, so the replay and the boot restore persist too, and
        // because only here is the landing tab known.
        persist_seat(&landed_state, &ui, &radio_ui);
        crate::ui::nav_history::record_current(&ui);
        // Last: the box has just hidden, and a re-open of the same station fires no mirror.
        g.invoke_detail_scope_changed();
    });

    // Spawned, not awaited: it only adds chips, and a slow directory would otherwise hold the
    // boot restore, and `Radio.restoring` with it, for a whole request timeout.
    let (refresh_state, refresh_ui) = (state.clone(), radio_ui.clone());
    state.runtime.spawn(async move {
        refresh_from_directory(&refresh_state, &refresh_ui, &weak, &station).await;
    });
    Ok(())
}

/// Writes a seat into the properties the page and the band paint from: the one writer for an
/// open, a tab hand-off and a re-enter's re-decode, so the page can't describe one station while
/// the hero paints another. `animate` is false only for the re-decode.
///
/// Reads `section_is_up` live, so the caller's navigation must have landed first.
fn paint_seat(
    ui: &AppWindow,
    g: &Radio<'_>,
    radio_ui: &RadioUi,
    open: &OpenStation,
    animate: bool,
) {
    // Built here, not carried across an await: it holds `SharedString`s and reads `favorite_uuids`.
    g.set_detail_station(open.source.to_row(radio_ui));
    g.set_detail_stream_url(open.source.stream_url().into());
    g.set_detail_votable(open.votable);
    g.set_detail_check_known(open.check.is_some());
    g.set_detail_check_ok(open.check.unwrap_or_default());
    g.set_detail_votes(open.votes);
    g.set_detail_logo_size(logo_size(&open.artwork));

    let on_screen = section_is_up(ui);
    hero_chips::publish_station(ui, &open.facts, on_screen);
    apply_detail_artwork(ui, g, open.artwork.clone(), animate, on_screen);
}

/// Points the painted station page at whatever `tab` holds, or gives the seat up.
///
/// Synchronous, on the click's own tick: `Radio.tab-idx` has already moved, so a deferral would
/// let the view's `changed` trackers read a `detail-open` that is neither tab's answer. With no
/// seat it writes [`NO_SEAT`] alone, so the band paints the departing station through its
/// collapse and `hero-collapsed` hands the images back.
pub fn reseat(ui: &AppWindow, radio_ui: &RadioUi, tab: RadioTab) {
    let g = ui.global::<Radio>();
    let Some(open) = radio_ui.detail.lock().seat(tab).cloned() else {
        g.set_detail_tab(NO_SEAT);
        return;
    };
    paint_seat(ui, &g, radio_ui, &open, true);
    sync_history_seat(ui, radio_ui);
    g.set_detail_tab(g.get_tab_idx());
}

/// The station behind a [`StationRef`]. The caches come first, holding what the database can't
/// (a session-found logo, a browsed answer); the database answers the boot restore.
async fn resolve(
    state: &AppState,
    radio_ui: &RadioUi,
    station: &StationRef,
) -> AppResult<StationSource> {
    if station.is_kept() {
        if let Some(kept) = kept_from_cache(radio_ui, station.id) {
            return Ok(StationSource::Kept(kept));
        }
        return Ok(StationSource::Kept(library::radio::get_station(&state.db, station.id).await?));
    }
    browse::resolve(radio_ui, &station.uuid)
        .map(|(found, logo)| StationSource::Browsed(found, logo))
        .ok_or_else(|| AppError::not_found_msg("Station is no longer in the browse results"))
}

/// A kept station from either local tab's cache, a row being in one or both.
fn kept_from_cache(radio_ui: &RadioUi, id: i64) -> Option<RadioStation> {
    kept::resolve(radio_ui, RadioTab::Favorites, id)
        .or_else(|| kept::resolve(radio_ui, RadioTab::Recent, id))
}

/// Asks the directory about the open station and folds in the facts no local row carries.
///
/// Quiet on every failure, the switch included: the page is already drawn, so a miss is fewer
/// chips rather than an error. The answer lands in whichever seat holds the station and reaches
/// the properties only where that seat is mounted.
pub(super) async fn refresh_from_directory(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: &Weak<AppWindow>,
    station: &StationRef,
) {
    if station.uuid.is_empty() {
        return;
    }
    let found = match library::radio::station_details(state, &station.uuid).await {
        Ok(Some(found)) => found,
        Ok(None) => return,
        Err(e) => {
            log::debug!("radio: station details not read: {}", melodia_core::error::describe(&e));
            return;
        }
    };

    let radio_ui = radio_ui.clone();
    let station = station.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let votes = clamp_i64_to_i32(found.votes);
        let Some((seated, facts)) = ({
            let mut detail = radio_ui.detail.lock();
            // The page may have closed or moved tabs while this was in flight.
            RadioTab::ALL
                .into_iter()
                .find(|tab| detail.seat(*tab).is_some_and(|open| open.station == station))
                .and_then(|tab| {
                    let open = detail.seat_mut(tab).as_mut()?;
                    open.facts.state.clone_from(&found.state);
                    open.check = Some(found.last_check_ok);
                    open.votes = votes;
                    Some((tab, open.facts.clone()))
                })
        }) else {
            return;
        };

        let g = ui.global::<Radio>();
        if mounted_tab(&g) != seated {
            return;
        }
        g.set_detail_check_known(true);
        g.set_detail_check_ok(found.last_check_ok);
        g.set_detail_votes(votes);
        hero_chips::publish_station(&ui, &facts, section_is_up(&ui));
    });
}

/// Drops the mounted tab's page from the Rust mirror; the other tabs' pages aren't the back
/// arrow's to lose. The Slint half is the closing callback's, the hero's images `hero-collapsed`'s.
pub fn close_detail(ui: &AppWindow, radio_ui: &RadioUi) {
    let tab = mounted_tab(&ui.global::<Radio>());
    radio_ui.detail.lock().seat_mut(tab).take();
}

/// Gives up every seat, for the Radio switch going off, which takes the page away.
pub fn forget_all_seats(radio_ui: &RadioUi) {
    *radio_ui.detail.lock() = DetailState::default();
}

pub fn any_seated(radio_ui: &RadioUi) -> bool {
    radio_ui.detail.lock().any_seated()
}

/// Hands every seat's decoded hero back on the section leave, keeping the seats;
/// [`rewarm_hero`] rebuilds them on the enter.
pub fn release_seated_artwork(radio_ui: &RadioUi) {
    let mut detail = radio_ui.detail.lock();
    for tab in RadioTab::ALL {
        if let Some(open) = detail.seat_mut(tab).as_mut() {
            open.artwork = DetailPair::default();
        }
    }
}

/// Names the mounted tab's station in `views.json`, or clears the key where it names none.
///
/// Only a kept station can be named, a browsed one being a directory answer with a shelf life.
/// Called on an open, a close and every tab move, the station the restored tab shows being what a
/// boot can act on. The shadow keeps a tab bounce off the disk, and holding `persist_writer` with
/// the shadow reloaded under it drops a superseded write, the ordering the pool doesn't give.
pub fn persist_seat(state: &AppState, ui: &AppWindow, radio_ui: &Arc<RadioUi>) {
    let tab = mounted_tab(&ui.global::<Radio>());
    let named = {
        let mut detail = radio_ui.detail.lock();
        let named =
            detail.seat(tab).and_then(|open| open.station.is_kept().then_some(open.station.id));
        if detail.persisted == named {
            return;
        }
        detail.persisted = named;
        named
    };
    let state = state.clone();
    let radio_ui = radio_ui.clone();
    state.runtime.clone().spawn_blocking(move || {
        let _writer = radio_ui.persist_writer.lock();
        let latest = radio_ui.detail.lock().persisted;
        if latest != named {
            return;
        }
        if let Err(e) =
            library::settings::set_last_detail_id(&state.paths, view_id::RADIO_DETAIL, named)
        {
            log::warn!("radio: persist station detail: {}", describe(&e));
        }
    });
}

/// Every seated tab and its station, as a snapshot: both callers lock caches per seat, which
/// aren't ours to order against `detail`.
fn seated_stations(radio_ui: &RadioUi) -> Vec<(RadioTab, StationRef)> {
    let detail = radio_ui.detail.lock();
    RadioTab::ALL
        .into_iter()
        .filter_map(|tab| detail.seat(tab).map(|open| (tab, open.station.clone())))
        .collect()
}

/// Re-reads every seated station from its cache and rewrites the row the page paints, at the tail
/// of `browse::apply` and `kept::apply`, so the page and the grid can't disagree.
///
/// A miss is left alone: at boot or on a column change the cache may not be filled yet, and
/// whether a station is gone is [`close_if_gone`]'s question. Every seat is stamped, a tab move
/// repainting from the seat's own source, but only the mounted row reaches Slint.
pub fn restamp(ui: &AppWindow, radio_ui: &RadioUi) {
    let g = ui.global::<Radio>();
    let mounted = mounted_tab(&g);

    for (tab, station) in seated_stations(radio_ui) {
        let Some(source) = from_cache(radio_ui, &station) else {
            continue;
        };
        // Before the seat takes the source: `to_row` locks `favorite_uuids`.
        let row = (tab == mounted).then(|| source.to_row(radio_ui));
        if let Some(open) = radio_ui.detail.lock().seat_mut(tab).as_mut() {
            open.source = source;
        }
        if let Some(row) = row {
            g.set_detail_station(row);
        }
    }
}

/// Closes a kept station's page on any tab once its row is gone.
///
/// Called only from `kept::refresh`'s landing, which has just rebuilt both caches, so a miss there
/// is authoritative. Un-favoriting an unplayed station deletes its row, as a removal can, and a
/// page left open would offer Play and Remove against a dead id.
pub fn close_if_gone(ui: &AppWindow, radio_ui: &RadioUi) {
    let g = ui.global::<Radio>();
    let mounted = mounted_tab(&g);
    for (tab, station) in seated_stations(radio_ui) {
        // A browsed page is backed by the directory answer, not these caches.
        if !station.is_kept() || kept_from_cache(radio_ui, station.id).is_some() {
            continue;
        }
        if tab == mounted {
            g.invoke_close_detail();
        } else {
            radio_ui.detail.lock().seat_mut(tab).take();
        }
    }
}

/// The open station as whichever cache holds it describes it now.
fn from_cache(radio_ui: &RadioUi, station: &StationRef) -> Option<StationSource> {
    if station.is_kept() {
        return kept_from_cache(radio_ui, station.id).map(StationSource::Kept);
    }
    browse::resolve(radio_ui, &station.uuid)
        .map(|(found, logo)| StationSource::Browsed(found, logo))
}

/// Re-answers whether the mounted tab's page is the station `Radio.history-rows` describes; the
/// rows are `history::apply`'s, so a seat change moves only this flag.
pub fn sync_history_seat(ui: &AppWindow, radio_ui: &RadioUi) {
    let g = ui.global::<Radio>();
    let seated_url = radio_ui
        .detail
        .lock()
        .seat(mounted_tab(&g))
        .map(|open| open.source.stream_url().to_owned());
    let is_playing = seated_url.is_some_and(|url| radio_ui.history.lock().describes(&url));
    g.set_detail_station_is_playing(is_playing);
}

/// Re-decodes every seated tab's hero on a section enter, the leave having handed the images back.
///
/// Every seat, since a tab move repaints from the seat's held pair and a cold one would flash its
/// monogram; the mounted tab first, the rest waiting on a tab move that hasn't happened.
pub fn rewarm_hero(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let weak = ui.as_weak();
    let mounted = mounted_tab(&ui.global::<Radio>());
    let seats: Vec<(RadioTab, Option<String>)> = {
        let detail = radio_ui.detail.lock();
        std::iter::once(mounted)
            .chain(RadioTab::ALL.into_iter().filter(|tab| *tab != mounted))
            .filter_map(|tab| detail.seat(tab).map(|open| (tab, open.source.artwork_path())))
            .collect()
    };
    if seats.is_empty() {
        return;
    }

    let (state, radio_ui) = (state.clone(), radio_ui.clone());
    state.runtime.clone().spawn(async move {
        for (tab, artwork_path) in seats {
            let pair =
                decode_detail_pair(&state, radio_ui.detail_artwork.clone(), artwork_path).await;
            let radio_ui = radio_ui.clone();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                {
                    let mut detail = radio_ui.detail.lock();
                    let Some(open) = detail.seat_mut(tab).as_mut() else {
                        return;
                    };
                    open.artwork = pair;
                }
                let g = ui.global::<Radio>();
                if mounted_tab(&g) != tab {
                    return;
                }
                // Cloned out: `paint_seat` locks `favorite_uuids`.
                let Some(open) = radio_ui.detail.lock().seat(tab).cloned() else {
                    return;
                };
                // A re-decode of what was already painted, not a hand-off.
                paint_seat(&ui, &g, &radio_ui, &open, false);
            });
        }
    });
}

/// Reopens the station `views.json` was left naming; only a kept one can be, so a browsed page
/// lands on the tab root after a restart.
pub fn seed_detail_from_settings(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let named = library::settings::get_view_state(&state.paths)
        .ok()
        .and_then(|vs| vs.last_detail_ids.get(view_id::RADIO_DETAIL).copied());
    // Before the liveness filter: a dead id is exactly what the next `persist_seat` must clear.
    radio_ui.detail.lock().persisted = named;

    let Some(id) = named.filter(|id| rows::station_has_row(*id)) else {
        return;
    };

    // Ahead of `app.show()`: the tab bodies gate on it, else a restore paints a grid first.
    ui.global::<Radio>().set_restoring(true);

    let (state, radio_ui, weak) = (state.clone(), radio_ui.clone(), ui.as_weak());
    state.runtime.clone().spawn(async move {
        // Id-only: `open_station_with` completes the uuid off the row it resolves.
        let station = StationRef { id, uuid: String::new() };
        if let Err(e) =
            open_station(&state, &radio_ui, weak.clone(), station, NavEnterFrom::Above).await
        {
            log::warn!("radio: restore station detail: {}", melodia_core::error::describe(&e));
        }
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<Radio>().set_restoring(false);
        });
    });
}

/// The decoded logo's smallest side, which the hero compares its tile against; zero, where
/// nothing decoded, draws the monogram.
fn logo_size(pair: &DetailPair) -> i32 {
    pair.cover
        .as_ref()
        .map_or(0, |cover| clamp_i64_to_i32(i64::from(cover.width().min(cover.height()))))
}

#[cfg(test)]
#[path = "tests/detail_tests.rs"]
mod tests;
