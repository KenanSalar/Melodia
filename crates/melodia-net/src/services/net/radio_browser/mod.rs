//! Client for the [radio-browser.info](https://www.radio-browser.info) station directory: mirror
//! discovery, station search, and the facet lists the filter chips are built from.
//!
//! Takes the shared client, whose `Melodia/<version>` is the descriptive `User-Agent` the
//! directory asks for. Nothing outside `library::radio` should reach here: that facade is where
//! the setting that turns radio off is enforced.

mod model;
mod query;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use rand::RngExt;
use tokio::sync::OnceCell;

use crate::services::net::radio_blocklist;
use melodia_core::entities::radio::{
    DirectoryStation, Facet, FacetKind, StationPage, StationSearch,
};
use melodia_core::error::AppError;
use model::{ApiFacet, ApiServer, ApiStation, ApiVote};

/// The mirror list, behind one name so it needs no discovery of its own.
const SERVERS_URL: &str = "https://all.api.radio-browser.info/json/servers";

/// For when discovery fails. A bare host, [`url_for`] being the one place a URL is spelled.
const FALLBACK_HOST: &str = "de1.api.radio-browser.info";

static MIRROR: OnceCell<String> = OnceCell::const_new();

/// Whole-request ceiling: the shared client bounds a read, which a mirror trickling a byte at a
/// time never trips. Wide enough to clear its ten-second connect and still fetch a facet list.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// The widest answer is the full country or language facet list, thousands of short objects.
/// Generous because a mirror may add fields, bounded because a mirror is a third party.
const DIRECTORY_MAX_BYTES: u64 = 16 * 1024 * 1024;

pub async fn search(
    client: &reqwest::Client,
    search: &StationSearch,
) -> Result<StationPage, AppError> {
    search_at(client, &endpoint(client, "stations/search").await, search).await
}

async fn search_at(
    client: &reqwest::Client,
    url: &str,
    search: &StationSearch,
) -> Result<StationPage, AppError> {
    let stations: Vec<ApiStation> =
        get_json(client, url, &query::search_params(search), "search").await?;
    Ok(page_from(stations, search.limit))
}

/// One response as a page, without the rows nothing can use or this build won't show.
/// [`StationPage::has_more`] comes off the **raw** length, so a full page thinned to nothing
/// doesn't read as the end of the directory.
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

/// Tells the directory a station was played, which its popularity ordering is built from.
///
/// Checked on **status, never parsed**: an unknown station is a `404` with an empty body, the
/// opposite of [`cast_vote`]. Deduplicated server-side per IP per station per day, so a repeat
/// needs no client-side debounce.
pub async fn count_click(client: &reqwest::Client, station_uuid: &str) -> Result<(), AppError> {
    let url = endpoint(client, &format!("url/{station_uuid}")).await;
    count_click_at(client, &url).await
}

