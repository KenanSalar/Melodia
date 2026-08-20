//! What the wire shapes are easy to get wrong: the directory keeps adding and
//! removing fields, spells two booleans as integers, pads its names, and lists
//! one mirror once per address family.

use super::{ApiFacet, ApiServer, ApiStation, hosts};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A live row, verbatim. Kept whole rather than trimmed to what
/// [`ApiStation`] declares, so it also pins that the rest is ignored.
const STATION_BODY: &str = r#"{
    "changeuuid": "1e4aa307-6ef0-4444-bdb9-5bee3e26a230",
    "stationuuid": "c76686ca-a8b9-4db9-9839-1470c9599623",
    "serveruuid": null,
    "name": "102.7 KIIS FM",
    "url": "https://stream.revma.ihrhls.com/zc185",
    "url_resolved": "https://stream.revma.ihrhls.com/zc185",
    "homepage": "https://kiisfm.iheart.com/",
    "favicon": "https://i.iheart.com/v3/re/assets.brands/logo.png",
    "tags": "pop,top 40",
    "country": "The United States Of America",
    "countrycode": "US",
    "iso_3166_2": "",
    "state": "Los Angeles CA",
    "language": "english",
    "languagecodes": "en",
    "votes": 2325,
    "lastchangetime": "2026-01-15 04:36:26",
    "lastchangetime_iso8601": "2026-01-15T04:36:26Z",
    "codec": "AAC",
    "bitrate": 0,
    "hls": 0,
    "lastcheckok": 1,
    "lastchecktime_iso8601": "2026-01-15T04:36:26Z",
    "clickcount": 915,
    "clicktrend": 915,
    "ssl_error": 0,
    "geo_lat": null,
    "geo_long": null,
    "geo_distance": null,
    "has_extended_info": false
}"#;

/// The live `/json/servers` body. One mirror, listed once per address family.
const SERVERS_BODY: &str = r#"[
    {"ip":"91.98.4.78","name":"de1.api.radio-browser.info"},
    {"ip":"2a01:4f8:1c1d:699::1","name":"de1.api.radio-browser.info"}
]"#;

/// The response carries about twice the fields the struct declares, several of
/// them null. Undeclared has to stay the same as ignored, or one addition
/// upstream empties the whole grid.
#[test]
fn a_full_directory_row_decodes_and_the_rest_is_ignored() -> TestResult {
    let station = serde_json::from_str::<ApiStation>(STATION_BODY)?.into_directory_station();

    assert_eq!(station.station_uuid, "c76686ca-a8b9-4db9-9839-1470c9599623");
    assert_eq!(station.name, "102.7 KIIS FM");
    assert_eq!(station.country, "The United States Of America");
    assert_eq!(station.country_code, "US");
    assert_eq!(station.state, "Los Angeles CA");
    assert_eq!(station.tags, "pop,top 40");
    assert_eq!(station.votes, 2325);
    assert_eq!(station.click_count, 915);
    Ok(())
}

/// Both are `0`/`1` on the wire, not JSON bools, so a `bool` field would fail
/// every row rather than one.
#[test]
fn the_directory_reports_hls_and_its_last_check_as_integers() -> TestResult {
    let station = serde_json::from_str::<ApiStation>(STATION_BODY)?.into_directory_station();

    assert!(!station.hls);
    assert!(station.last_check_ok);
    Ok(())
}

/// The most-clicked station in the world advertises no bitrate at all, which is
/// why nothing may treat the field as a number it can divide by.
#[test]
fn a_missing_bitrate_arrives_as_zero_rather_than_absent() -> TestResult {
    let station = serde_json::from_str::<ApiStation>(STATION_BODY)?.into_directory_station();

    assert_eq!(station.bitrate, 0);
    Ok(())
}

/// A missing field is its default rather than a failed row, which is what lets
/// the API keep changing the shape under us.
#[test]
fn a_row_carrying_nothing_at_all_still_decodes() -> TestResult {
    let station = serde_json::from_str::<ApiStation>("{}")?.into_directory_station();

    assert!(station.name.is_empty());
    assert_eq!(station.favicon_url, None);
    assert!(!station.last_check_ok);
    Ok(())
}

/// Observed on live rows: a leading tab sorts ahead of every letter and paints
/// as a gap down the card column.
#[test]
fn a_padded_station_name_is_trimmed() -> TestResult {
    let body = r#"{"name": "\tArrow Classic Rock "}"#;
    let station = serde_json::from_str::<ApiStation>(body)?.into_directory_station();

    assert_eq!(station.name, "Arrow Classic Rock");
    Ok(())
}

/// `url_resolved` is the one already past `.pls`/`.m3u` indirection, but the
/// directory leaves it blank for a station it has not resolved.
#[test]
fn an_unresolved_url_falls_back_to_the_plain_one() -> TestResult {
    let body = r#"{"url": "http://example.invalid/stream", "url_resolved": ""}"#;
    let station = serde_json::from_str::<ApiStation>(body)?.into_directory_station();

    assert_eq!(station.stream_url, "http://example.invalid/stream");
    Ok(())
}

#[test]
fn a_resolved_url_wins_over_the_plain_one() -> TestResult {
    let body =
        r#"{"url": "http://example.invalid/x.pls", "url_resolved": "http://example.invalid/x"}"#;
    let station = serde_json::from_str::<ApiStation>(body)?.into_directory_station();

    assert_eq!(station.stream_url, "http://example.invalid/x");
    Ok(())
}

/// Blank is how the directory says "no logo", far more often than it omits the
/// field. Carried through as `Some("")`, every such card would fetch an empty
/// URL.
#[test]
fn a_blank_favicon_or_homepage_is_absent_rather_than_empty() -> TestResult {
    let body = r#"{"favicon": "", "homepage": ""}"#;
    let station = serde_json::from_str::<ApiStation>(body)?.into_directory_station();

    assert_eq!(station.favicon_url, None);
    assert_eq!(station.homepage, None);
    Ok(())
}

