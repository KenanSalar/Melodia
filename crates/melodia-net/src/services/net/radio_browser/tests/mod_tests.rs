//! The host and URL contract the fallback and every discovered mirror share.
//!
//! The walk holding this module's reach prohibition is `crates/melodia/tests/radio_facade.rs`: its
//! other half asks the same question of `melodia-app`, and neither covers the other's direction.

use std::collections::BTreeMap;

use melodia_testkit::http::{TestResponse, TestServer};

use super::{
    ApiStation, FALLBACK_HOST, SERVERS_URL, cast_vote, cast_vote_at, count_click_at,
    discover_mirror, fetch_facets_at, get_json, page_from, search_at, station_by_uuid,
    station_by_uuid_at, url_for,
};
use melodia_core::entities::radio::{DEFAULT_PAGE_LIMIT, FacetKind, StationSearch};
use melodia_core::error::AppError;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A server answering every path with one canned response, and the URL to hand a worker.
///
/// The workers below take the whole URL, so nothing here goes through `url_for` — the path is
/// only what the recorded request carries.
fn serving(response: TestResponse) -> Result<(TestServer, String), Box<dyn std::error::Error>> {
    let server = TestServer::start(move |_| response.clone())?;
    let url = format!("{}/json/probe", server.base_url());
    Ok((server, url))
}

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

// ---- the click and the vote, which read opposite halves of one response ----

