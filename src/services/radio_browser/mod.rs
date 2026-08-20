//! Client for the [radio-browser.info](https://www.radio-browser.info) station
//! directory: mirror discovery, station search, and the facet lists the filter
//! chips are built from.
//!
//! Takes the shared `reqwest::Client` rather than building one. The directory
//! asks callers for a descriptive `User-Agent` of the form `appname/appversion`
//! and `services::build_http_client` already sends `Melodia/<version>`, so
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

use crate::entities::radio::{DirectoryStation, Facet, FacetKind, StationSearch};
use crate::error::AppError;
use model::{ApiFacet, ApiServer, ApiStation};

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

/// Search the directory.
///
/// Rows failing [`DirectoryStation::is_usable`] are dropped here rather than at
/// the surface, so a page can come back shorter than the limit it asked for.
pub async fn search(
    client: &reqwest::Client,
    search: &StationSearch,
) -> Result<Vec<DirectoryStation>, AppError> {
    let url = endpoint(client, "stations/search").await;
    let stations: Vec<ApiStation> =
        get_json(client, &url, &query::search_params(search), "search").await?;
    Ok(stations
        .into_iter()
        .map(ApiStation::into_directory_station)
        .filter(DirectoryStation::is_usable)
        .collect())
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
    Ok(facets.into_iter().map(ApiFacet::into_facet).collect())
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
                        crate::services::describe(&e)
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

    response
        .json::<T>()
        .await
        .map_err(|e| AppError::network(format!("Failed to parse radio directory {what}"), e))
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
