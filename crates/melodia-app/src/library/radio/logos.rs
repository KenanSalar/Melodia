//! Where a station's logo comes from, and the answer table that stops it being asked for twice.
//!
//! Three sources in order of cost: a path an earlier session recorded, the row's `favicon_url`,
//! and the station's own site. **The constants are retention policy rather than tuning**: a miss
//! costs a request and a hit costs nothing, so they are about not asking a dead host on a schedule.

use crate::state::AppState;
use melodia_artwork::media::image::logo_tile;
use melodia_core::entities::radio::{self, StoredLogo};
use melodia_core::error::AppError;
use melodia_net::media::fetch::station_logo;
use melodia_store::database::{DbPool, queries};

use super::directory_client;

/// Fetch a station's logo into the radio-logo store, returning where it landed.
///
/// `Ok(None)` is a usable answer with no logo in it (not an image, an unsupported container, or
/// too small to draw); `Err` is a failure worth retrying later.
pub async fn fetch_logo(
    state: &AppState,
    favicon_url: &str,
) -> Result<Option<StoredLogo>, AppError> {
    let client = directory_client(state)?;
    let Some(fetched) = station_logo::fetch(client, favicon_url).await? else {
        return Ok(None);
    };
    let dir = state.paths.radio_logos_dir.clone();
    tokio::task::spawn_blocking(move || logo_tile::store(&fetched.bytes, fetched.extension, &dir))
        .await
        .map_err(AppError::io_source)
}

/// How long a logo URL that answered with nothing is left alone, per failed attempt. A day, so a
/// host down for an afternoon is asked again the next time the user opens Radio.
const LOGO_MISS_BACKOFF_HOURS: i64 = 24;

/// Ceiling on that multiplier: a week between attempts already rounds to never for anyone but a
/// user who leaves the app open.
const LOGO_MISS_MAX_ATTEMPTS: i64 = 7;

/// Age past which a miss is dropped rather than escalated further. Deliberately clear of
/// `LOGO_MISS_BACKOFF_HOURS × LOGO_MISS_MAX_ATTEMPTS`, so nothing is pruned while it still
/// suppresses anything.
const LOGO_MISS_MAX_AGE_DAYS: i64 = 30;

/// How long a cached browse logo is kept without being asked for again, dated from its fetch:
/// nothing on the read path writes, and access times depend on mount options.
const LOGO_CACHE_MAX_AGE_DAYS: i64 = 14;

/// Ceiling on cached browse logos, sized so a heavy session's pages survive and a month's don't.
/// **A bound on the cache, not on the disk**: it evicts rows, and a kept station's file survives on
/// `radio_stations.artwork_path`.
const LOGO_CACHE_MAX_BYTES: i64 = 32 * 1024 * 1024;

/// What this install already knows about each of `favicon_urls`. Asked about the page in hand,
/// the table having no bound worth reading whole.
pub async fn logo_answers(
    db: &DbPool,
    favicon_urls: &[String],
) -> Result<Vec<radio::StoredLogoAnswer>, AppError> {
    queries::radio::logo_answers(db, favicon_urls).await
}

/// Whether a stored answer still suppresses its URL at `now`. Compared here, a second bind in the
/// `WHERE` having to ride ahead of the placeholder list `chunked_in_query` binds.
fn answer_is_suppressed(answer: &radio::StoredLogoAnswer, now: &str) -> bool {
    answer.retry_after.as_deref().is_some_and(|retry_after| retry_after > now)
}

/// Hold the answer table to its two bounds; the files dropped rows named go on the next sweep. Not
/// behind the switch, there being no network behind it.
pub async fn prune_logo_answers(db: &DbPool) -> Result<u64, AppError> {
    let now = chrono::Utc::now();
    let miss_cutoff = now - chrono::TimeDelta::try_days(LOGO_MISS_MAX_AGE_DAYS).unwrap_or_default();
    let hit_cutoff = now - chrono::TimeDelta::try_days(LOGO_CACHE_MAX_AGE_DAYS).unwrap_or_default();
    queries::radio::prune_logo_answers(
        db,
        &miss_cutoff.to_rfc3339(),
        &hit_cutoff.to_rfc3339(),
        LOGO_CACHE_MAX_BYTES,
    )
    .await
}

/// Carry one URL's answer back to the table the next session reads: **the one writer of an
/// outcome**, for this module's repair and `ui::radio::logos`' page burst alike. A hit also clears
/// the URL's backoff, or a recovered host would stay suppressed. Failing to record is a debug line,
/// the row only saving a request.
pub async fn record_logo_outcome(db: &DbPool, favicon_url: &str, logo: Option<&StoredLogo>) {
    let recorded = match logo {
        Some(logo) => note_logo_hit(db, favicon_url, logo).await,
        None => note_logo_miss(db, favicon_url).await,
    };
    if let Err(e) = recorded {
        log::debug!("radio: logo outcome not recorded: {}", melodia_core::error::describe(&e));
    }
}

