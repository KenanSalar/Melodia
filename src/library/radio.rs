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
use crate::media::station_logo::StoredLogo;
use crate::player::stream_source::{self, StationFacts};
use crate::player::types::RadioNowPlaying;
use crate::services::radio_blocklist;
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

/// Drop a station out of the Favorites tab.
///
/// **Un-starring and deleting are the same button on two different rows.** A *directory* station
/// with a play behind it is still in Recently Played and its history is not the star's to take —
/// the argument [`set_directory_favorite`] already makes from the other side. One that was only
/// ever starred is listed nowhere once the star goes, and Browse rewrites it from the directory
/// the moment it is kept again. A hand-typed one has no directory to be rewritten from, so this
/// is its delete either way — see [`is_listed`].
pub async fn remove_from_favorites(state: &AppState, id: i64) -> Result<(), AppError> {
    set_favorite(state, id, false).await?;
    delete_if_unlisted(state, id).await
}

/// Drop a station out of the Recently Played tab.
///
/// The mirror of [`remove_from_favorites`]: forget the plays, and keep the row while a star still
/// lists it somewhere.
pub async fn remove_from_recent(state: &AppState, id: i64) -> Result<(), AppError> {
    queries::radio::clear_play_history(&state.db, id).await?;
    delete_if_unlisted(state, id).await
}

/// Delete a row no tab would list any more.
///
/// The table backs the two local tabs and nothing else, so a station neither of them shows is a
/// row nothing can reach — including the user, who has just removed it from both.
///
/// **Every un-star owes this, not just the trash.** The star and the trash leave a station in the
/// same place; [`set_directory_favorite`] deliberately doesn't decide, so the surface calling it
/// has to, or a browse-and-unstar leaves a row behind on every pass.
pub async fn delete_if_unlisted(state: &AppState, id: i64) -> Result<(), AppError> {
    let station = get_station(state, id).await?;
    if is_listed(&station) {
        return Ok(());
    }
    queries::radio::delete_station(&state.db, id).await
}

/// Whether either local tab would still show a station: the star is Favorites' filter and the
/// stamp is Recently Played's, so between them they are the whole of what a row is kept for.
///
/// **A hand-typed station is listed by its star alone**, whatever it has been played. No directory
/// page names it, so the card offers no star to set (`starrable: station.uuid != ""`) and Browse
/// cannot write the row back. Counting the stamp there leaves it in Recently Played with the one
/// tab that could restore it unable to, which is the row-nothing-can-reach this predicate exists
/// to prevent rather than a milder version of it.
fn is_listed(station: &radio::RadioStation) -> bool {
    if station.station_uuid.as_deref().is_none_or(str::is_empty) {
        return station.is_favorite;
    }
    station.is_favorite || station.last_played.is_some()
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
    // Ahead of both the merge and the probe: a refused station should cost no network
    // and must not star a row it may already have from before it was blocked. **The
    // URL is the only handle worth checking at this door** — the user supplies the
    // name, and a stream announces no country, language or tags to match on.
    if radio_blocklist::blocks(radio_blocklist::StationTerms {
        station_uuid: None,
        name: "",
        stream_url,
        country_code: "",
        language: "",
        codec: "",
        tags: "",
    }) {
        return Err(AppError::Validation("This station can't be added".to_owned()));
    }

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
        // Off the probe rather than assumed: a segment playlist now opens like any other mount,
        // so whether this one was segmented is something only the connect knows.
        hls: facts.hls,
    };

    let saved = save_station(state, &station).await?;
    set_favorite(state, saved.id, true).await?;
    if let Some(logo_url) = facts.logo_url.as_deref() {
        adopt_logo(state, saved.id, logo_url).await;
    }
    Ok(saved.id)
}

