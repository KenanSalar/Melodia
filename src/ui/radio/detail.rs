//! The station page: open, close, the hero it morphs the band into, and the directory refresh
//! behind it.
//!
//! **A station is opened from a row, not fetched by id.** A browsed station has no database row —
//! `RadioStationRow.id` is `0` for one — so the id cannot be the handle, and it cannot be the
//! open/closed flag either. [`StationRef`] carries both halves of what identifies either kind, and
//! `Radio.detail-tab` is the flag.
//!
//! **A page belongs to the tab it was opened from, and all three tabs may hold one.** What is per
//! tab is the *seat* ([`DetailState`]) rather than the properties: `Radio.detail-*`, `HeroBackdrop`
//! and `HeroChips` are one set between six heroes, so only one page is ever painted. [`reseat`] is
//! what keeps the painted one and the mounted tab in step, and it is `super::filter::sync_box`'s
//! move one level up — the state belongs to the tab and the surface follows the mount.
//!
//! **The refresh is additive.** What a kept row knows and what the directory knows overlap but do
//! not agree: the table has no column for the popularity figures, the state or the directory's own
//! last check, and the four `local_*` columns are the user's answers to what the directory left
//! blank. So [`refresh_from_directory`] fills the first set and touches nothing else — letting it
//! rewrite the rest would undo an override from a fetch nobody asked for.

use std::sync::Arc;

use slint::{ComponentHandle, Weak};

use crate::entities::radio::{DirectoryStation, RadioStation};
use crate::error::{AppError, AppResult};
use crate::library;
use crate::state::AppState;
use crate::ui::detail_artwork::{DetailPair, decode_detail_pair};
use crate::ui::detail_view::impl_detail_view_helpers;
use crate::ui::hero_chips::{self, StationFacts};
use crate::ui::track_list_view::view_id;
use crate::ui::util::clamp_i64_to_i32;
use crate::{AppWindow, NavEnterFrom, Radio, RadioStationRow};

use super::tabs::{RadioTab, section_is_up, tab_from_index};
use super::{RadioUi, browse, kept, rows};

// `apply_detail_artwork` — the cover and hero-blur write. `artwork_only` because this detail's
// list is bare titles rather than a `TrackList`, so there is no `tracks` model to swap.
impl_detail_view_helpers!(artwork_only Radio);

/// What identifies a station across the fetch that opens its page.
///
/// Both halves, because neither is enough on its own: a browsed station has no `id` and a
/// hand-typed one has no `uuid`. `id == 0` is the split every other station call site already
/// spells, and it decides which cache answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationRef {
    pub id: i64,
    pub uuid: String,
}

impl StationRef {
    pub fn from_row(row: &RadioStationRow) -> Self {
        Self {
            id: i64::from(row.id),
            uuid: row.uuid.to_string(),
        }
    }

    /// Whether this station has a database row, which decides where it resolves from and whether
    /// `views.json` can name it for the next launch.
    pub fn is_kept(&self) -> bool {
        rows::station_has_row(self.id)
    }
}

/// A station page, and everything repainting it needs without asking anything again.
///
/// It holds the resolved [`StationSource`] rather than re-reading it, because a tab move repaints
/// from here and the cache it came out of can be refilled underneath.
#[derive(Clone)]
struct OpenStation {
    station: StationRef,
    source: StationSource,
    facts: StationFacts,
    /// The directory's own last reachability verdict. `None` is "nobody has said", which for a
    /// station it no longer lists is forever, and only a real verdict is worth printing.
    check: Option<bool>,
    votes: i32,
    votable: bool,
    /// The decoded hero, held so a tab move repaints in its own tick rather than a decode later.
    /// Refcounted clones of what `DetailArtwork` already holds, so a seat costs a
    /// `(cover, blur)` pair of its own only once the tier has evicted it.
    artwork: DetailPair,
}

/// One station page per tab, held beside the three tab caches.
///
/// **A seat per tab, `kept::cache`'s shape**: a station opens from all three and a tab move must
/// not evict what another tab is holding. Named fields rather than an array, `super::tabs` being
/// deliberate about no Rust file restating the Slint `tab-*` indices.
///
/// No needle: the band's box is hidden while a station page is open, a station having no songs of
/// its own to filter, so the page is the one surface here that is never filtered.
#[derive(Default)]
pub struct DetailState {
    browse: Option<OpenStation>,
    favorites: Option<OpenStation>,
    recent: Option<OpenStation>,
    /// What `views.json` currently names, so a tab move that changes nothing writes nothing.
    /// Seeded from the file by [`seed_detail_from_settings`], **before** its own liveness filter:
    /// a persisted id whose row has since gone is exactly what the next write has to clear.
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

