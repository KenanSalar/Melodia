//! The station page: open, close, the hero it morphs the band into, and the directory refresh
//! behind it.
//!
//! **A station is opened from a row, not fetched by id.** A browsed station has no database row —
//! `RadioStationRow.id` is `0` for one — so the id cannot be the handle, and it cannot be the
//! open/closed flag either. [`StationRef`] carries both halves of what identifies either kind and
//! `Radio.detail-open` carries the flag.
//!
//! **The refresh is additive.** What a kept row knows and what the directory knows overlap but do
//! not agree: the table has no column for the popularity figures, the state or the directory's own
//! last check, and the four `local_*` columns are the user's answers to what the directory left
//! blank. So [`refresh_from_directory`] fills the first set and touches nothing else — letting it
//! rewrite the rest would undo an override from a fetch nobody asked for.

use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use crate::entities::radio::{DirectoryStation, RadioStation};
use crate::error::{AppError, AppResult};
use crate::library;
use crate::state::AppState;
use crate::ui::detail_artwork::decode_detail_pair;
use crate::ui::detail_view::impl_detail_view_helpers;
use crate::ui::hero_chips::{self, StationFacts};
use crate::ui::row_match::fold_needle;
use crate::ui::track_list_view::view_id;
use crate::ui::util::{clamp_i64_to_i32, len_as_i32};
use crate::{AppWindow, NavEnterFrom, Radio, RadioStationRow};

use super::tabs::{RadioTab, section_is_up};
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

/// The station on screen, plus what a re-publish of the hero needs without asking again.
struct OpenStation {
    station: StationRef,
    stream_url: String,
    artwork_path: Option<String>,
    facts: StationFacts,
}

/// The page's detail state, held beside the three tab caches.
#[derive(Default)]
pub struct DetailState {
    open: Option<OpenStation>,
    /// The box's needle over the song history. Stored **raw**, `kept::KeptState`'s shape rather
    /// than the four track details': the box is reseated from this, and a folded `Needle` holds
    /// its text lowercased and unaccented, which is not the spelling to put back in front of
    /// somebody.
    filter: String,
}

/// The station a page was opened for, from whichever side had it.
///
/// Two types rather than one because they know different things, which is [`DirectoryStation`]'s
/// own argument: the table's id, logo and play stats mean nothing to the directory, and the
/// popularity figures have no column here.
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

    /// What the band's chip strip states. A kept station has no `state` and no vote count: those
    /// live only in the directory's answer, and [`refresh_from_directory`] is what fills them in.
    fn facts(&self) -> StationFacts {
        match self {
            Self::Kept(station) => StationFacts {
                tags: rows::split_tags(station.genre().unwrap_or_default()),
                country: station.country_name().unwrap_or_default().to_owned(),
                state: String::new(),
                language: station.language.clone(),
                codec: station.codec.clone(),
                bitrate: station.bitrate,
                votes: None,
            },
            Self::Browsed(station, _) => StationFacts {
                tags: rows::split_tags(&station.tags),
                country: station.country.clone(),
                state: station.state.clone(),
                language: station.language.clone(),
                codec: station.codec.clone(),
                bitrate: station.bitrate,
                votes: Some(station.votes),
            },
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
    let stream_url = source.stream_url().to_owned();
    let artwork_path = source.artwork_path();
    let facts = source.facts();
    let pair =
        decode_detail_pair(state, radio_ui.detail_artwork.clone(), artwork_path.clone()).await;
    let votable = state.radio_enabled() && !station.uuid.is_empty();

    let open = OpenStation {
        station: station.clone(),
        stream_url: stream_url.clone(),
        artwork_path,
        facts: facts.clone(),
    };
    *radio_ui.detail.lock() = DetailState {
        open: Some(open),
        filter: String::new(),
    };

    let history_state = state.clone();
    let paint_ui = radio_ui.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let radio_ui = paint_ui;
        let g = ui.global::<Radio>();
        // The Slint row is built here rather than carried across the await: it holds
        // `SharedString`s, and this is also the only side that can read the star shadow.
        g.set_detail_station(source.to_row(&radio_ui));
        g.set_detail_stream_url(stream_url.into());
        g.set_detail_votable(votable);
        // A refresh has not answered yet, and a stale verdict under a new station is worse
        // than none.
        g.set_detail_check_known(false);
        g.set_detail_check_ok(false);
        g.set_detail_logo_size(logo_size(&pair));
        apply_history(&ui, &radio_ui);

        crate::ui::nav_transition::mark(&ui, enter_from);
        // The flag, and everything above is what it gates.
        g.set_detail_open(true);
        on_applied(&ui);

        // Live, and after the hook: the section shadow updates next frame, and a replay moves
        // the nav index from inside that hook.
        let on_screen = section_is_up(&ui);
        hero_chips::publish_station(&ui, &facts, on_screen);
        apply_detail_artwork(&ui, &g, pair, true, on_screen);
        crate::ui::nav_history::record_current(&history_state, &ui);
        // Last: the box now describes the song history rather than the tab under it, and a
        // re-open with the same station has no edge for the sheet's mirror to fire on.
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
async fn refresh_from_directory(
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
        let facts = {
            let mut detail = radio_ui.detail.lock();
            // The user moved on while this was in flight, or moved to another station.
            let Some(open) = detail.open.as_mut().filter(|open| open.station == station) else {
                return;
            };
            open.facts.state.clone_from(&found.state);
            open.facts.votes = Some(found.votes);
            open.facts.clone()
        };
        let g = ui.global::<Radio>();
        g.set_detail_check_known(true);
        g.set_detail_check_ok(found.last_check_ok);
        hero_chips::publish_station(&ui, &facts, section_is_up(&ui));
    });
}

