//! The host and URL contract the fallback and every discovered mirror share.
//!
//! The walk holding this module's reach prohibition is `crates/melodia/tests/radio_facade.rs`: its
//! other half asks the same question of `melodia-app`, and neither covers the other's direction.

use std::collections::BTreeMap;

use melodia_testkit::http::{TestResponse, TestServer};

use super::{
    ApiStation, FALLBACK_HOST, SERVERS_URL, cast_vote, get_json, page_from, station_by_uuid,
    url_for,
};
use melodia_core::entities::radio::DEFAULT_PAGE_LIMIT;
use melodia_core::error::AppError;

/// A bare name, so [`url_for`] stays the one place a scheme or a separator is
/// spelled. A host carrying either would produce `https://https://…` or a
/// doubled slash.
#[test]
fn the_fallback_host_is_a_bare_name() {
    assert!(!FALLBACK_HOST.contains("://"), "{FALLBACK_HOST} carries a scheme");
    assert!(!FALLBACK_HOST.ends_with('/'), "{FALLBACK_HOST} carries a separator");
    assert!(!FALLBACK_HOST.is_empty());
}

#[test]
fn a_directory_url_joins_the_host_and_path_once() {
    assert_eq!(
        url_for(FALLBACK_HOST, "stations/search"),
        "https://de1.api.radio-browser.info/json/stations/search"
    );
    assert_eq!(
        url_for(FALLBACK_HOST, "countries"),
        "https://de1.api.radio-browser.info/json/countries"
    );
}

/// The mirror list is reached over HTTPS rather than the documented DNS SRV
/// lookup, which is what keeps a resolver crate out of the dependency list.
#[test]
fn the_mirror_list_is_fetched_over_https() {
    let parsed = crate::services::net::http_url(SERVERS_URL);

    assert!(parsed.is_some_and(|url| url.scheme() == "https"), "{SERVERS_URL} is not HTTPS");
}

/// A row the client keeps: it can be played, and it can be told apart from the
/// next one.
fn usable_station(index: usize) -> ApiStation {
    ApiStation {
        stationuuid: format!("uuid-{index}"),
        name: format!("Station {index}"),
        url_resolved: format!("https://example.test/{index}"),
        ..ApiStation::default()
    }
}

fn full_page(limit: usize) -> Vec<ApiStation> {
    (0..limit).map(usable_station).collect()
}

/// `has_more` answers about the *response*, not about what survived the client's
/// own filter. Read off the kept rows, one uuid-less station in a full page
/// reads as the end of the directory and paging stops there, on a query the
/// directory has thousands more answers to.
#[test]
fn a_full_page_thinned_by_the_usable_filter_still_reports_more() {
    const LIMIT: usize = 4;

    let mut stations = full_page(LIMIT);
    // Served and counted by the directory, and dropped on the way out of here.
    stations[0].stationuuid = String::new();
    stations[1].url_resolved = String::new();

    let page = page_from(stations, 4);
    assert_eq!(page.stations.len(), LIMIT - 2, "both unusable rows must be dropped");
    assert!(page.has_more, "a full response must report more, however little of it survived");
}

#[test]
fn a_response_short_of_the_limit_is_the_end_of_the_directory() {
    let page = page_from(full_page(3), 4);
    assert_eq!(page.stations.len(), 3);
    assert!(!page.has_more);
}

/// A caller leaving the limit at zero takes the client's page size, so the
/// fullness test has to measure against the number the *request* carried rather
/// than against the zero.
#[test]
fn an_unset_limit_is_measured_against_the_default_page_size() {
    let default = usize::try_from(DEFAULT_PAGE_LIMIT).unwrap_or(usize::MAX);

    assert!(page_from(full_page(default), 0).has_more);
    assert!(!page_from(full_page(default - 1), 0).has_more);
}

/// An outage is not an empty directory. Folding a non-success status into "no stations matched"
/// paints an empty grid over a mirror that is down, and the retry the user would otherwise make
/// looks like a query with no answers.
#[tokio::test]
async fn a_refused_request_is_an_error_rather_than_an_empty_result() {
    let Ok(server) = TestServer::start(|_| TestResponse::status(503)) else {
        unreachable!("a loopback listener on port 0")
    };
    let url = format!("{}/json/stations/search", server.base_url());

    let answered: Result<Vec<ApiStation>, AppError> =
        get_json(&reqwest::Client::new(), &url, &BTreeMap::new(), "search").await;

    assert!(matches!(answered, Err(AppError::Network { .. })));
}

/// A mirror answering 200 with something else entirely is reported as a parse failure naming the
/// call, not as a station list that happens to be empty.
#[tokio::test]
async fn a_body_of_the_wrong_shape_is_reported_as_a_parse_failure() {
    let Ok(server) = TestServer::start(|_| TestResponse::ok("<html>not json</html>")) else {
        unreachable!("a loopback listener on port 0")
    };
    let url = format!("{}/json/stations/search", server.base_url());

    let answered: Result<Vec<ApiStation>, AppError> =
        get_json(&reqwest::Client::new(), &url, &BTreeMap::new(), "search").await;

    assert!(matches!(answered, Err(AppError::Network { .. })));
}

/// Both id-carrying endpoints interpolate the id straight into a path, so the guard in front of
/// them is what stops a crafted id addressing something else on the mirror. It fires before the
/// URL is built, which is also why this test reaches no network: a regression here would send a
/// request, which is the failure it exists to catch.
#[tokio::test]
async fn an_id_that_is_not_a_uuid_never_reaches_the_directory() {
    let client = reqwest::Client::new();

    for crafted in ["../../json/servers", "uuid/../vote", "a b", ""] {
        assert!(
            matches!(station_by_uuid(&client, crafted).await, Err(AppError::Validation(_))),
            "station lookup accepted {crafted:?}",
        );
        assert!(
            matches!(cast_vote(&client, crafted).await, Err(AppError::Validation(_))),
            "vote accepted {crafted:?}",
        );
    }
}