    /// Whether any tab is holding a page, which is what "the page has a hero to hand back" means
    /// once the mounted one has been dealt with.
    fn any_seated(&self) -> bool {
        RadioTab::ALL.into_iter().any(|tab| self.seat(tab).is_some())
    }
}

/// `Radio.detail-votes` before the directory has answered — which for a station it no longer
/// lists is forever. `0` is a real number of votes and prints as one.
const VOTES_UNKNOWN: i32 = -1;

/// `Radio.detail-tab` with no station page on the mounted tab.
pub const NO_SEAT: i32 = -1;

/// Which tab's page is painted right now.
fn mounted_tab(g: &Radio<'_>) -> RadioTab {
    tab_from_index(g, g.get_tab_idx())
}

/// The station a page was opened for, from whichever side had it.
///
/// Two types rather than one because they know different things, which is [`DirectoryStation`]'s
/// own argument: the table's id, logo and play stats mean nothing to the directory, and the
/// popularity figures have no column here.
#[derive(Clone)]
enum StationSource {
    Kept(RadioStation),
    /// The directory's answer, and whatever logo this session found for it.
    Browsed(DirectoryStation, Option<String>),
}

impl StationSource {
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

    /// What the band's chip strip states — where the station is and what it plays, and nothing
    /// the band or the body already says. The codec, bitrate and vote count are rows on the page
    /// instead, and the country is the band's own subtitle.
    ///
    /// A kept station has no `state`: only the directory carries it, and
    /// [`refresh_from_directory`] is what fills it in.
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

    /// The directory's vote count where the page was opened from a directory answer, so the row
    /// is right on the first frame rather than after the refresh lands.
    fn votes(&self) -> i32 {
        match self {
            Self::Kept(_) => VOTES_UNKNOWN,
            Self::Browsed(station, _) => clamp_i64_to_i32(station.votes),
        }
    }

    /// UI thread only — the browsed arm reads the star shadow the grid is built from.
    fn to_row(&self, radio_ui: &RadioUi) -> RadioStationRow {
        match self {
            Self::Kept(station) => rows::to_slint_kept_station_row(station),
            Self::Browsed(station, logo) => rows::to_slint_radio_station_row(
                station,
                radio_ui.starred.lock().contains(&station.station_uuid),
                logo.as_deref(),
            ),
        }
    }
}

/// Open a station's page.
pub async fn open_station(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: Weak<AppWindow>,
    station: StationRef,
    enter_from: NavEnterFrom,
) -> AppResult<()> {
    open_station_with(state, radio_ui, weak, station, enter_from, |_ui| {}).await
}

/// Open a station's page, running `on_applied` in the **same** UI-thread closure that raises
/// `detail-open`.
///
/// The hook is what a Mouse-4/5 replay landing on a station page uses: the body router is a pure
/// function of `(tab-idx, detail-open)`, so a nav written up front mounts the destination's *grid*
/// for the length of this fetch. Handed in here, both land in one tick.
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
    let facts = source.facts();
    let votes = source.votes();
    let pair =
        decode_detail_pair(state, radio_ui.detail_artwork.clone(), source.artwork_path()).await;
    let votable = state.radio_enabled() && !station.uuid.is_empty();

    let landed_state = state.clone();
    let paint_ui = radio_ui.clone();
    let seated = OpenStation {
        station: station.clone(),
        source,
        facts,
        // A refresh has not answered yet, and a stale verdict under a new station is worse than
        // none.
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
        // **After the hook**, which is what moves the tab on a history replay: the seat this
        // lands in, and the section shadow `paint_seat` reads, both answer for the tab being
        // left if anything here runs ahead of it.
        let tab = mounted_tab(&g);
        paint_seat(&ui, &g, &radio_ui, &seated, true);
        *radio_ui.detail.lock().seat_mut(tab) = Some(seated);
        // Reads the seat, so it comes after the store.
        sync_history_seat(&ui, &radio_ui);
        // **The seat property last** — everything above is what it gates.
        g.set_detail_tab(g.get_tab_idx());

        // Named for the next launch here rather than at the click, so every way in gets it: the
        // Mouse-4/5 replay and the boot restore never touch that callback. It is also the only
        // point at which the tab the station landed on is known.
        persist_seat(&landed_state, &ui, &radio_ui);
        crate::ui::nav_history::record_current(&landed_state, &ui);
        // Last: the box has just gone away, so whatever the user typed on the tab underneath is
        // no longer in front of them, and a re-open with the same station fires no mirror.
        g.invoke_detail_scope_changed();
    });

    // **Spawned, not awaited.** The page is painted; this only adds chips to it, and a directory
    // that is merely slow would otherwise hold the open's caller for a whole request timeout —
    // the boot restore's, which is what lowers `Radio.restoring`.
    let (refresh_state, refresh_ui) = (state.clone(), radio_ui.clone());
    state.runtime.spawn(async move {
        refresh_from_directory(&refresh_state, &refresh_ui, &weak, &station).await;
    });
    Ok(())
}

