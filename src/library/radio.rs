//! Radio library API, and the only door the UI has onto stations.
//!
//! The stored table and the radio-browser.info directory answer through the same
//! module rather than side by side, so a callback never has to know which one
//! did, and so the toggle that turns radio off has one place to guard.
//!
//! Writes here deliberately do not bump `library_changed_tx`. Its subscribers
//! are the library views, none of which shows a station; the Radio section
//! refreshes through its own global.

use std::sync::Arc;

use crate::database::queries;
use crate::entities::radio;
use crate::error::AppError;
use crate::library::playback;
use crate::player::stream_source::{self, StationFacts};
use crate::player::types::RadioNowPlaying;
use crate::services::radio_browser;
use crate::state::AppState;

/// Stations per directory page.
///
/// Re-exported rather than restated so a caller can page without naming the client: an offset
/// advances by exactly the limit the request carried, and the two coming from one definition is
/// what stops paging skipping or repeating a page.
pub use radio_browser::DEFAULT_PAGE_LIMIT;

/// How far back the recently-played station list reaches.
///
/// A station history has no natural end, so this is what bounds the fetch. The
/// list is a way back to what you were just listening to rather than a record,
/// so it is sized to a session or two of tuning around, not to everything ever
/// played.
pub const RECENT_STATIONS_LIMIT: i64 = 50;

/// A refusal when the user has switched Radio off.
///
/// **This is where "off" means no traffic.** D14 already made this module the only door
/// onto the directory, so the switch is enforced here rather than at the sidebar row,
/// which a stale callback or an in-flight fetch is already past. Stored stations stay
/// readable through the getters below: hiding a feature is not deleting what the user
/// kept, and nothing but the section itself asks for them.
fn ensure_enabled(state: &AppState) -> Result<(), AppError> {
    if state.radio_enabled() {
        return Ok(());
    }
    Err(AppError::Settings("Radio is switched off".to_owned()))
}

/// The shared client, reachable only past [`ensure_enabled`] — which is the point of
/// spelling it as a seam rather than repeating the check beside each call.
///
/// Every outbound call this module makes takes it, including the logo download, whose host
/// is one the directory named rather than the directory itself. The guard is about traffic,
/// not about who is on the other end.
fn directory_client(state: &AppState) -> Result<&reqwest::Client, AppError> {
    ensure_enabled(state)?;
    Ok(state.http_client())
}

/// Every favorited station, naturally name-ordered.
pub async fn get_favorites(state: &AppState) -> Result<Vec<radio::RadioStation>, AppError> {
    queries::radio::get_favorite_stations(&state.db).await
}

/// The stations played most recently, newest first.
pub async fn get_recent(state: &AppState) -> Result<Vec<radio::RadioStation>, AppError> {
    queries::radio::get_recent_stations(&state.db, RECENT_STATIONS_LIMIT).await
}

/// One station, or `AppError::NotFound` if it is gone.
pub async fn get_station(state: &AppState, id: i64) -> Result<radio::RadioStation, AppError> {
    queries::radio::get_station_by_id(&state.db, id).await
}

/// Persist a station, updating the row when the directory already knows it.
/// Preserves everything the user did with it.
pub async fn save_station(
    state: &AppState,
    station: &radio::NewRadioStation,
) -> Result<radio::RadioStation, AppError> {
    queries::radio::save_station(&state.db, station).await
}

pub async fn set_favorite(state: &AppState, id: i64, favorite: bool) -> Result<(), AppError> {
    queries::radio::set_favorite(&state.db, id, favorite).await
}

pub async fn remove_station(state: &AppState, id: i64) -> Result<(), AppError> {
    queries::radio::delete_station(&state.db, id).await
}

/// Open a URL far enough to know it plays, and report what the server said about itself.
///
/// Behind [`directory_client`] like every other outbound call here: a probe is traffic whoever
/// is on the other end, and "off means no traffic" does not have an exception for a URL the user
/// typed.
async fn probe_station(state: &AppState, stream_url: &str) -> Result<StationFacts, AppError> {
    stream_source::probe(directory_client(state)?, stream_url).await
}

/// What to call a station nobody named.
///
/// The user's own text wins, then whatever the server calls itself, and the host last — a row
/// titled with a full stream URL is unreadable in a card and sorts under `https`.
///
/// Shared with [`super::radio_files`] so a station typed in and one imported from a nameless
/// playlist entry end up called the same thing.
pub(super) fn resolve_station_name(
    typed: &str,
    from_stream: Option<&str>,
    stream_url: &str,
) -> String {
    let typed = typed.trim();
    if !typed.is_empty() {
        return typed.to_owned();
    }
    if let Some(name) = from_stream.map(str::trim).filter(|n| !n.is_empty()) {
        return name.to_owned();
    }
    reqwest::Url::parse(stream_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| stream_url.to_owned())
}