/// **A click is judged on its status and the body is never touched.** The endpoint answers an
/// unknown station with a 404 carrying nothing, so asking for JSON would turn a clean refusal into
/// a parse error; a mirror that answers 200 with anything at all has counted the play.
#[tokio::test]
async fn a_click_the_directory_accepts_is_not_parsed_at_all() -> TestResult {
    let (_server, url) = serving(TestResponse::ok("<html>not json</html>"))?;

    assert!(count_click_at(&reqwest::Client::new(), &url).await.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_click_for_a_station_the_directory_lost_is_an_error() -> TestResult {
    let (_server, url) = serving(TestResponse::status(404))?;

    let counted = count_click_at(&reqwest::Client::new(), &url).await;

    assert!(matches!(counted, Err(AppError::Network { .. })), "{counted:?}");
    Ok(())
}

/// **A vote is judged on its body and never on its status**, which is why it cannot borrow the
/// click's guard: a refused vote and a counted one are both `200`, and the wording is the server's
/// own — a rate-limited second press has something to say that "it worked" would swallow.
#[tokio::test]
async fn a_vote_the_directory_refuses_carries_the_reason_it_gave() -> TestResult {
    let (_server, url) =
        serving(TestResponse::ok(r#"{"ok":false,"message":"you voted too recently"}"#))?;

    let voted = cast_vote_at(&reqwest::Client::new(), &url).await;

    let Err(AppError::Network { msg, .. }) = voted else {
        return Err(format!("a refused vote must be an error, got {voted:?}").into());
    };
    assert!(msg.contains("you voted too recently"), "{msg}");
    Ok(())
}

#[tokio::test]
async fn a_vote_the_directory_counted_is_a_success() -> TestResult {
    let (_server, url) = serving(TestResponse::ok(r#"{"ok":true,"message":"voted"}"#))?;

    assert!(cast_vote_at(&reqwest::Client::new(), &url).await.is_ok());
    Ok(())
}

// ---- a station the directory no longer serves ----

/// A withdrawn station is `Ok(None)` rather than an error: the endpoint answers an empty array for
/// it, and a row kept locally outliving its directory entry is the design rather than a failure.
#[tokio::test]
async fn a_uuid_the_directory_no_longer_knows_is_absent_rather_than_an_error() -> TestResult {
    let (_server, url) = serving(TestResponse::ok("[]"))?;

    let found = station_by_uuid_at(&reqwest::Client::new(), &url).await?;

    assert!(found.is_none());
    Ok(())
}

/// The same answer for a row the directory still serves and nothing can play. Every caller already
/// handles the page that no longer exists, so an unusable row takes that shape rather than a second.
#[tokio::test]
async fn a_station_the_directory_serves_that_nothing_can_play_is_absent() -> TestResult {
    let (_server, url) =
        serving(TestResponse::ok(r#"[{"stationuuid":"abc","name":"No Stream","url":""}]"#))?;

    let found = station_by_uuid_at(&reqwest::Client::new(), &url).await?;

    assert!(found.is_none(), "a row with no stream URL must not reach a caller");
    Ok(())
}

// ---- the projections, against a body rather than a hand-built Vec ----

/// The composition, which is all this holds: the body a mirror sent reaches `page_from` and the
/// rows come back projected. What each field becomes on the way is `model_tests`' — asserting it
/// again here would pin the same tidying twice and read as two independent guarantees.
#[tokio::test]
async fn a_search_page_is_built_from_the_body_the_mirror_sent() -> TestResult {
    let (_server, url) = serving(TestResponse::ok(
        r#"[{"stationuuid":"a","name":"Alpha FM","url_resolved":"https://example.test/a"},
            {"stationuuid":"b","name":"Beta FM","url_resolved":"https://example.test/b"}]"#,
    ))?;

    let page = search_at(&reqwest::Client::new(), &url, &StationSearch::default()).await?;

    let names: Vec<&str> = page.stations.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Alpha FM", "Beta FM"]);
    assert!(!page.has_more, "two rows is short of any page size");
    Ok(())
}

/// The same claim for the facet door, which has no `page_from` between the body and the entity:
/// what it adds is that the list is projected and frozen rather than handed back raw.
#[tokio::test]
async fn a_facet_list_is_built_from_the_body_the_mirror_sent() -> TestResult {
    let (_server, url) = serving(TestResponse::ok(
        r#"[{"name":"Germany","stationcount":42,"iso_3166_1":"DE"},
            {"name":"Netherlands","stationcount":7,"iso_3166_1":"NL"}]"#,
    ))?;

    let facets = fetch_facets_at(&reqwest::Client::new(), &url, FacetKind::Countries).await?;

    let named: Vec<(&str, i64)> =
        facets.iter().map(|f| (f.name.as_str(), f.station_count)).collect();
    assert_eq!(named, [("Germany", 42), ("Netherlands", 7)]);
    Ok(())
}

// ---- mirror discovery ----

/// An empty list is an error rather than a pick, and the guard is load-bearing rather than
/// defensive: `random_range` panics on an empty range, so losing it takes the whole process down
/// on a mirror list that came back valid and empty.
#[tokio::test]
async fn an_empty_mirror_list_is_an_error_rather_than_a_pick() -> TestResult {
    let (_server, url) = serving(TestResponse::ok("[]"))?;

    let discovered = discover_mirror(&reqwest::Client::new(), &url).await;

    assert!(matches!(discovered, Err(AppError::Network { .. })), "{discovered:?}");
    Ok(())
}

/// Membership rather than an expected host, the pick being random by design. What it holds is that
/// the pick comes off the *list*: a version answering `FALLBACK_HOST` here would leave discovery
/// working, every session on one mirror, and nothing else able to tell.
#[tokio::test]
async fn the_mirror_picked_is_one_the_directory_served() -> TestResult {
    const SERVED: [&str; 3] = [
        "de1.api.example.test",
        "nl1.api.example.test",
        "us1.api.example.test",
    ];

    let (_server, url) = serving(TestResponse::ok(
        r#"[{"ip":"10.0.0.1","name":"de1.api.example.test"},
            {"ip":"10.0.0.2","name":"nl1.api.example.test"},
            {"ip":"10.0.0.3","name":"us1.api.example.test"}]"#,
    ))?;

    let picked = discover_mirror(&reqwest::Client::new(), &url).await?;

    assert!(SERVED.contains(&picked.as_str()), "{picked} is not one of the hosts served");
    Ok(())
}