/// Write a seat into the properties the station page and the band paint from.
///
/// The one writer for all three arrivals — a fresh open, a tab hand-off and a section re-enter's
/// re-decode — so the page can never describe one station while the hero paints another's.
/// `animate` is the caller answering "is this a hand-off between stations": `true` for an open
/// and a tab move, `false` for a re-decode of what is already on screen.
///
/// UI thread only, and it reads `section_is_up` live, so any navigation the caller owes has to
/// have landed first.
fn paint_seat(
    ui: &AppWindow,
    g: &Radio<'_>,
    radio_ui: &RadioUi,
    open: &OpenStation,
    animate: bool,
) {
    // The Slint row is built here rather than carried across an await: it holds `SharedString`s,
    // and this is also the only side that can read the star shadow.
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

/// Point the painted station page at whatever `tab` was left holding, or give the seat up where
/// that tab holds nothing.
///
/// **Synchronous, and on the click's own tick.** `Radio.tab-idx` has already moved by the time
/// this runs, so anything deferred would let the view's own `changed` trackers read a
/// `detail-open` that is briefly neither tab's answer.
///
/// With no seat it writes [`NO_SEAT`] and **touches nothing else**: the band paints the departing
/// station all the way through its collapse, which is the same reason a close leaves the facts
/// alone and `hero-collapsed` is what hands the images back.
///
/// UI thread only.
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

/// The station behind a [`StationRef`], from whichever cache holds it.
///
/// The caches come first because they carry what the database does not: the logo this session
/// found on a station's own site, and the directory answer a browsed station has no row for. The
/// database is the fallback for the one path with no cache at all, the boot restore.
async fn resolve(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    station: &StationRef,
) -> AppResult<StationSource> {
    if station.is_kept() {
        if let Some(kept) = kept_from_cache(radio_ui, station.id) {
            return Ok(StationSource::Kept(kept));
        }
        return Ok(StationSource::Kept(library::radio::get_station(state, station.id).await?));
    }
    browse::resolve(radio_ui, &station.uuid)
        .map(|(found, logo)| StationSource::Browsed(found, logo))
        .ok_or_else(|| AppError::NotFound("Station is no longer in the browse results".to_owned()))
}

/// A kept station from either local tab's cache — the row lives in both when it is starred *and*
/// has been played, and in only one otherwise, so both are asked.
fn kept_from_cache(radio_ui: &RadioUi, id: i64) -> Option<RadioStation> {
    kept::resolve(radio_ui, RadioTab::Favorites, id)
        .or_else(|| kept::resolve(radio_ui, RadioTab::Recent, id))
}

/// Ask the directory what it currently says about the open station, and fold in the facts no
/// local row can carry.
///
/// Quiet on every failure, including the switch being off: this is a refinement of a page that is
/// already drawn, so an unreachable directory means fewer chips rather than an error to report.
///
/// The answer goes into whichever tab's seat holds that station, and reaches the properties only
/// where that seat is the mounted one — a page waiting on another tab still comes back with it.
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
            log::debug!("radio: station details not read: {}", crate::services::describe(&e));
            return;
        }
    };

    let radio_ui = radio_ui.clone();
    let station = station.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let votes = clamp_i64_to_i32(found.votes);
        let Some((seated, facts)) = ({
            let mut detail = radio_ui.detail.lock();
            // The user closed the page while this was in flight, or opened another station over
            // it. Every tab is asked: the open that started this may not be the mounted one any
            // more.
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

/// Drop the Rust-side mirror of the page the mounted tab was showing. The Slint half is the
/// closing callback's, and the hero's images are `hero-collapsed`'s.
///
/// **The mounted tab's alone**: a close is the back arrow on one page, and the other tabs' pages
/// are not the user's to lose to it.
pub fn close_detail(ui: &AppWindow, radio_ui: &RadioUi) {
    let tab = mounted_tab(&ui.global::<Radio>());
    radio_ui.detail.lock().seat_mut(tab).take();
}

/// Give up every seat. The Radio switch going off is the only caller: the page itself is being
/// taken away, so there is nothing left for the two unmounted tabs to come back to.
pub fn forget_all_seats(radio_ui: &RadioUi) {
    *radio_ui.detail.lock() = DetailState::default();
}

/// Whether any tab is holding a page, for the same caller.
pub fn any_seated(radio_ui: &RadioUi) -> bool {
    radio_ui.detail.lock().any_seated()
}

/// Hand every seat's decoded hero back, keeping the seats themselves. The section leave's, beside
/// the tier clear the images came out of; [`rewarm_hero`] is what the enter rebuilds them with.
pub fn release_seated_artwork(radio_ui: &RadioUi) {
    let mut detail = radio_ui.detail.lock();
    for tab in RadioTab::ALL {
        if let Some(open) = detail.seat_mut(tab).as_mut() {
            open.artwork = DetailPair::default();
        }
    }
}

/// Name the mounted tab's station in `views.json`, or clear the key where that tab names none.
///
/// **Only a station with a database row can be named** — a browsed one is a directory answer with
/// a shelf life, and `id == 0` is what says so. Called on an open, a close and every tab move,
/// because with a seat per tab "the last station opened" and "the station the restored tab is
/// showing" stopped being the same answer, and only the second is what a boot can act on.
///
/// The shadow beside it is what keeps a tab bounce off the disk: the value changes on an open and
/// a close, and on a tab move only where the two tabs disagree. What survives that holds
/// `RadioUi::persist_writer` for the whole round trip, and reloads the shadow under it so a
/// superseded write drops its own — the ordering the pool does not give, and which the tab index
/// written beside this one takes from `IndexPersist`.
///
/// UI thread only.
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
        if let Err(e) = library::settings::set_last_detail_id(&state, view_id::RADIO_DETAIL, named)
        {
            log::warn!("radio: persist station detail: {e}");
        }
    });
}

