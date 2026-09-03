//! Client for the [radio-browser.info](https://www.radio-browser.info) station
//! directory: mirror discovery, station search, and the facet lists the filter
//! chips are built from.
//!
//! Takes the shared `reqwest::Client` rather than building one. The directory
//! asks callers for a descriptive `User-Agent` of the form `appname/appversion`
//! and `services::net::build_http_client` already sends `Melodia/<version>`, so
//! reusing it is what satisfies the request as well as what shares the pool.
//!
//! [`model`] holds the wire shapes and [`query`] every parameter name; both are
//! pure, leaving the sending as the only part of this module that needs a
//! socket. Nothing outside `library::radio` should reach here — that facade is
//! where the setting that turns radio off is enforced.

mod model;
mod query;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rand::RngExt;
use tokio::sync::OnceCell;

use crate::entities::radio::{DirectoryStation, Facet, FacetKind, StationPage, StationSearch};
use crate::error::AppError;
use crate::services::net::radio_blocklist;
use model::{ApiFacet, ApiServer, ApiStation, ApiVote};

pub use query::DEFAULT_PAGE_LIMIT;

/// Where the mirror list lives. One name in front of every mirror, so it needs
/// no discovery of its own.
const SERVERS_URL: &str = "https://all.api.radio-browser.info/json/servers";

/// The mirror to talk to when discovery fails.
///
/// A bare hostname with no scheme and no trailing slash, so [`endpoint`] stays
/// the single place a URL is spelled.
const FALLBACK_HOST: &str = "de1.api.radio-browser.info";

/// The mirror this session talks to.
static MIRROR: OnceCell<String> = OnceCell::const_new();

/// Whole-request ceiling for one directory call, connect included.
///
/// The shared client bounds a *read* and not a request, which is what the
/// updater's multi-minute downloads want and the opposite of what a fetch
/// somebody is waiting on wants: a mirror trickling a byte at a time never
/// trips a per-read deadline. Wide enough to clear the client's own
/// ten-second connect timeout and still transfer a facet list behind it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Ceiling on a directory answer. The widest of them is the full country or language facet list,
/// which is thousands of short objects; a station page is `PAGE_SIZE` rows and nowhere near it.
/// Generous because a mirror is free to add fields, bounded because a mirror is a third party.
const DIRECTORY_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Search the directory.
pub async fn search(
    client: &reqwest::Client,
    search: &StationSearch,
) -> Result<StationPage, AppError> {
    let url = endpoint(client, "stations/search").await;
    let stations: Vec<ApiStation> =
        get_json(client, &url, &query::search_params(search), "search").await?;
    Ok(page_from(stations, search.limit))
}

/// Project one response onto a page, dropping the rows nothing can use and the rows
/// this build refuses to show.
///
/// Rows failing [`DirectoryStation::is_usable`] are dropped here rather than at
/// the surface, so a page can come back shorter than the limit it asked for. Hence
/// [`StationPage::has_more`] off the **raw** response length: read off what
/// survived the drop, a full page holding one uuid-less station would report the
/// end of the directory and paging would stop there. `radio_blocklist` thins the
/// same page for the same reason and needs the same care — a blocked country can
/// empty a page the directory did serve, and paging has to step over it.
fn page_from(stations: Vec<ApiStation>, limit: u32) -> StationPage {
    let full_page = usize::try_from(query::page_limit(limit)).unwrap_or(usize::MAX);
    let has_more = stations.len() >= full_page;
    StationPage {
        stations: stations
            .into_iter()
            .map(ApiStation::into_directory_station)
            .filter(DirectoryStation::is_usable)
            .filter(|station| !radio_blocklist::blocks(station))
            .collect(),
        has_more,
    }
}