/// Ask the directory again about whatever station is open. What the vote action reaches for, a
/// vote's own answer carrying no count.
pub async fn refresh_open_station(
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    weak: &Weak<AppWindow>,
) {
    let Some(station) = open_station_ref(radio_ui) else {
        return;
    };
    refresh_from_directory(state, radio_ui, weak, &station).await;
}

/// Drop the Rust-side mirror of an open detail. The Slint half is the closing callback's, and the
/// hero's images are `hero-collapsed`'s.
pub fn close_detail(radio_ui: &RadioUi) {
    *radio_ui.detail.lock() = DetailState::default();
}

/// The open station, for the callbacks that act on it.
pub fn open_station_ref(radio_ui: &RadioUi) -> Option<StationRef> {
    radio_ui.detail.lock().open.as_ref().map(|open| open.station.clone())
}

/// The detail's needle, for the box reseat.
pub fn filter_text(radio_ui: &RadioUi) -> String {
    radio_ui.detail.lock().filter.clone()
}

/// Narrow the song history. The page's fourth filter destination.
pub fn set_filter(ui: &AppWindow, radio_ui: &RadioUi, filter: &str) {
    {
        let mut detail = radio_ui.detail.lock();
        if detail.filter == filter {
            return;
        }
        filter.clone_into(&mut detail.filter);
    }
    apply_history(ui, radio_ui);
}

/// Write the open station's song history into the global, through the box's needle.
///
/// **The count is the unfiltered one**, `kept::apply`'s rule: it is what tells "this station has
/// announced nothing" from "the box matched nothing", and those are different empty states.
///
/// UI thread only.
pub fn apply_history(ui: &AppWindow, radio_ui: &RadioUi) {
    let detail = radio_ui.detail.lock();
    let Some(open) = detail.open.as_ref() else {
        return;
    };
    let history = radio_ui.history.lock();
    let Some(titles) = history.titles_for(&open.stream_url) else {
        // A different station is playing, or none has yet. The page is not wrong to be empty.
        let g = ui.global::<Radio>();
        g.set_history_count(0);
        g.set_history_rows(ModelRc::new(VecModel::<slint::SharedString>::default()));
        return;
    };

    let needle = fold_needle(&detail.filter);
    let matched: Vec<slint::SharedString> = titles
        .iter()
        .filter(|title| needle.contains(title))
        .map(|title| slint::SharedString::from(title.as_str()))
        .collect();

    let g = ui.global::<Radio>();
    g.set_history_count(len_as_i32(titles.len()));
    g.set_history_rows(ModelRc::new(VecModel::from(matched)));
}

/// Re-publish the hero for a detail that was left open when the section was.
///
/// The leave hands the hero's images and colours back and the enter rebuilds them, which is the
/// one thing this page's otherwise-narrow leave has to re-fetch. Everything else — the three
/// grids, the browse page, the logo memo — survives.
pub fn rewarm_hero(state: &AppState, radio_ui: &Arc<RadioUi>, weak: &Weak<AppWindow>) {
    let Some((artwork_path, facts)) = radio_ui
        .detail
        .lock()
        .open
        .as_ref()
        .map(|open| (open.artwork_path.clone(), open.facts.clone()))
    else {
        return;
    };

    let (state, radio_ui, weak) = (state.clone(), radio_ui.clone(), weak.clone());
    state.runtime.clone().spawn(async move {
        let pair = decode_detail_pair(&state, radio_ui.detail_artwork.clone(), artwork_path).await;
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let g = ui.global::<Radio>();
            if !g.get_detail_open() {
                return;
            }
            g.set_detail_logo_size(logo_size(&pair));
            let on_screen = section_is_up(&ui);
            hero_chips::publish_station(&ui, &facts, on_screen);
            // `animate: false` — the banner was already on screen when the page was left, so
            // this is a re-decode of what is painted rather than a hand-off between stations.
            apply_detail_artwork(&ui, &g, pair, false, on_screen);
        });
    });
}

/// Reopen whatever station `views.json` was left holding.
///
/// **Only a station with a row can be named**, which is the whole of what `id != 0` buys: a
/// browsed station is a directory answer with a shelf life, so a restart lands on the tab root
/// rather than on a page rebuilt from a cache that no longer exists.
pub fn seed_detail_from_settings(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let Some(id) = library::settings::get_view_state(state)
        .ok()
        .and_then(|vs| vs.last_detail_ids.get(view_id::RADIO_DETAIL).copied())
        .filter(|id| rows::station_has_row(*id))
    else {
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
fn logo_size(pair: &crate::ui::detail_artwork::DetailPair) -> i32 {
    pair.cover
        .as_ref()
        .map_or(0, |cover| clamp_i64_to_i32(i64::from(cover.width().min(cover.height()))))
}