/// Every tab holding a page, and which station each is about.
///
/// Taken as a snapshot rather than walked under the lock: both callers reach a cache per seat and
/// `restamp` reaches the star shadow besides, and neither is ours to order against `detail`.
fn seated_stations(radio_ui: &RadioUi) -> Vec<(RadioTab, StationRef)> {
    let detail = radio_ui.detail.lock();
    RadioTab::ALL
        .into_iter()
        .filter_map(|tab| detail.seat(tab).map(|open| (tab, open.station.clone())))
        .collect()
}

/// Re-read every seated station from whichever cache owns it and rewrite the row the page paints.
///
/// Called at the tail of the two single write paths, `browse::apply` and `kept::apply`, so the
/// page and the grid behind it cannot disagree about a station they both draw.
///
/// **A miss is left alone**, deliberately: these run at boot and on a column change too, when the
/// cache the page's station lives in may simply not have been filled yet. Whether the station is
/// *gone* is [`close_if_gone`]'s question, and only one caller is in a position to ask it.
///
/// **Every seat, not just the mounted one**, because a tab move repaints from the seat's own
/// source: left unstamped, a page waiting on another tab comes back describing the station as it
/// was when it was opened, and a star flipped in the meantime is invisible until it is reopened.
/// Only the mounted tab's row reaches Slint.
///
/// UI thread only.
pub fn restamp(ui: &AppWindow, radio_ui: &RadioUi) {
    let g = ui.global::<Radio>();
    let mounted = mounted_tab(&g);

    for (tab, station) in seated_stations(radio_ui) {
        let Some(source) = from_cache(radio_ui, &station) else {
            continue;
        };
        // Built before the seat takes the source: `to_row` reaches the star shadow, and the two
        // are not ours to order.
        let row = (tab == mounted).then(|| source.to_row(radio_ui));
        if let Some(open) = radio_ui.detail.lock().seat_mut(tab).as_mut() {
            open.source = source;
        }
        if let Some(row) = row {
            g.set_detail_station(row);
        }
    }
}