/// Tell the directory a station was played, which is what its popularity
/// ordering is built from.
///
/// Checked on **status and never parsed**: an unknown station comes back as a
/// 404 with a zero-length body, so there is nothing to deserialize and asking
/// for JSON turns a clean refusal into a parse error. (Its sibling
/// `/json/vote/{uuid}` is the other way round, answering 200 with
/// `{"ok":false}`, which is why neither can borrow the other's guard.)
///
/// Deduplicated server-side at one click per IP per station per day, so a
/// repeated call is not an error and needs no client-side debounce.
pub async fn count_click(client: &reqwest::Client, station_uuid: &str) -> Result<(), AppError> {
    let url = endpoint(client, &format!("url/{station_uuid}")).await;
    let response = client
        .get(&url)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| AppError::network("Radio directory click request failed", e))?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    Err(AppError::network_msg(format!("Radio directory click returned HTTP {status}")))
}

/// One station as the directory currently describes it.
///
/// `Ok(None)` for a uuid the directory does not know, which is a station that
/// was withdrawn rather than a failure to report: the endpoint answers an empty
/// array for it, and a row kept locally outlives its directory entry by design.
/// A blocked station answers the same way on purpose — every caller already
/// handles a page that no longer exists, and none has to learn a second shape.
///
/// This is what a kept station's page is refreshed from. The table has no column
/// for the popularity figures or the directory's own last check, so without a
/// call here the same station would state fewer facts opened from Favorites than
/// opened from Browse.
pub async fn station_by_uuid(
    client: &reqwest::Client,
    station_uuid: &str,
) -> Result<Option<DirectoryStation>, AppError> {
    if !model::is_path_safe_uuid(station_uuid) {
        return Err(AppError::Validation("Not a radio directory station id".to_owned()));
    }
    let url = endpoint(client, &format!("stations/byuuid/{station_uuid}")).await;
    let stations: Vec<ApiStation> =
        get_json(client, &url, &BTreeMap::new(), "station lookup").await?;

    Ok(stations
        .into_iter()
        .map(ApiStation::into_directory_station)
        .find(|station| station.is_usable() && !radio_blocklist::blocks(station)))
}

/// Vote for a station.
///
/// **Checked on the body, never on the status**, which is the opposite of
/// [`count_click`] and the reason neither can borrow the other's guard: this
/// endpoint reports a refusal as `200 {"ok":false}`.
///
/// Deduplicated server-side at one vote per station per ten minutes, so a second
/// press inside that window is refused rather than counted — a real answer to
/// report, not an error to swallow.
pub async fn cast_vote(client: &reqwest::Client, station_uuid: &str) -> Result<(), AppError> {
    if !model::is_path_safe_uuid(station_uuid) {
        return Err(AppError::Validation("Not a radio directory station id".to_owned()));
    }
    let url = endpoint(client, &format!("vote/{station_uuid}")).await;
    let vote: ApiVote = get_json(client, &url, &BTreeMap::new(), "vote").await?;

    if vote.ok {
        return Ok(());
    }
    Err(AppError::network_msg(format!("Radio directory refused the vote: {}", vote.message)))
}

/// One of the directory's facet lists, fetched once per session.
pub async fn facets(client: &reqwest::Client, kind: FacetKind) -> Result<Arc<[Facet]>, AppError> {
    facet_cell(kind).get_or_try_init(|| fetch_facets(client, kind)).await.cloned()
}

/// The session slot for one facet list.
///
/// Four cells rather than a map behind a lock: the set is closed, and a cell
/// cannot be caught holding a guard across the fetch it is waiting on.
fn facet_cell(kind: FacetKind) -> &'static OnceCell<Arc<[Facet]>> {
    static COUNTRIES: OnceCell<Arc<[Facet]>> = OnceCell::const_new();
    static LANGUAGES: OnceCell<Arc<[Facet]>> = OnceCell::const_new();
    static TAGS: OnceCell<Arc<[Facet]>> = OnceCell::const_new();
    static CODECS: OnceCell<Arc<[Facet]>> = OnceCell::const_new();

    match kind {
        FacetKind::Countries => &COUNTRIES,
        FacetKind::Languages => &LANGUAGES,
        FacetKind::Tags => &TAGS,
        FacetKind::Codecs => &CODECS,
    }
}