/// Record what the user says about a station, and fetch a logo where they named one.
///
/// **The only fields a directory-owned row will take from them**, and it takes them because the
/// directory is community-maintained and frequently partial: an entry carrying no homepage has no
/// website button and no way to grow one, a third of them ship no logo, and nothing is derivable
/// from a stream URL that usually belongs to a streaming provider rather than to the station. They
/// land in the `local_*` columns, which the directory never writes, so they survive the re-import
/// that follows the next play. Everything else about a browsed row stays the directory's, per
/// [`ensure_editable`].
///
/// **The fetch is this door's and not the import's**, and the asymmetry is not an optimization: a
/// station whose row already points at a logo is one [`heal_station_logo`] skips, so a
/// *replacement* URL typed here would never land on its own. Every row an import creates is new
/// and logo-less, so the refresh behind its toast heals the lot four at a time — which is why
/// `radio_files` composes [`validated_overrides`] with the write itself rather than calling this.
pub async fn set_station_overrides(
    state: &AppState,
    id: i64,
    form: &radio::StationOverrides,
) -> Result<(), AppError> {
    let overrides = validated_overrides(form)?;
    queries::radio::set_local_fields(&state.db, id, &overrides).await?;

    if let Some(logo_url) = overrides.logo_url.as_deref() {
        adopt_logo(state, id, logo_url).await;
    }
    Ok(())
}

/// What the four typed fields store as, or the first refusal among them.
///
/// **Everything is checked before anything is written**, so a typo in one URL leaves the whole
/// save untouched rather than half-applied. An empty field clears its column.
pub(super) fn validated_overrides(
    form: &radio::StationOverrides,
) -> Result<radio::StationOverrides, AppError> {
    Ok(radio::StationOverrides {
        website: website_url(form.website.as_deref().unwrap_or_default())?,
        logo_url: website_url(form.logo_url.as_deref().unwrap_or_default())?,
        genre: trimmed(form.genre.as_deref()),
        country: trimmed(form.country.as_deref()),
    })
}

/// A typed free-text field, or `None` where it holds nothing worth storing.
fn trimmed(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|text| !text.is_empty()).map(str::to_owned)
}

/// What to store for a typed website, or a refusal.
///
/// `None` is the empty field, which clears the column. Anything else has to parse as an HTTP(S)
/// URL with a host: the value ends up behind a button that opens the user's browser, so a typo is
/// caught while they are looking at the field rather than handed to `open::that_detached`. The
/// scheme list is [`media::station_logo::fetchable_url`]'s, for the same reason it is there.
///
/// Normalized through `Url` rather than stored as typed, so `nidaa.fm` and a trailing-slash-less
/// spelling of the same site do not read as two different links.
fn website_url(website: &str) -> Result<Option<String>, AppError> {
    let website = website.trim();
    if website.is_empty() {
        return Ok(None);
    }
    reqwest::Url::parse(website)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
        .map(|url| Some(url.to_string()))
        .ok_or_else(|| {
            AppError::Validation("A station website must be an http:// or https:// address".into())
        })
}

/// Refuse to edit a station the directory owns.
///
/// **The refusal is the point rather than a nicety**: `save_station`'s conflict clause rewrites
/// `name` and `stream_url` from the directory on the next favorite or play of the same uuid, so
/// an edit to a browsed station would revert with nothing on screen to say why. The card only
/// offers the full editor on a custom station; this is what holds when something else asks.
/// [`set_station_overrides`] is the deliberate exception and takes no part in it, the four
/// `local_*` columns being the ones the conflict clause yields on.
fn ensure_editable(station: &radio::RadioStation) -> Result<(), AppError> {
    if station.station_uuid.is_none() {
        return Ok(());
    }
    Err(AppError::Validation("Only a station you added by URL can be edited".to_owned()))
}

/// Rewrite a hand-typed station's name and URL.
///
/// The probe runs only when the URL actually moved, so renaming a station that happens to be off
/// air today still works — and when it did move, **everything the old mount said about itself
/// goes with it**, logo included. A repointed station keeping the previous brand's icon and
/// homepage link is the failure `keep_station` already argues against on the directory's side.
/// The `local_*` columns are outside that: a value the user typed is theirs to carry over or
/// clear, and the editor shows it on the form that repointed the station.
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
    if let Some(path) = ask_logo_url(state, logo_url).await {
        adopted(state, id, path).await;
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
    let mut station = get_station(state, id).await?;
    mark_played(state, id).await?;
    // The row was read before the count went in, and this play is one the Now-Playing surfaces
    // should already be stating. `mark_played` is `play_count + 1`, so this is the new value
    // rather than a guess at it.
    station.play_count += 1;
    let now_playing = std::sync::Arc::new(RadioNowPlaying::from(&station));
    // Every play passes through here, whichever surface started it, so the directory's own
    // count is reported once and in one place rather than at each caller.
    spawn_click(state, station.station_uuid.as_deref());
    playback::player_play_station(&state.playback_ctx(), &now_playing).await
}