async fn count_click_at(client: &reqwest::Client, url: &str) -> Result<(), AppError> {
    let response = client
        .get(url)
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

/// One station as the directory currently describes it, which a kept station's page refreshes
/// its popularity and check figures from, the table having no columns for them.
///
/// `Ok(None)` for a uuid the directory has withdrawn (it answers an empty array), and for one
/// this build won't show, so no caller learns a second shape.
pub async fn station_by_uuid(
    client: &reqwest::Client,
    station_uuid: &str,
) -> Result<Option<DirectoryStation>, AppError> {
    if !model::is_path_safe_uuid(station_uuid) {
        return Err(AppError::validation_msg("Not a radio directory station id"));
    }
    let url = endpoint(client, &format!("stations/byuuid/{station_uuid}")).await;
    station_by_uuid_at(client, &url).await
}

async fn station_by_uuid_at(
    client: &reqwest::Client,
    url: &str,
) -> Result<Option<DirectoryStation>, AppError> {
    let stations: Vec<ApiStation> =
        get_json(client, url, &BTreeMap::new(), "station lookup").await?;

    Ok(stations
        .into_iter()
        .map(ApiStation::into_directory_station)
        .find(|station| station.is_usable() && !radio_blocklist::blocks(station)))
}

/// **Checked on the body, never the status**: the endpoint refuses with `200 {"ok":false}`.
/// Deduplicated per station per ten minutes, so a second press is a refusal to report, not an
/// error to swallow.
pub async fn cast_vote(client: &reqwest::Client, station_uuid: &str) -> Result<(), AppError> {
    if !model::is_path_safe_uuid(station_uuid) {
        return Err(AppError::validation_msg("Not a radio directory station id"));
    }
    let url = endpoint(client, &format!("vote/{station_uuid}")).await;
    cast_vote_at(client, &url).await
}

async fn cast_vote_at(client: &reqwest::Client, url: &str) -> Result<(), AppError> {
    let vote: ApiVote = get_json(client, url, &BTreeMap::new(), "vote").await?;

    if vote.ok {
        return Ok(());
    }
    Err(AppError::network_msg(format!("Radio directory refused the vote: {}", vote.message)))
}

/// One of the directory's facet lists, fetched once per session.
pub async fn facets(client: &reqwest::Client, kind: FacetKind) -> Result<Arc<[Facet]>, AppError> {
    facet_cell(kind).get_or_try_init(|| fetch_facets(client, kind)).await.cloned()
}

/// Four cells rather than a map behind a lock: the set is closed, and a cell can't be caught
/// holding a guard across the fetch it is waiting on.
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
    fetch_facets_at(client, &endpoint(client, query::facet_path(kind)).await, kind).await
}

async fn fetch_facets_at(
    client: &reqwest::Client,
    url: &str,
    kind: FacetKind,
) -> Result<Arc<[Facet]>, AppError> {
    let path = query::facet_path(kind);
    let facets: Vec<ApiFacet> = get_json(client, url, &query::facet_params(kind), path).await?;

    // A ceiling has to be sent, so the only question is whether hitting it is silent. The raw
    // length, for `page_from`'s reason: a cut list is a degradation nothing else would report.
    if query::facet_list_is_capped(kind, facets.len()) {
        log::warn!("radio: the {path} list came back at its ceiling and is likely cut short");
    }

    // Before the cell freezes it, so the cost lands once a session and every consumer is covered.
    Ok(facets
        .into_iter()
        .map(ApiFacet::into_facet)
        .filter(|facet| !radio_blocklist::facet_is_blocked(kind, facet))
        .collect())
}

async fn endpoint(client: &reqwest::Client, path: &str) -> String {
    url_for(mirror(client).await, path)
}

/// The one place a scheme and a separator are spelled, so every host stays a bare name.
fn url_for(host: &str, path: &str) -> String {
    format!("https://{host}/json/{path}")
}

/// Resolved on the first call and kept for the session; `tokio`'s cell because the initialiser is
/// async and `await_holding_lock` is denied. A failed discovery pins the fallback rather than
/// retrying, which would put a second connect timeout in front of every request while offline.
async fn mirror(client: &reqwest::Client) -> &'static str {
    MIRROR
        .get_or_init(|| async {
            match discover_mirror(client, SERVERS_URL).await {
                Ok(host) => host,
                Err(e) => {
                    log::warn!(
                        "Radio directory mirror discovery failed ({}); using \
                         {FALLBACK_HOST} for this session",
                        melodia_core::error::describe(&e)
                    );
                    FALLBACK_HOST.to_owned()
                }
            }
        })
        .await
        .as_str()
}

/// Picks one host off the mirror list. The list URL is a parameter so a test can serve one; the
/// only caller spells the const.
async fn discover_mirror(client: &reqwest::Client, servers_url: &str) -> Result<String, AppError> {
    let servers: Vec<ApiServer> =
        get_json(client, servers_url, &BTreeMap::new(), "mirror list").await?;

    let mut hosts = model::hosts(&servers);
    if hosts.is_empty() {
        return Err(AppError::network_msg("Radio directory mirror list was empty"));
    }
    // Guarded above: `random_range` panics on an empty range.
    let picked = rand::rng().random_range(..hosts.len());
    Ok(hosts.swap_remove(picked))
}

/// One GET with the status guard every call here shares. A non-success status is an `Err`, never
/// an empty result, or an outage would show as an empty grid.
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
