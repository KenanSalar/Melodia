//! The host and URL contract the fallback and every discovered mirror share.
//!
//! The walk holding this module's reach prohibition is `melodia-tidy`'s: its other half sits
//! in `melodia-app`, and neither covers the other's direction.

use super::{ApiStation, DEFAULT_PAGE_LIMIT, FALLBACK_HOST, SERVERS_URL, page_from, url_for};

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