/// Keep a station the user typed in, after proving it plays.
///
/// **The probe is the validation**, and it is deliberately the whole connect rather than a
/// reachability check: a mount that is a web page, a segmented playlist or a codec with no
/// decoder is refused here, with the dialog still open, instead of at the first click.
///
/// Starred on the way in, because a station is typed in for one reason. That is also what puts it
/// in `get_favorite_stations`, which is why the kept list needs no query of its own.
///
/// **A URL already here merges onto its row rather than adding a second one**, which is the answer
/// the other two doors already give: a browsed station can't duplicate (`station_uuid` is UNIQUE)
/// and the import skips what it already holds. Without it a re-pasted URL spends a whole probe to
/// produce a second identical card. The merge stars what it finds, so re-adding a station that was
/// only ever played promotes it into the kept list.
pub async fn add_custom_station(
    state: &AppState,
    stream_url: &str,
    name: &str,
) -> Result<i64, AppError> {
    if let Some(id) = queries::radio::station_id_with_url(&state.db, stream_url).await? {
        set_favorite(state, id, true).await?;
        return Ok(id);
    }

    let facts = probe_station(state, stream_url).await?;
    let station = radio::NewRadioStation {
        station_uuid: None,
        name: resolve_station_name(name, facts.name.as_deref(), stream_url),
        stream_url: stream_url.to_owned(),
        homepage: facts.homepage.clone(),
        favicon_url: facts.logo_url.clone(),
        tags: facts.genre.clone(),
        // The three the directory would have filled. A stream announces no country and no
        // language, and guessing one from the host would be wrong more often than blank is.
        country: String::new(),
        country_code: String::new(),
        language: String::new(),
        codec: facts.codec.clone(),
        bitrate: facts.bitrate,
        // Nothing segmented survives the probe, so this is a fact rather than an assumption.
        hls: false,
    };

    let saved = save_station(state, &station).await?;
    set_favorite(state, saved.id, true).await?;
    if let Some(logo_url) = facts.logo_url.as_deref() {
        adopt_logo(state, saved.id, logo_url).await;
    }
    Ok(saved.id)
}

/// Refuse to edit a station the directory owns.
///
/// **The refusal is the point rather than a nicety**: `save_station`'s conflict clause rewrites
/// `name` and `stream_url` from the directory on the next favorite or play of the same uuid, so
/// an edit to a browsed station would revert with nothing on screen to say why. The card only
/// offers Edit on a custom station; this is what holds when something else asks.
fn ensure_editable(station: &radio::RadioStation) -> Result<(), AppError> {
    if station.station_uuid.is_none() {
        return Ok(());
    }
    Err(AppError::Validation("Only a station you added by URL can be edited".to_owned()))
}

/// Refuse a segmented stream, whichever surface asked.
///
/// Symphonia has no MPEG-TS demuxer, so the card's badge is the honest half and this is the other.
/// Both play doors take it: Browse reaches the decoder by row, the kept tabs by id, and a refusal
/// on one of them only is a refusal a stale row walks around.
fn ensure_playable(hls: bool) -> Result<(), AppError> {
    if !hls {
        return Ok(());
    }
    Err(AppError::Validation(
        "This station streams in a segmented format Melodia cannot play yet".to_owned(),
    ))
}

/// Rewrite a hand-typed station's name and URL.
///
/// The probe runs only when the URL actually moved, so renaming a station that happens to be off
/// air today still works — and when it did move, **everything the old mount said about itself
/// goes with it**, logo included. A repointed station keeping the previous brand's icon and
/// homepage link is the failure `keep_station` already argues against on the directory's side.
pub async fn update_custom_station(
    state: &AppState,
    id: i64,
    stream_url: &str,
    name: &str,
) -> Result<(), AppError> {
    let existing = get_station(state, id).await?;
    ensure_editable(&existing)?;
    let moved = existing.stream_url != stream_url;

    let mut edit = radio::StationEdit {
        name: resolve_station_name(name, Some(existing.name.as_str()), stream_url),
        stream_url: stream_url.to_owned(),
        homepage: existing.homepage,
        favicon_url: existing.favicon_url,
        tags: existing.tags,
        codec: existing.codec,
        bitrate: existing.bitrate,
    };

    let mut logo_url = None;
    if moved {
        let facts = probe_station(state, stream_url).await?;
        logo_url = facts.logo_url.clone();
        edit.homepage = facts.homepage;
        edit.favicon_url = facts.logo_url;
        edit.tags = facts.genre;
        edit.codec = facts.codec;
        edit.bitrate = facts.bitrate;
    }

    queries::radio::update_station(&state.db, id, &edit).await?;
    if let Some(logo_url) = logo_url.as_deref() {
        adopt_logo(state, id, logo_url).await;
    }
    Ok(())
}

