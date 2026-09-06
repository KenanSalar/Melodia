//! What the directory is easy to get wrong: it drops a parameter it does not
//! recognise instead of refusing the request, so every assertion below is
//! guarding against a filter that looks like it works and does nothing.

use std::collections::BTreeMap;

use super::{
    FACET_LIMIT, TAG_FACET_LIMIT, facet_list_is_capped, facet_params, facet_path, order_params,
    search_params,
};
use melodia_core::entities::radio::{DEFAULT_PAGE_LIMIT, FacetKind, SearchOrder, StationSearch};

const EVERY_FACET: [FacetKind; 4] = [
    FacetKind::Countries,
    FacetKind::Languages,
    FacetKind::Tags,
    FacetKind::Codecs,
];

fn get<'a>(params: &'a BTreeMap<&'static str, String>, key: &str) -> Option<&'a str> {
    params.get(key).map(String::as_str)
}

/// Verified live: `bitrateMin=320` answered with 320 kbps and up, `bitratemin=320`
/// with 32, 128 and 192. The lowercase spelling is not an error, it is a full
/// page of stations under the floor.
#[test]
fn the_minimum_bitrate_filter_is_camel_case() {
    let search = StationSearch {
        bitrate_min: 320,
        ..StationSearch::default()
    };
    let params = search_params(&search);

    assert_eq!(get(&params, "bitrateMin"), Some("320"));
    assert!(!params.contains_key("bitratemin"));
}

/// A floor of zero is no floor. Sent, it would ask for the stations advertising
/// exactly zero kbps, which is a large share of the directory.
#[test]
fn a_zero_bitrate_floor_is_not_sent() {
    let params = search_params(&StationSearch::default());
    assert!(!params.contains_key("bitrateMin"));
}

/// The API's own default limit is the entire directory, so a search that forgets
/// one asks for tens of thousands of rows.
#[test]
fn every_search_sends_a_limit() {
    let fallback = DEFAULT_PAGE_LIMIT.to_string();
    assert_eq!(get(&search_params(&StationSearch::default()), "limit"), Some(fallback.as_str()));

    let paged = StationSearch {
        limit: 25,
        ..StationSearch::default()
    };
    assert_eq!(get(&search_params(&paged), "limit"), Some("25"));
}

/// The directory ANDs tags through one comma-joined parameter. A second `tag`
/// key would be dropped rather than combined.
#[test]
fn several_tags_become_one_comma_joined_parameter() {
    let search = StationSearch {
        tags: vec!["jazz".to_owned(), "blues".to_owned()],
        ..StationSearch::default()
    };
    let params = search_params(&search);

    assert_eq!(get(&params, "tagList"), Some("jazz,blues"));
    assert!(!params.contains_key("tag"));
}

/// One tag goes through the same key, so there is no second spelling to keep
/// working.
#[test]
fn a_single_tag_uses_the_same_parameter_as_several() {
    let search = StationSearch {
        tags: vec!["jazz".to_owned()],
        ..StationSearch::default()
    };
    assert_eq!(get(&search_params(&search), "tagList"), Some("jazz"));
}

/// The spelling half. `blank_filters_are_omitted_rather_than_sent_empty` below
/// names the same keys and asserts only that they are *absent* from an empty
/// search, which a misspelling satisfies exactly as well.
///
/// Exhaustive rather than `..StationSearch::default()`, so a filter added to the
/// struct has to be pinned here before the test compiles.
#[test]
fn every_set_filter_lands_under_its_own_wire_key() {
    let search = StationSearch {
        name: "jazz fm".to_owned(),
        country_code: "NL".to_owned(),
        language: "dutch".to_owned(),
        tags: vec!["jazz".to_owned()],
        codec: "MP3".to_owned(),
        bitrate_min: 128,
        order: SearchOrder::Votes,
        offset: 100,
        limit: 25,
    };
    let params = search_params(&search);

    assert_eq!(get(&params, "name"), Some("jazz fm"));
    assert_eq!(get(&params, "countrycode"), Some("NL"));
    assert_eq!(get(&params, "language"), Some("dutch"));
    assert_eq!(get(&params, "codec"), Some("MP3"));
    assert_eq!(get(&params, "tagList"), Some("jazz"));
    assert_eq!(get(&params, "bitrateMin"), Some("128"));
    assert_eq!(get(&params, "offset"), Some("100"));
    assert_eq!(get(&params, "limit"), Some("25"));
    assert_eq!(get(&params, "order"), Some("votes"));
    assert_eq!(get(&params, "reverse"), Some("true"));
    assert_eq!(get(&params, "hidebroken"), Some("true"));
}

/// `language` takes the language's name where `countrycode` takes a code, so a
/// filter chip wired off `Facet::code` for both would send `language=en` and
/// substring-match english, armenian and slovenian alike.
#[test]
fn the_language_filter_is_keyed_by_name_and_the_country_one_by_code() {
    let search = StationSearch {
        country_code: "NL".to_owned(),
        language: "dutch".to_owned(),
        ..StationSearch::default()
    };
    let params = search_params(&search);

    assert!(!params.contains_key("languagecode"));
    assert!(!params.contains_key("country"));
}