fn miss_backoff_hours(attempts: i64) -> i64 {
    LOGO_MISS_BACKOFF_HOURS * attempts.min(LOGO_MISS_MAX_ATTEMPTS)
}

/// Record that `favicon_url` answered with nothing, pushing its next attempt further out.
async fn note_logo_miss(db: &DbPool, favicon_url: &str) -> Result<(), AppError> {
    let attempts =
        queries::radio::logo_miss_attempts(db, favicon_url).await?.unwrap_or(0).saturating_add(1);
    let hours = miss_backoff_hours(attempts);
    let retry_after = chrono::Utc::now() + chrono::TimeDelta::try_hours(hours).unwrap_or_default();
    queries::radio::record_logo_miss(
        db,
        favicon_url,
        attempts,
        &retry_after.to_rfc3339(),
        &melodia_core::utils::now_rfc3339(),
    )
    .await
}

/// Record where `favicon_url`'s logo landed, so the next session draws it without asking.
///
/// **This is what makes the store a cache.** The file is named by a hash of its own bytes, so
/// nothing can know a URL's path without downloading them first.
async fn note_logo_hit(db: &DbPool, favicon_url: &str, logo: &StoredLogo) -> Result<(), AppError> {
    queries::radio::record_logo_hit(
        db,
        favicon_url,
        &logo.path,
        i64::try_from(logo.bytes).unwrap_or(i64::MAX),
        &melodia_core::utils::now_rfc3339(),
    )
    .await
}

/// Point a station at its stored logo, or clear it with `None`.
pub async fn set_artwork(db: &DbPool, id: i64, artwork_path: Option<&str>) -> Result<(), AppError> {
    queries::radio::set_artwork(db, id, artwork_path).await
}

/// Whether a stored artwork path still names a file. A logo under a data root another install
/// sweeps can be gone, and trusting the column paints an empty tile the fetch path never revisits.
pub fn artwork_is_present(artwork_path: Option<&str>) -> bool {
    artwork_path.is_some_and(|path| !path.is_empty() && std::path::Path::new(path).exists())
}

/// Ask one logo URL, consulting the stored answer first, and carry what comes back into the table.
///
/// **`Ok(None)` earns a backoff and an `Err` does not**: one is the host saying it has nothing
/// usable, the other is not reaching it, and persisting that would suppress a good logo for a day
/// over a moment offline. `ui::radio::logos` splits them the same way.
pub(super) async fn ask_logo_url(state: &AppState, seed: &AnswerSeed, url: &str) -> Option<String> {
    match stored_answer(&state.db, seed, url).await {
        LogoAnswer::Hit(path) => return Some(path),
        LogoAnswer::Suppressed => return None,
        LogoAnswer::Unknown => {}
    }
    let logo = match fetch_logo(state, url).await {
        Ok(logo) => logo,
        Err(e) => {
            log::debug!("radio: station logo fetch failed: {}", melodia_core::error::describe(&e));
            return None;
        }
    };
    record_logo_outcome(&state.db, url, logo.as_ref()).await;
    logo.map(|logo| logo.path)
}

/// What an earlier session already settled about one URL. Public so the browse page asks it
/// through [`classify_logo_answer`] rather than deciding it a second time.
pub enum LogoAnswer {
    /// A stored file still on disk, so there is nothing to ask.
    Hit(String),
    /// A miss inside the backoff it earned.
    Suppressed,
    /// Never asked, past its backoff, or a hit whose file has since been swept.
    Unknown,
}

/// Stored answers already in hand, so a pass over many stations asks once rather than per station.
///
/// **The heal pass is who wants it.** Each station settles against the table twice before the
/// network, its row's URL then its site, and one URL at a time those are one-element batched
/// queries that rebuild their SQL and prepare non-persistent.
#[derive(Default)]
pub struct AnswerSeed(std::collections::HashMap<String, radio::StoredLogoAnswer>);

impl AnswerSeed {
    /// For a caller settling a single URL: every lookup falls through to its own query.
    #[must_use]
    pub fn unseeded() -> Self {
        Self::default()
    }

    /// Every stored answer among `urls`, in one query. Duplicates and blanks cost nothing.
    pub async fn for_urls(db: &DbPool, urls: &[String]) -> Self {
        let Ok(answers) = logo_answers(db, urls).await else {
            return Self::default();
        };
        Self(answers.into_iter().map(|answer| (answer.favicon_url.clone(), answer)).collect())
    }
}

/// The stored answer for `url`, from `seed` where it holds one and from its own query otherwise.
/// Takes the pool rather than the state so a test can read the seed against an empty table.
async fn stored_answer(db: &DbPool, seed: &AnswerSeed, url: &str) -> LogoAnswer {
    if let Some(answer) = seed.0.get(url) {
        return classify_logo_answer(answer, &melodia_core::utils::now_rfc3339());
    }
    let asked = [url.to_owned()];
    let Ok(answers) = queries::radio::logo_answers(db, &asked).await else {
        return LogoAnswer::Unknown;
    };
    let now = melodia_core::utils::now_rfc3339();
    answers.first().map_or(LogoAnswer::Unknown, |answer| classify_logo_answer(answer, &now))
}