/// Download a station's logo and point the row at it, or leave the row alone.
///
/// Failures are silent by design: a station with no logo takes the Material Symbols glyph, which
/// is what most of the directory takes anyway, and there is nothing a user could do about a dead
/// favicon host.
async fn adopt_logo(state: &AppState, id: i64, logo_url: &str) {
    match fetch_logo(state, logo_url).await {
        Ok(Some(path)) => {
            if let Err(e) = set_artwork(state, id, Some(&path)).await {
                log::debug!("radio: station logo not stored: {}", crate::services::describe(&e));
            }
        }
        Ok(None) => {}
        Err(e) => {
            log::debug!("radio: station logo fetch failed: {}", crate::services::describe(&e));
        }
    }
}

/// Count a play against a station, which is what orders the recents list.
pub async fn mark_played(state: &AppState, id: i64) -> Result<(), AppError> {
    queries::radio::mark_played(&state.db, id).await
}

/// Tune to a stored station, counting the play ahead of the connect.
///
/// The count goes in even if the stream turns out to be unreachable: it records that the user
/// chose the station, which is what orders the recents list, and a station that is down today is
/// exactly the one they will want to find again. Hence the ordering — counting afterwards would
/// make the row conditional on the server being up.
pub async fn play_station(state: &AppState, id: i64) -> Result<(), AppError> {
    ensure_enabled(state)?;
    let station = get_station(state, id).await?;
    // Ahead of the count, unlike an unreachable stream: a station that is down today is exactly
    // the one to find again, where a segmented one can never play at all and does not belong in a
    // list of what to go back to.
    ensure_playable(station.hls)?;
    let now_playing = RadioNowPlaying::from(&station);
    mark_played(state, id).await?;
    // Every play passes through here, whichever surface started it, so the directory's own
    // count is reported once and in one place rather than at each caller.
    spawn_click(state, station.station_uuid.as_deref());
    playback::player_play_station(&state.playback_ctx(), &now_playing).await
}

/// Write a browsed station into the table, which is what makes it the user's.
///
/// Directory results are otherwise never persisted (D3), so this is the crossing, and both of
/// the things that count as keeping a station go through it.
///
/// **The logo write follows the fetched URL, not the stored file.** `save_station`'s conflict
/// clause deliberately leaves `artwork_path` alone, which is right for a re-import that changed
/// nothing else and wrong for a station whose logo moved: the caller fetched from the
/// `favicon_url` in hand, so pointing the row at what that returned is what stops a moved logo
/// showing the old one forever.
async fn keep_station(
    state: &AppState,
    station: &radio::DirectoryStation,
    logo: Option<&str>,
) -> Result<radio::RadioStation, AppError> {
    let saved = save_station(state, &station.to_new_station()).await?;
    if logo.is_some() {
        set_artwork(state, saved.id, logo).await?;
    }
    Ok(saved)
}

/// Keep or release a browsed station, writing its row on the way in.
///
/// Un-favoriting leaves the row: it may carry a play history, and deciding whether an unstarred
/// never-played station is worth deleting belongs with the surface that lists them.
pub async fn set_directory_favorite(
    state: &AppState,
    station: &radio::DirectoryStation,
    favorite: bool,
    logo: Option<&str>,
) -> Result<i64, AppError> {
    let saved = keep_station(state, station, logo).await?;
    set_favorite(state, saved.id, favorite).await?;
    Ok(saved.id)
}

/// Tune to a browsed station, keeping it first.
///
/// [`ensure_playable`] runs before the row is written rather than inside [`play_station`], so a
/// click that cannot succeed also does not leave a station behind.
pub async fn play_directory_station(
    state: &AppState,
    station: &radio::DirectoryStation,
    logo: Option<&str>,
) -> Result<(), AppError> {
    ensure_enabled(state)?;
    ensure_playable(station.hls)?;
    let saved = keep_station(state, station, logo).await?;
    play_station(state, saved.id).await
}

/// Tell the directory a station was played, if the user left that on.
///
/// Detached rather than awaited: the click is the directory's business and the listener's is the
/// audio, so a slow mirror must not sit between the click and the connect. Failures are a debug
/// line — there is nothing for a user to do about one, and the call is deduplicated server-side
/// so a repeat is not an error either.
fn spawn_click(state: &AppState, station_uuid: Option<&str>) {
    if !state.radio_send_clicks() {
        return;
    }
    let Some(uuid) = station_uuid.filter(|uuid| !uuid.is_empty()) else {
        return;
    };
    let (s, uuid) = (state.clone(), uuid.to_owned());
    state.runtime.spawn(async move {
        let Ok(client) = directory_client(&s) else {
            return;
        };
        if let Err(e) = radio_browser::count_click(client, &uuid).await {
            log::debug!("radio: click report failed: {}", crate::services::describe(&e));
        }
    });
}