async fn fetch_facets(client: &reqwest::Client, kind: FacetKind) -> Result<Arc<[Facet]>, AppError> {
    let path = query::facet_path(kind);
    let url = endpoint(client, path).await;
    let facets: Vec<ApiFacet> = get_json(client, &url, &query::facet_params(kind), path).await?;

    // A ceiling is not optional — omitting the parameter takes the directory's own
    // default slice rather than everything — so the only question is whether hitting
    // one is silent. Read off the **raw** length, before the blocklist thins it, for
    // `page_from`'s reason. A cut list is a degradation nothing else can report: the
    // missing tags are simply not offered, and no surface looks wrong.
    if facets.len() >= query::facet_limit(kind) as usize {
        log::warn!("radio: the {path} list came back at its ceiling and is likely cut short");
    }

    // Filtered before the list is frozen into its cell, so the cost lands once a
    // session rather than on every chip open, and every consumer of the cached list
    // — the pickers, their needle filter and the scope pills — is covered by it.
    Ok(facets
        .into_iter()
        .map(ApiFacet::into_facet)
        .filter(|facet| !radio_blocklist::facet_is_blocked(kind, facet))
        .collect())
}

/// A full URL for one directory path, against this session's mirror.
async fn endpoint(client: &reqwest::Client, path: &str) -> String {
    url_for(mirror(client).await, path)
}

/// The one place a scheme and a separator are spelled, which is what lets
/// [`FALLBACK_HOST`] and the discovered hosts both stay bare names.
fn url_for(host: &str, path: &str) -> String {
    format!("https://{host}/json/{path}")
}

/// The host to dial, resolved on the first call and kept for the session.
///
/// `tokio`'s cell rather than the `OnceLock<Mutex<Option<_>>>` used elsewhere in
/// `services`, because the initialiser is `async` and `await_holding_lock` is
/// denied.
///
/// A failed discovery pins the fallback rather than leaving the cell empty to
/// retry: retrying puts a second connect timeout in front of every request made
/// while the network is down, and doubling the wait to re-derive a host is a bad
/// trade when the fallback is a working mirror rather than a placeholder.
async fn mirror(client: &reqwest::Client) -> &'static str {
    MIRROR
        .get_or_init(|| async {
            match discover_mirror(client).await {
                Ok(host) => host,
                Err(e) => {
                    log::warn!(
                        "Radio directory mirror discovery failed ({}); using \
                         {FALLBACK_HOST} for this session",
                        crate::error::describe(&e)
                    );
                    FALLBACK_HOST.to_owned()
                }
            }
        })
        .await
        .as_str()
}

/// Ask for the mirror list and pick one of its hosts.
async fn discover_mirror(client: &reqwest::Client) -> Result<String, AppError> {
    let servers: Vec<ApiServer> =
        get_json(client, SERVERS_URL, &BTreeMap::new(), "mirror list").await?;

    let mut hosts = model::hosts(&servers);
    if hosts.is_empty() {
        return Err(AppError::network_msg("Radio directory mirror list was empty"));
    }
    // Guarded above: `random_range` panics on an empty range.
    let picked = rand::rng().random_range(..hosts.len());
    Ok(hosts.swap_remove(picked))
}

/// One GET, with the status guard every call here shares.
///
/// A non-success status is an `Err` rather than an empty result: search and the
/// facet lists both report failure by status, and folding one into "no stations
/// matched" would show an empty grid for an outage.
async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    params: &BTreeMap<&'static str, String>,
    what: &str,
) -> Result<T, AppError> {
    let response = client
        .get(url)
        .query(params)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| AppError::network(format!("Radio directory {what} request failed"), e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(AppError::network_msg(format!(
            "Radio directory {what} returned HTTP {status}"
        )));
    }

    let body = crate::services::net::read_capped(
        response,
        &format!("Radio directory {what}"),
        DIRECTORY_MAX_BYTES,
    )
    .await?;

    serde_json::from_slice(&body)
        .map_err(|e| AppError::network(format!("Failed to parse radio directory {what}"), e))
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