/// Close a kept station's page once its row is gone.
///
/// **Called from the one place a miss is authoritative** — `kept::refresh`'s landing, which has
/// just rebuilt both caches from the database. Elsewhere an absent station is an unfilled cache.
///
/// It is not defensive: un-starring a station with no play behind it takes `delete_if_unlisted`
/// with it, so the row a page is about really can vanish under it. Left open, the page keeps
/// offering Play and Remove against a dead id, each failing to a log line with nothing on screen
/// to say so.
///
/// Every tab is asked, not just the mounted one: a page waiting on another tab is exactly the one
/// nothing else would ever notice had gone.
///
/// UI thread only.
pub fn close_if_gone(ui: &AppWindow, radio_ui: &RadioUi) {
    let g = ui.global::<Radio>();
    let mounted = mounted_tab(&g);
    for (tab, station) in seated_stations(radio_ui) {
        // A browsed page is backed by the directory answer, not by these caches, and outlives
        // them.
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

/// Re-answer whether the mounted tab's page is the station whose titles `Radio.history-rows` holds.
///
/// The rows themselves belong to whatever is playing and are written by `history::apply`; this is
/// the only thing a seat change moves, which is why a tab hand-off no longer rebuilds a model.
///
/// UI thread only.
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

/// Re-decode the heroes for whatever tabs were left holding a page when the section was left.
///
/// The leave hands the hero's images and colours back and the enter rebuilds them, which is the
/// one thing this page's otherwise-narrow leave has to re-fetch. Everything else — the three
/// grids, the browse page, the logo memo — survives.
///
/// **Every seat, not only the mounted one.** A tab move repaints from the seat's own held pair,
/// so a seat left cold would come back a decode late and flash its monogram on the way. Bounded
/// at three.
///
/// **The mounted tab decodes first**, the rest being work for a tab move that has not happened
/// yet: queued in declaration order the banner on screen waits behind up to two heroes nobody is
/// looking at, and on a cold re-enter those are real decodes rather than tier hits.
///
/// UI thread only, for the mounted-tab read.
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
                // Cloned out rather than painted under the lock: `paint_seat` reaches the star
                // shadow, and the two are not ours to order.
                let Some(open) = radio_ui.detail.lock().seat(tab).cloned() else {
                    return;
                };
                // `animate: false` — the banner was already on screen when the page was left, so
                // this is a re-decode of what is painted rather than a hand-off between stations.
                paint_seat(&ui, &g, &radio_ui, &open, false);
            });
        }
    });
}

/// Reopen whatever station `views.json` was left holding.
///
/// **Only a station with a row can be named**, which is the whole of what `id != 0` buys: a
/// browsed station is a directory answer with a shelf life, so a restart lands on the tab root
/// rather than on a page rebuilt from a cache that no longer exists.
pub fn seed_detail_from_settings(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let named = library::settings::get_view_state(state)
        .ok()
        .and_then(|vs| vs.last_detail_ids.get(view_id::RADIO_DETAIL).copied());
    // Seeded **before** the liveness filter, so the shadow says what the file says: an id whose
    // row has since gone is exactly what the next [`persist_seat`] has to be allowed to clear.
    radio_ui.detail.lock().persisted = named;

    let Some(id) = named.filter(|id| rows::station_has_row(*id)) else {
        return;
    };

    // Synchronously, ahead of `app.show()`: the tab bodies gate on it, so without it a restore
    // paints a grid for the length of the fetch and swaps.
    ui.global::<Radio>().set_restoring(true);

    let (state, radio_ui, weak) = (state.clone(), radio_ui.clone(), ui.as_weak());
    state.runtime.clone().spawn(async move {
        let station = StationRef {
            id,
            uuid: String::new(),
        };
        if let Err(e) =
            open_station(&state, &radio_ui, weak.clone(), station, NavEnterFrom::Below).await
        {
            log::warn!("radio: restore station detail: {}", crate::services::describe(&e));
        }
        let _ = weak.upgrade_in_event_loop(|ui| {
            ui.global::<Radio>().set_restoring(false);
        });
    });
}

/// The decoded logo's smallest side, which is what the hero compares its tile against.
///
/// Zero where nothing decoded, and the hero reads that as "draw the monogram" rather than as a
/// zero-sized image.
fn logo_size(pair: &DetailPair) -> i32 {
    pair.cover
        .as_ref()
        .map_or(0, |cover| clamp_i64_to_i32(i64::from(cover.width().min(cover.height()))))
}