/// Fetch a station's logo into the shared artwork store, returning its path there.
///
/// Through the same seam as the directory calls: "off" has to mean no traffic, and a logo
/// download is traffic whoever is serving it.
pub async fn fetch_logo(state: &AppState, favicon_url: &str) -> Result<Option<String>, AppError> {
    let client = directory_client(state)?;
    crate::media::station_logo::fetch(client, favicon_url, &state.paths.artwork_dir).await
}

/// How long a logo URL that answered with nothing is left alone, per failed attempt. A day, so a
/// host down for an afternoon is asked again the next time the user opens Radio.
const LOGO_MISS_BACKOFF_HOURS: i64 = 24;

/// Ceiling on that multiplier. Past it the schedule stops escalating, a week between attempts
/// already rounding to never for anyone but a user who leaves the app open.
const LOGO_MISS_MAX_ATTEMPTS: i64 = 7;

/// Age past which a miss is dropped rather than escalated further. Deliberately clear of
/// `LOGO_MISS_BACKOFF_HOURS × LOGO_MISS_MAX_ATTEMPTS`, so nothing is pruned while it still
/// suppresses anything.
const LOGO_MISS_MAX_AGE_DAYS: i64 = 30;

/// The logo URLs a browse should not ask about yet, and a prune of the ones too old to matter.
///
/// Called once a session rather than per page: the answer only shrinks as the session records
/// its own misses, and the caller's memo is where those land.
pub async fn suppressed_logo_urls(state: &AppState) -> Result<Vec<String>, AppError> {
    let stale = chrono::Utc::now()
        - chrono::TimeDelta::try_days(LOGO_MISS_MAX_AGE_DAYS).unwrap_or_default();
    queries::radio::prune_logo_misses(&state.db, &stale.to_rfc3339()).await?;
    queries::radio::suppressed_logo_urls(&state.db, &crate::utils::now_rfc3339()).await
}

/// Record that `favicon_url` answered with nothing, pushing its next attempt further out.
pub async fn note_logo_miss(state: &AppState, favicon_url: &str) -> Result<(), AppError> {
    let attempts = queries::radio::logo_miss_attempts(&state.db, favicon_url)
        .await?
        .unwrap_or(0)
        .saturating_add(1);
    let hours = LOGO_MISS_BACKOFF_HOURS * attempts.min(LOGO_MISS_MAX_ATTEMPTS);
    let retry_after = chrono::Utc::now() + chrono::TimeDelta::try_hours(hours).unwrap_or_default();
    queries::radio::upsert_logo_miss(&state.db, favicon_url, attempts, &retry_after.to_rfc3339())
        .await
}

/// Forget a URL's misses, for a host that has started answering again.
pub async fn clear_logo_miss(state: &AppState, favicon_url: &str) -> Result<(), AppError> {
    queries::radio::clear_logo_miss(&state.db, favicon_url).await
}

/// Point a station at its stored logo, or clear it with `None`.
pub async fn set_artwork(
    state: &AppState,
    id: i64,
    artwork_path: Option<&str>,
) -> Result<(), AppError> {
    queries::radio::set_artwork(&state.db, id, artwork_path).await
}

/// Search the directory. Results are a network answer with a shelf life and are
/// never written to the table; one becomes a row when the user keeps or plays it.
pub async fn search(
    state: &AppState,
    search: &radio::StationSearch,
) -> Result<radio::StationPage, AppError> {
    let mut page = radio_browser::search(directory_client(state)?, search).await?;
    hide_hls(&mut page, state.radio_hide_hls());
    Ok(page)
}

/// Drop the segmented stations from a page, if the user has them hidden.
///
/// Here rather than in the request because the endpoint has no `hls` parameter to send. It thins
/// the page without touching [`radio::StationPage::has_more`], which the client already read off
/// the raw response: these rows were served and counted, and paging has to step over them rather
/// than stop at them.
fn hide_hls(page: &mut radio::StationPage, hide: bool) {
    if hide {
        page.stations.retain(|station| !station.hls);
    }
}

/// One of the directory's facet lists, for the filter chips. Large and
/// near-static, so it is fetched once per session and shared thereafter.
pub async fn facets(
    state: &AppState,
    kind: radio::FacetKind,
) -> Result<Arc<[radio::Facet]>, AppError> {
    radio_browser::facets(directory_client(state)?, kind).await
}

#[cfg(test)]
#[path = "tests/radio_tests.rs"]
mod tests;