/// Both URL fields default to empty, so an upstream rename would otherwise fill
/// a grid with cards that cannot be played and report nothing anywhere.
#[test]
fn a_station_with_neither_url_is_not_usable() -> TestResult {
    let body = r#"{"stationuuid":"c76686ca-a8b9-4db9-9839-1470c9599623","name":"Nowhere"}"#;
    let station = serde_json::from_str::<ApiStation>(body)?.into_directory_station();

    assert!(!station.is_usable());
    Ok(())
}

/// An empty uuid is the worse half of the same default: `to_new_station` passes
/// it as `Some("")`, which the UNIQUE column takes as a value, so every station
/// missing one would upsert onto a single row.
#[test]
fn a_station_with_no_uuid_is_not_usable() -> TestResult {
    let body = r#"{"url":"http://example.invalid/stream","name":"Nowhere"}"#;
    let station = serde_json::from_str::<ApiStation>(body)?.into_directory_station();

    assert!(!station.is_usable());
    Ok(())
}

#[test]
fn a_live_directory_row_is_usable() -> TestResult {
    let station = serde_json::from_str::<ApiStation>(STATION_BODY)?.into_directory_station();

    assert!(station.is_usable());
    Ok(())
}

/// Undeduplicated, a "random" pick over this list is a coin flip between two
/// spellings of one host — which looks like it works until a second mirror
/// exists.
#[test]
fn both_address_families_of_one_mirror_collapse_to_one_host() -> TestResult {
    let servers: Vec<ApiServer> = serde_json::from_str(SERVERS_BODY)?;

    assert_eq!(hosts(&servers), vec!["de1.api.radio-browser.info".to_owned()]);
    Ok(())
}

/// The certificate is issued for the hostname, so an IP-keyed request fails TLS
/// verification. The `ip` field is there for clients resolving names themselves,
/// which is the work this endpoint exists to avoid.
#[test]
fn a_host_list_never_offers_an_address() -> TestResult {
    let servers: Vec<ApiServer> = serde_json::from_str(SERVERS_BODY)?;
    let addresses: Vec<&str> = servers.iter().map(|server| server.ip.as_str()).collect();

    for host in hosts(&servers) {
        assert!(!addresses.contains(&host.as_str()), "{host} is an address, not a name");
    }
    Ok(())
}

/// Nothing to dial, and nothing to verify a certificate against.
#[test]
fn a_mirror_entry_without_a_name_is_dropped() -> TestResult {
    let servers: Vec<ApiServer> = serde_json::from_str(r#"[{"ip":"1.2.3.4","name":""}]"#)?;

    assert!(hosts(&servers).is_empty());
    Ok(())
}

/// The name is interpolated straight into a URL, so one carrying a scheme or a
/// path of its own rewrites the request rather than picking a mirror.
#[test]
fn a_mirror_entry_that_is_not_a_bare_host_is_dropped() -> TestResult {
    let body = r#"[
        {"ip":"1.2.3.4","name":"https://de1.api.radio-browser.info"},
        {"ip":"1.2.3.5","name":"de1.api.radio-browser.info/json"},
        {"ip":"1.2.3.6","name":"de1.api.radio-browser.info"}
    ]"#;
    let servers: Vec<ApiServer> = serde_json::from_str(body)?;

    assert_eq!(hosts(&servers), vec!["de1.api.radio-browser.info".to_owned()]);
    Ok(())
}

/// Several mirrors stay several; the dedupe is by name, not a collapse to one.
#[test]
fn distinct_mirrors_all_survive() -> TestResult {
    let body = r#"[
        {"ip":"1.2.3.4","name":"nl1.api.radio-browser.info"},
        {"ip":"1.2.3.5","name":"de1.api.radio-browser.info"},
        {"ip":"::1","name":"de1.api.radio-browser.info"}
    ]"#;
    let servers: Vec<ApiServer> = serde_json::from_str(body)?;

    assert_eq!(hosts(&servers).len(), 2);
    Ok(())
}

/// Countries key their code as `iso_3166_1` and languages as `iso_639`; tags and
/// codecs have neither and filter by name.
#[test]
fn a_facet_takes_whichever_code_its_list_carries() -> TestResult {
    let country = r#"{"name":"Andorra","iso_3166_1":"AD","stationcount":8}"#;
    assert_eq!(serde_json::from_str::<ApiFacet>(country)?.into_facet().code.as_deref(), Some("AD"));

    let language = r#"{"name":"english","iso_639":"en","stationcount":3}"#;
    assert_eq!(
        serde_json::from_str::<ApiFacet>(language)?.into_facet().code.as_deref(),
        Some("en")
    );

    let tag = r#"{"name":"pop","stationcount":6002}"#;
    let tag = serde_json::from_str::<ApiFacet>(tag)?.into_facet();
    assert_eq!(tag.code, None);
    assert_eq!(tag.name, "pop");
    assert_eq!(tag.station_count, 6002);
    Ok(())
}

/// The languages list sends an explicit null for an entry with no code, and the
/// countries list sends a blank. Neither is a code.
#[test]
fn a_null_or_blank_facet_code_is_no_code() -> TestResult {
    let null = r#"{"name":"esperanto","iso_639":null,"stationcount":3}"#;
    assert_eq!(serde_json::from_str::<ApiFacet>(null)?.into_facet().code, None);

    let blank = r#"{"name":"Nowhere","iso_3166_1":"","stationcount":1}"#;
    assert_eq!(serde_json::from_str::<ApiFacet>(blank)?.into_facet().code, None);
    Ok(())
}
