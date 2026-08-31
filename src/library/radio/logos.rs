//! Where a station's logo comes from, and the answer table that stops it being asked for twice.
//!
//! Three sources in order of cost: a path an earlier session already recorded, the `favicon_url`
//! the row carries, and the station's own site. The table is what makes the store a cache — a
//! logo's file is named by a hash of its own bytes, so nothing can know a URL's path without
//! downloading the bytes first, and before the table a re-browse re-fetched every logo on the page
//! and rewrote the identical file.
//!
//! **The constants here are retention policy rather than tuning**, each argued at its own
//! definition. What they share is that a miss costs a request and a hit costs nothing, so they are
//! about not asking a dead host on a schedule rather than about bandwidth.

use crate::database::queries;
use crate::entities::radio;
use crate::error::AppError;
use crate::media::station_logo::StoredLogo;
use crate::state::AppState;

use super::directory_client;

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
) -> Result<Vec<radio::StoredLogoAnswer>, AppError> {
    queries::radio::logo_answers(&state.db, favicon_urls).await
}

/// Whether a stored answer is still suppressing its URL at `now`.
///
/// The clock comparison lands here rather than in the `WHERE`: the placeholder list is what
/// `chunked_in_query` binds and a second parameter would have to ride ahead of it. Still a string
/// comparison against the same `to_rfc3339` shape both sides are written in.
pub fn answer_is_suppressed(answer: &radio::StoredLogoAnswer, now: &str) -> bool {
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

/// Carry one URL's answer back to the table the next session reads, whichever it was.
///
/// **The one writer of an outcome**, and the reason the two `note_logo_*` halves below it are
/// private: both fetch paths — this module's own single-URL repair and `ui::radio::logos`' page
/// burst — owe exactly this pair, and had a copy each down to the debug line.
///
/// A hit stores its path, which is what lets that session draw the logo without asking; it also
/// clears whatever backoff the URL had earned, or a host that recovered would stay suppressed
/// until a schedule from when it was down finally ran out.
///
/// Failing to record is a debug line rather than an error: the row is an optimization over asking
/// again, and asking again is exactly what its absence causes.
pub async fn record_logo_outcome(state: &AppState, favicon_url: &str, logo: Option<&StoredLogo>) {
    let recorded = match logo {
        Some(logo) => note_logo_hit(state, favicon_url, logo).await,
        None => note_logo_miss(state, favicon_url).await,
    };
    if let Err(e) = recorded {
        log::debug!("radio: logo outcome not recorded: {}", crate::services::describe(&e));
    }
}

/// Record that `favicon_url` answered with nothing, pushing its next attempt further out.
async fn note_logo_miss(state: &AppState, favicon_url: &str) -> Result<(), AppError> {
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
async fn note_logo_hit(
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
pub(super) async fn ask_logo_url(state: &AppState, url: &str) -> Option<String> {
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
    record_logo_outcome(state, url, logo.as_ref()).await;
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
/// **A newtype over the `reqwest::Url` rather than the URL itself**, so a caller can hold one
/// without naming the transport crate. `ui::radio::logos` builds a page's worth of these per
/// browse and was the whole of `src/ui/` that reached for `reqwest`; the facade is meant to be the
/// seam that ends at HTTP, and a `Vec<reqwest::Url>` in a view slice is that seam leaking.
///
/// The backoff, the session memo and the fetch all have to agree on one spelling of "this
/// station's site", which is what [`Self::as_str`] is — and there is only ever one place it is
/// derived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteOrigin(reqwest::Url);

impl SiteOrigin {
    /// The key every one of the three agrees on.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// The site to ask about a station, from whatever the row carries. `None` where neither the
/// homepage nor the stream URL names a host.
pub fn site_origin(homepage: &str, stream_url: &str) -> Option<SiteOrigin> {
    crate::media::logo_discovery::origin_for(homepage, stream_url).map(SiteOrigin)
}

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
pub async fn discover_site_logo(state: &AppState, origin: &SiteOrigin) -> Option<String> {
    match stored_answer(state, origin.as_str()).await {
        Answered::Hit(path) => return Some(path),
        Answered::Suppressed => return None,
        Answered::Unknown => {}
    }
    let landed = match discover_logo_url(state, &origin.0).await {
        Some(url) => ask_logo_url(state, &url).await,
        None => None,
    };
    if landed.is_none() {
        note_site_miss(state, origin.as_str()).await;
    }
    landed
}

/// Point the row at `path`, reporting it only once the write took.
pub(super) async fn adopted(state: &AppState, id: i64, path: String) -> Option<String> {
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