/// An empty filter is no filter, not a filter for the empty string.
#[test]
fn blank_filters_are_omitted_rather_than_sent_empty() {
    let params = search_params(&StationSearch::default());
    for key in [
        "name",
        "countrycode",
        "language",
        "codec",
        "tagList",
        "offset",
    ] {
        assert!(!params.contains_key(key), "{key} should be absent from an empty search");
    }
}

/// Direction belongs to the order rather than to the caller: popularity wants
/// the top of the list, alphabetical wants A, and no surface has to pair them.
#[test]
fn popularity_orders_read_downwards_and_the_alphabetical_one_does_not() {
    assert_eq!(order_params(SearchOrder::ClickCount), ("clickcount", true));
    assert_eq!(order_params(SearchOrder::Votes), ("votes", true));
    assert_eq!(order_params(SearchOrder::Bitrate), ("bitrate", true));
    assert_eq!(order_params(SearchOrder::Name), ("name", false));
}

/// The remaining arm, so the whole match is pinned rather than four fifths of
/// it. A shuffle has no direction to reverse.
#[test]
fn a_random_order_asks_for_no_direction() {
    assert_eq!(order_params(SearchOrder::Random), ("random", false));
}

/// The empty-query first screen is the directory's own most-played list, which
/// is why there is no separate top-stations endpoint to call for it.
#[test]
fn an_empty_search_asks_for_the_most_clicked_first() {
    let params = search_params(&StationSearch::default());
    assert_eq!(get(&params, "order"), Some("clickcount"));
    assert_eq!(get(&params, "reverse"), Some("true"));
}

/// A station the directory knows is dead is never a useful result.
#[test]
fn every_search_hides_broken_stations() {
    assert_eq!(get(&search_params(&StationSearch::default()), "hidebroken"), Some("true"));
}

/// The server's default order is alphabetical, which on tags puts punctuation
/// and typos ahead of `pop` and `rock`.
#[test]
fn facet_lists_are_ordered_by_station_count() {
    for kind in EVERY_FACET {
        let params = facet_params(kind);
        assert_eq!(get(&params, "order"), Some("stationcount"), "{kind:?}");
        assert_eq!(get(&params, "reverse"), Some("true"), "{kind:?}");
    }
}

/// The tag list is capped server-side well below its own size, so a call without
/// a limit takes an arbitrary alphabetical slice rather than everything.
#[test]
fn every_facet_list_sends_a_limit() {
    for kind in EVERY_FACET {
        assert!(facet_params(kind).contains_key("limit"), "{kind:?}");
    }
}

/// Tags are the one facet nobody curates: free-form, user-entered and an order of
/// magnitude longer than the approved lists. So its ceiling is the loose one, and
/// a limit that fell to theirs would silently stop fetching the tail — where the
/// specific genres are, the head being all general.
#[test]
fn the_tag_list_is_capped_looser_than_the_curated_ones() {
    const { assert!(TAG_FACET_LIMIT > FACET_LIMIT) };

    let whole_tail = TAG_FACET_LIMIT.to_string();
    assert_eq!(get(&facet_params(FacetKind::Tags), "limit"), Some(whole_tail.as_str()));

    let whole = FACET_LIMIT.to_string();
    for kind in [
        FacetKind::Countries,
        FacetKind::Languages,
        FacetKind::Codecs,
    ] {
        assert_eq!(get(&facet_params(kind), "limit"), Some(whole.as_str()), "{kind:?}");
    }
}

/// Bare segments, because the caller joins them onto a path that already ends in
/// a separator.
#[test]
fn facet_paths_are_bare_segments() {
    for kind in EVERY_FACET {
        let path = facet_path(kind);
        assert!(!path.is_empty(), "{kind:?}");
        assert!(!path.starts_with('/'), "{kind:?} would double the separator");
    }
    assert_eq!(facet_path(FacetKind::Countries), "countries");
    assert_eq!(facet_path(FacetKind::Languages), "languages");
    assert_eq!(facet_path(FacetKind::Tags), "tags");
    assert_eq!(facet_path(FacetKind::Codecs), "codecs");
}

/// The ceiling is reported at the value, not past it: a list arriving at exactly its limit is
/// already the cut one, the row that would have been next never having been sent.
///
/// Each kind is measured against its **own** ceiling, which is the half a single-number check
/// would lose — 2000 tags is a quarter of a healthy list and saying so every session trains the
/// reader to ignore the line.
#[test]
fn a_facet_list_is_called_cut_short_from_its_own_ceiling_up() {
    let curated = FACET_LIMIT as usize;
    for kind in [
        FacetKind::Countries,
        FacetKind::Languages,
        FacetKind::Codecs,
    ] {
        assert!(!facet_list_is_capped(kind, curated - 1), "{kind:?} one short of its ceiling");
        assert!(facet_list_is_capped(kind, curated), "{kind:?} sitting on its ceiling");
        assert!(facet_list_is_capped(kind, curated + 1), "{kind:?} past its ceiling");
    }

    let tags = TAG_FACET_LIMIT as usize;
    assert!(!facet_list_is_capped(FacetKind::Tags, curated), "tags stop at the looser ceiling");
    assert!(!facet_list_is_capped(FacetKind::Tags, tags - 1));
    assert!(facet_list_is_capped(FacetKind::Tags, tags));
}
