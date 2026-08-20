//! The host and URL contract the fallback and every discovered mirror share.

use super::{FALLBACK_HOST, SERVERS_URL, url_for};

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
    assert!(SERVERS_URL.starts_with("https://"));
}