/// What one stored row says. A hit carries no `retry_after` and a miss no path, so the arms can't
/// overlap; a hit whose file has been swept is [`LogoAnswer::Unknown`], the monogram being the
/// honest answer there.
#[must_use]
pub fn classify_logo_answer(answer: &radio::StoredLogoAnswer, now: &str) -> LogoAnswer {
    if answer_is_suppressed(answer, now) {
        return LogoAnswer::Suppressed;
    }
    match &answer.artwork_path {
        Some(path) if artwork_is_present(Some(path)) => LogoAnswer::Hit(path.clone()),
        _ => LogoAnswer::Unknown,
    }
}

/// The site a station's logo is discovered from, and the key its answer is memoized under.
///
/// A newtype so `ui::radio::logos` can hold a page's worth without naming `reqwest`, the facade
/// being the seam that ends at HTTP. [`Self::as_str`] is the one spelling of "this station's site"
/// the backoff, the session memo and the fetch agree on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteOrigin(reqwest::Url);

impl SiteOrigin {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// The site to ask about a station, from whatever the row carries. `None` where neither the
/// homepage nor the stream URL names a host.
pub fn site_origin(homepage: &str, stream_url: &str) -> Option<SiteOrigin> {
    melodia_net::media::fetch::logo_discovery::origin_for(homepage, stream_url).map(SiteOrigin)
}

/// Give a kept station with no usable logo another chance at one, and point its row at what lands:
/// the row's logo URL, then what its site advertises, read through the resolvers so a website the
/// user typed is the one asked.
pub async fn heal_station_logo(
    state: &AppState,
    seed: &AnswerSeed,
    station: &radio::RadioStation,
) -> Option<String> {
    if let Some(url) = station.logo_source()
        && let Some(path) = ask_logo_url(state, seed, url).await
    {
        return adopted(&state.db, station.id, path).await;
    }

    let origin = site_origin(station.website().unwrap_or_default(), &station.stream_url)?;
    let path = discover_site_logo(state, seed, &origin).await?;
    adopted(&state.db, station.id, path).await
}

/// The URLs [`heal_station_logo`] settles against the table before the network, in its order. Not
/// the logo a site advertises, known only once its page has been read.
#[must_use]
pub fn heal_seed_urls(stations: &[radio::RadioStation]) -> Vec<String> {
    let mut urls = Vec::with_capacity(stations.len() * 2);
    for station in stations {
        urls.extend(station.logo_source().map(str::to_owned));
        urls.extend(
            site_origin(station.website().unwrap_or_default(), &station.stream_url)
                .map(|origin| origin.as_str().to_owned()),
        );
    }
    urls
}

/// The logo a station's own site advertises. **The backoff rides on the site**, re-reading its
/// page being the expensive half, which is also why browse asks only about a result narrow enough
/// to be the station the user typed.
pub async fn discover_site_logo(
    state: &AppState,
    seed: &AnswerSeed,
    origin: &SiteOrigin,
) -> Option<String> {
    match stored_answer(&state.db, seed, origin.as_str()).await {
        LogoAnswer::Hit(path) => return Some(path),
        LogoAnswer::Suppressed => return None,
        LogoAnswer::Unknown => {}
    }
    // The seed couldn't have named the discovered URL, so it settles on its own query.
    let landed = match discover_logo_url(state, &origin.0).await {
        Some(url) => ask_logo_url(state, &AnswerSeed::unseeded(), &url).await,
        None => None,
    };
    if landed.is_none() {
        note_site_miss(&state.db, origin.as_str()).await;
    }
    landed
}

/// Point the row at `path`, reporting it only once the write took.
pub(super) async fn adopted(db: &DbPool, id: i64, path: String) -> Option<String> {
    match set_artwork(db, id, Some(&path)).await {
        Ok(()) => Some(path),
        Err(e) => {
            log::debug!("radio: station logo not stored: {}", melodia_core::error::describe(&e));
            None
        }
    }
}

async fn note_site_miss(db: &DbPool, origin: &str) {
    if let Err(e) = note_logo_miss(db, origin).await {
        log::debug!("radio: site outcome not recorded: {}", melodia_core::error::describe(&e));
    }
}

async fn discover_logo_url(state: &AppState, origin: &reqwest::Url) -> Option<String> {
    let client = directory_client(state).ok()?;
    match melodia_net::media::fetch::logo_discovery::icon_url(client, origin).await {
        Ok(url) => url,
        Err(e) => {
            log::debug!("radio: station site not read: {}", melodia_core::error::describe(&e));
            None
        }
    }
}

#[cfg(test)]
#[path = "../tests/radio_logos_tests.rs"]
mod tests;