/// The station a restart should put back on the deck, or nothing.
///
/// Guarded like a play rather than like a getter: what comes back goes on a Now-Playing surface
/// with a button that opens a socket, which is what D15's switch is about.
///
/// Silent on every miss — radio switched off since, or a row removed from both tabs and swept.
/// Nothing has been asked for at this point in the boot, so there is nothing to report to
/// somebody who may not have been thinking about the station at all.
pub async fn station_to_restore(state: &AppState, id: i64) -> Option<Arc<RadioNowPlaying>> {
    if !state.radio_enabled() {
        return None;
    }
    match get_station(state, id).await {
        Ok(station) => Some(Arc::new(RadioNowPlaying::from(&station))),
        Err(e) => {
            log::debug!("radio: not restoring station {id}: {}", crate::services::describe(&e));
            None
        }
    }
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
pub async fn play_directory_station(
    state: &AppState,
    station: &radio::DirectoryStation,
    logo: Option<&str>,
) -> Result<(), AppError> {
    ensure_enabled(state)?;
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

/// Fetch a station's logo into the radio-logo store, returning where it landed.
///
/// Through the same seam as the directory calls: "off" has to mean no traffic, and a logo
/// download is traffic whoever is serving it.
pub async fn fetch_logo(
    state: &AppState,
    favicon_url: &str,
) -> Result<Option<StoredLogo>, AppError> {
    let client = directory_client(state)?;
    crate::media::station_logo::fetch(client, favicon_url, &state.paths.radio_logos_dir).await
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

/// How long a cached browse logo is kept without being asked for again.
///
/// The staleness half of the retention rule, and it dates from when the logo was *fetched* rather
/// than when it was last drawn — nothing on the read path writes, and Linux mount options make
/// access times a thing that may or may not exist. So a station browsed every week still re-fetches
/// once a fortnight, which is the price of not tracking reads and is one request.
const LOGO_CACHE_MAX_AGE_DAYS: i64 = 14;

/// Ceiling on what cached browse logos may occupy.
///
/// **The bound that actually holds.** A TTL alone lets the store run up as far as the user's own
/// browsing rate takes it — a directory page is fifty stations and most of them carry a logo — and
/// a station scrolled past is worth keeping only while it is cheap. Sized so a heavy session's
/// worth of pages survives and a month of them does not.
///
/// **A bound on the cache, not on the disk.** Every hit row is billed, a kept station's among
/// them, and what the cap evicts is the row: the file behind one survives on
/// `radio_stations.artwork_path` and is drawn from the column rather than from here.
const LOGO_CACHE_MAX_BYTES: i64 = 32 * 1024 * 1024;

/// What this install already knows about each of `favicon_urls`.
///
/// Asked about the page in hand rather than about the table, which has no bound worth reading
/// whole — see the query's own note.
pub async fn logo_answers(
    state: &AppState,
    favicon_urls: &[String],
) -> Result<Vec<queries::radio::StoredLogoAnswer>, AppError> {
    queries::radio::logo_answers(&state.db, favicon_urls).await
}

/// Whether a stored answer is still suppressing its URL at `now`.
///
/// The clock comparison lands here rather than in the `WHERE`: the placeholder list is what
/// `chunked_in_query` binds and a second parameter would have to ride ahead of it. Still a string
/// comparison against the same `to_rfc3339` shape both sides are written in.
pub fn answer_is_suppressed(answer: &queries::radio::StoredLogoAnswer, now: &str) -> bool {
    answer.retry_after.as_deref().is_some_and(|retry_after| retry_after > now)
}

/// Hold the answer table to its two bounds, dropping the rows that fall outside them.
///
/// The files those rows named become unreferenced by dropping, so the store follows on the next
/// sweep rather than being touched here — see the query's own note.
pub async fn prune_logo_answers(state: &AppState) -> Result<u64, AppError> {
    let now = chrono::Utc::now();
    let miss_cutoff = now - chrono::TimeDelta::try_days(LOGO_MISS_MAX_AGE_DAYS).unwrap_or_default();
    let hit_cutoff = now - chrono::TimeDelta::try_days(LOGO_CACHE_MAX_AGE_DAYS).unwrap_or_default();
    queries::radio::prune_logo_answers(
        &state.db,
        &miss_cutoff.to_rfc3339(),
        &hit_cutoff.to_rfc3339(),
        LOGO_CACHE_MAX_BYTES,
    )
    .await
}

/// Record that `favicon_url` answered with nothing, pushing its next attempt further out.
pub async fn note_logo_miss(state: &AppState, favicon_url: &str) -> Result<(), AppError> {
    let attempts = queries::radio::logo_miss_attempts(&state.db, favicon_url)
        .await?
        .unwrap_or(0)
        .saturating_add(1);
    let hours = LOGO_MISS_BACKOFF_HOURS * attempts.min(LOGO_MISS_MAX_ATTEMPTS);
    let retry_after = chrono::Utc::now() + chrono::TimeDelta::try_hours(hours).unwrap_or_default();
    queries::radio::record_logo_miss(
        &state.db,
        favicon_url,
        attempts,
        &retry_after.to_rfc3339(),
        &crate::utils::now_rfc3339(),
    )
    .await
}

/// Record where `favicon_url`'s logo landed, so the next session draws it without asking.
///
/// **This is what makes the store a cache.** The file is named by a hash of its own bytes, so
/// nothing can know a URL's path without downloading the bytes first; without the row, every
/// browsed logo was re-fetched on every launch and rewritten identically.
pub async fn note_logo_hit(
    state: &AppState,
    favicon_url: &str,
    logo: &StoredLogo,
) -> Result<(), AppError> {
    queries::radio::record_logo_hit(
        &state.db,
        favicon_url,
        &logo.path,
        i64::try_from(logo.bytes).unwrap_or(i64::MAX),
        &crate::utils::now_rfc3339(),
    )
    .await
}

/// Point a station at its stored logo, or clear it with `None`.
pub async fn set_artwork(
    state: &AppState,
    id: i64,
    artwork_path: Option<&str>,
) -> Result<(), AppError> {
    queries::radio::set_artwork(&state.db, id, artwork_path).await
}

/// Whether a stored artwork path still names a file.
///
/// **A path outlives its file more easily than a row outlives its path.** The store is swept
/// against the columns that reference it, so a logo kept under a data root this build no longer
/// opens looks like an orphan to whichever install sweeps next, and the row is left pointing at
/// nothing. A reader that trusts the column paints an empty tile forever, since nothing in the
/// fetch path ever looks at a station that already has an answer.
pub fn artwork_is_present(artwork_path: Option<&str>) -> bool {
    artwork_path.is_some_and(|path| !path.is_empty() && std::path::Path::new(path).exists())
}

/// Ask one logo URL, and carry the answer back into the answer table.
///
/// **The stored answer is consulted first, hit as well as miss.** The table holds where a URL's
/// bytes landed precisely so nothing has to download them to find out, and a repair that went
/// straight to the socket spent a request re-deriving a path the row beside it already had.
///
/// **`Ok(None)` earns a backoff and an `Err` does not.** The two mean different things — one is
/// the host saying it has nothing usable, the other is not reaching the host at all — and
/// persisting a transport failure would suppress a perfectly good logo for a day over a moment
/// offline. `ui::radio::logos` splits them the same way on the browse path.
async fn ask_logo_url(state: &AppState, url: &str) -> Option<String> {
    match stored_answer(state, url).await {
        Answered::Hit(path) => return Some(path),
        Answered::Suppressed => return None,
        Answered::Unknown => {}
    }
    let logo = match fetch_logo(state, url).await {
        Ok(logo) => logo,
        Err(e) => {
            log::debug!("radio: station logo fetch failed: {}", crate::services::describe(&e));
            return None;
        }
    };
    let recorded = match logo.as_ref() {
        Some(logo) => note_logo_hit(state, url, logo).await,
        None => note_logo_miss(state, url).await,
    };
    if let Err(e) = recorded {
        log::debug!("radio: logo outcome not recorded: {}", crate::services::describe(&e));
    }
    logo.map(|logo| logo.path)
}

/// What an earlier session already settled about one URL.
enum Answered {
    /// A stored file still on disk, so there is nothing to ask.
    Hit(String),
    /// A miss inside the backoff it earned.
    Suppressed,
    /// Never asked, past its backoff, or a hit whose file has since been swept.
    Unknown,
}

/// The stored answer for `url`, read once for both questions it settles.
///
/// One query, because a second would ask the same row the same thing. A hit whose file is gone is
/// [`Answered::Unknown`]: the store is swept against the columns that reference it, and a path
/// naming nothing paints an empty tile where the monogram was the honest answer.
async fn stored_answer(state: &AppState, url: &str) -> Answered {
    let asked = [url.to_owned()];
    let Ok(answers) = logo_answers(state, &asked).await else {
        return Answered::Unknown;
    };
    // The two arms are mutually exclusive on the row — a hit carries no `retry_after` and a miss
    // carries no path — so the order is only what keeps the borrow ahead of the move.
    let now = crate::utils::now_rfc3339();
    for answer in answers {
        if answer_is_suppressed(&answer, &now) {
            return Answered::Suppressed;
        }
        if let Some(path) = answer.artwork_path
            && artwork_is_present(Some(&path))
        {
            return Answered::Hit(path);
        }
    }
    Answered::Unknown
}

/// The site a station's logo is discovered from, and the key its answer is memoized under.
///
/// Re-exported so a caller needing that key doesn't have to name the discovery module: the
/// backoff, the session memo and the fetch all have to agree on one spelling of "this station's
/// site", and there is only ever one place it is derived.
pub use crate::media::logo_discovery::origin_for as site_origin;

/// Give a kept station with no usable logo another chance at one, and point its row at what lands.
///
/// Two sources, in order: the logo URL the row carries, then whatever the station's own site
/// advertises. Returns the store path that landed, so a caller can patch the row it holds.
///
/// **Both come off the resolvers, so a field the user filled in is what this asks about.** That is
/// the whole payoff of letting them record a website for an entry the directory left blank: the
/// site they named is the one read for a `<link rel="icon">`, and a station that had no logo and
/// no way to get one now has both.
pub async fn heal_station_logo(state: &AppState, station: &radio::RadioStation) -> Option<String> {
    if let Some(url) = station.logo_source()
        && let Some(path) = ask_logo_url(state, url).await
    {
        return adopted(state, station.id, path).await;
    }

    let origin = site_origin(station.website().unwrap_or_default(), &station.stream_url)?;
    let path = discover_site_logo(state, &origin).await?;
    adopted(state, station.id, path).await
}

/// The logo a station's own site advertises, past the backoff that rides on the site.
///
/// **The backoff is on the site, not on whichever icon it named this time**: re-reading the
/// document is the expensive half, and a site with nothing to advertise will not have grown
/// something by the next refresh.
///
/// Costs a page fetch on top of the download, which is why the browse side asks only about a
/// result narrow enough to be the station the user typed — a directory page would pay one for the
/// third of its rows that carry no logo field.
pub async fn discover_site_logo(state: &AppState, origin: &reqwest::Url) -> Option<String> {
    match stored_answer(state, origin.as_str()).await {
        Answered::Hit(path) => return Some(path),
        Answered::Suppressed => return None,
        Answered::Unknown => {}
    }
    let landed = match discover_logo_url(state, origin).await {
        Some(url) => ask_logo_url(state, &url).await,
        None => None,
    };
    if landed.is_none() {
        note_site_miss(state, origin.as_str()).await;
    }
    landed
}

/// Point the row at `path`, reporting it only once the write took.
async fn adopted(state: &AppState, id: i64, path: String) -> Option<String> {
    match set_artwork(state, id, Some(&path)).await {
        Ok(()) => Some(path),
        Err(e) => {
            log::debug!("radio: station logo not stored: {}", crate::services::describe(&e));
            None
        }
    }
}

/// Record that a site advertised nothing usable.
async fn note_site_miss(state: &AppState, origin: &str) {
    if let Err(e) = note_logo_miss(state, origin).await {
        log::debug!("radio: site outcome not recorded: {}", crate::services::describe(&e));
    }
}

/// What a station's own site says its logo is, past the switch that turns Radio off.
async fn discover_logo_url(state: &AppState, origin: &reqwest::Url) -> Option<String> {
    let client = directory_client(state).ok()?;
    match crate::media::logo_discovery::icon_url(client, origin).await {
        Ok(url) => url,
        Err(e) => {
            log::debug!("radio: station site not read: {}", crate::services::describe(&e));
            None
        }
    }
}

/// Search the directory. Results are a network answer with a shelf life and are
/// never written to the table; one becomes a row when the user keeps or plays it.
pub async fn search(
    state: &AppState,
    search: &radio::StationSearch,
) -> Result<radio::StationPage, AppError> {
    let mut page = radio_browser::search(directory_client(state)?, search).await?;
    hide_segmented(&mut page, state.radio_hide_segmented());
    Ok(page)
}

/// Drop the segmented stations from a page, if the user has them hidden.
///
/// Here rather than in the request because the endpoint has no `hls` parameter to send. It thins
/// the page without touching [`radio::StationPage::has_more`], which the client already read off
/// the raw response: these rows were served and counted, and paging has to step over them rather
/// than stop at them.
fn hide_segmented(page: &mut radio::StationPage, hide: bool) {
    if hide {
        page.stations.retain(|station| !station.hls);
    }
}

/// What the directory currently says about one station, for the station page.
///
/// Deliberately **additive**: the caller keeps whatever the row it opened from
/// already said and takes only the facts the table has no column for — the
/// state, the popularity figures and the directory's own last check. Letting it
/// overwrite the rest would undo a user's `local_*` override from a background
/// fetch, which is the one thing the split columns exist to prevent.
///
/// `Ok(None)` is a uuid the directory no longer lists.
pub async fn station_details(
    state: &AppState,
    station_uuid: &str,
) -> Result<Option<radio::DirectoryStation>, AppError> {
    radio_browser::station_by_uuid(directory_client(state)?, station_uuid).await
}

/// Vote for a station, which is how its popularity ordering stays meaningful.
///
/// No opt-out of its own, unlike the play click: a vote happens only because
/// somebody pressed a button that says so, where the click rides every play and
/// is therefore the one that needs a setting. The master switch still covers it.
pub async fn vote(state: &AppState, station_uuid: &str) -> Result<(), AppError> {
    radio_browser::cast_vote(directory_client(state)?, station_uuid).await
}

/// One of the directory's facet lists, for the filter chips. Large and
/// near-static, so it is fetched once per session and shared thereafter.
pub async fn facets(
    state: &AppState,
    kind: radio::FacetKind,
) -> Result<Arc<[radio::Facet]>, AppError> {
    let facets = radio_browser::facets(directory_client(state)?, kind).await?;
    Ok(hide_segmented_codecs(facets, kind, state.radio_hide_segmented()))
}

/// Drop the codecs that only ever name a segmented stream, if the user has those hidden.
///
/// [`hide_segmented`]'s counterpart on the chip. The directory counts every station its checker
/// saw, so a Format list built from those counts otherwise offers filters whose entire result the
/// page thins away: `UNKNOWN` is what the checker writes when it could not read a playlist at all,
/// and a comma means it found a picture track beside the audio.
///
/// Filtered here rather than in `radio_browser`, whose cell holds one list per session and must
/// not bake a setting into it, and the input is handed back untouched for every other kind: the
/// tag list runs to tens of thousands of entries and this is called on every chip open.
fn hide_segmented_codecs(
    facets: Arc<[radio::Facet]>,
    kind: radio::FacetKind,
    hide: bool,
) -> Arc<[radio::Facet]> {
    if !hide || kind != radio::FacetKind::Codecs {
        return facets;
    }
    facets.iter().filter(|facet| !names_segmented(&facet.name)).cloned().collect()
}

/// Codec names the directory only ever writes for a stream nothing can play as one continuous
/// mount. `MP4` is spelled out because it is a container rather than a codec and every station
/// under it is flagged segmented.
fn names_segmented(codec: &str) -> bool {
    codec.contains(',')
        || codec.eq_ignore_ascii_case(radio::UNKNOWN_CODEC)
        || codec.eq_ignore_ascii_case("MP4")
}

#[cfg(test)]
#[path = "tests/radio_tests.rs"]
mod tests;
