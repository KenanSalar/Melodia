use crate::ui::row_match::fold_needle;

use super::{
    ChipFilter, Facet, FacetIndex, MAX_SUGGESTIONS, StationSearch, Suggestion, suggestions,
};

fn facet(name: &str, code: Option<&str>, count: i64) -> Facet {
    Facet {
        name: name.to_owned(),
        code: code.map(str::to_owned),
        station_count: count,
    }
}

/// The four lists as a session that has finished priming holds them.
fn index() -> FacetIndex {
    FacetIndex::from_lists(
        vec![
            facet("Germany", Some("DE"), 4102),
            facet("Turkey", Some("TR"), 611),
        ],
        vec![facet("Turkish", None, 281), facet("German", None, 3900)],
        vec![
            facet("turkish", None, 14),
            facet("jazz", None, 3214),
            facet("rock", None, 9000),
            facet("rockabilly", None, 120),
        ],
        vec![facet("MP3", None, 20000), facet("AAC", None, 5000)],
    )
}

fn offered(needle: &str) -> Vec<Suggestion> {
    suggestions(&fold_needle(needle), &index(), &StationSearch::default())
}

#[test]
fn a_language_the_name_search_barely_reaches_is_offered_with_its_count() {
    let out = offered("turkish");
    let language = out.iter().find(|s| s.chip == ChipFilter::Language);
    assert_eq!(language.map(|s| (s.name.as_str(), s.count)), Some(("Turkish", Some(281))));
}

#[test]
fn an_exact_match_outranks_a_larger_partial_one() {
    // "German" is the language exactly; "Germany" is a country that merely contains it and
    // carries more stations. Size loses to an exact name.
    let out = offered("german");
    let first = out.first().map(|s| (s.chip, s.name.as_str()));
    assert_eq!(first, Some((ChipFilter::Language, "German")));
}

#[test]
fn one_pill_per_scope_even_where_several_entries_match() {
    // "rock" and "rockabilly" both match. The row says "a genre" once and names the exact
    // one; choosing between entries is what the chip's own picker is for.
    let out = offered("rock");
    let tags: Vec<&str> =
        out.iter().filter(|s| s.chip == ChipFilter::Tag).map(|s| s.name.as_str()).collect();
    assert_eq!(tags, ["rock"]);
}

#[test]
fn a_scope_the_query_already_carries_is_not_offered_back() {
    let active = StationSearch {
        language: "Turkish".to_owned(),
        ..StationSearch::default()
    };
    let out = suggestions(&fold_needle("turkish"), &index(), &active);
    assert!(out.iter().all(|s| s.chip != ChipFilter::Language));
    // The tag of the same name is still on offer — it is a different scope.
    assert!(out.iter().any(|s| s.chip == ChipFilter::Tag));
}

#[test]
fn a_country_is_offered_by_its_code_because_that_is_what_the_endpoint_takes() {
    let out = offered("germany");
    let country = out.iter().find(|s| s.chip == ChipFilter::Country);
    assert_eq!(country.map(|s| (s.name.as_str(), s.code.as_str())), Some(("Germany", "DE")));
}

#[test]
fn a_frequency_offers_a_tag_scope_and_states_no_count() {
    // The tag list holds the most-used 500 and `92.1 fm` carries 27 stations, so nothing
    // resident can match it — the pill carries the needle itself.
    let out = offered("92.1");
    assert_eq!(
        out.iter().find(|s| s.count.is_none()).map(|s| (s.chip, s.name.as_str())),
        Some((ChipFilter::Tag, "92.1"))
    );
}

#[test]
fn a_comma_frequency_reaches_the_tag_a_point_is_written_with() {
    let out = offered("101,5");
    assert_eq!(
        out.iter().find_map(|s| s.count.is_none().then_some(s.name.as_str())),
        Some("101.5")
    );
}

#[test]
fn a_bare_integer_offers_a_bitrate_floor_and_a_decimal_one_never_does() {
    let with_floor = offered("128");
    assert_eq!(
        with_floor.iter().find(|s| s.chip == ChipFilter::BitrateMin).map(|s| s.code.as_str()),
        Some("128")
    );
    // The whole reason the two shapes are told apart: `92.1` is a frequency, and a bitrate
    // pill over it would bury the one that finds the station.
    assert!(offered("92.1").iter().all(|s| s.chip != ChipFilter::BitrateMin));
}

#[test]
fn a_number_outside_the_kbps_band_stays_a_name_search() {
    // "Radio 24" and "Radio 1000" are names, not floors.
    assert!(offered("24").iter().all(|s| s.chip != ChipFilter::BitrateMin));
    assert!(offered("1000").iter().all(|s| s.chip != ChipFilter::BitrateMin));
}

#[test]
fn a_bitrate_floor_already_applied_is_not_offered_back() {
    let active = StationSearch {
        bitrate_min: 128,
        ..StationSearch::default()
    };
    let out = suggestions(&fold_needle("128"), &index(), &active);
    assert!(out.iter().all(|s| s.chip != ChipFilter::BitrateMin));
}

#[test]
fn a_needle_too_short_to_scope_offers_nothing() {
    // One letter matches a third of the country list, and the row would fill with noise
    // while the user is still typing.
    assert!(offered("t").is_empty());
    assert!(offered("").is_empty());
}

#[test]
fn a_needle_matching_no_facet_offers_nothing() {
    assert!(offered("bbc").is_empty());
}

#[test]
fn the_cap_drops_a_guess_before_it_drops_a_shape() {
    // Contrived so all four lists match a needle that is also a bitrate — five scopes for a
    // row that holds four. The floor is a fact about what was typed where the rest are
    // guesses at what it names, so it is the one that must survive.
    let all = FacetIndex::from_lists(
        vec![facet("Region 128", Some("R1"), 10)],
        vec![facet("Lang128", None, 20)],
        vec![facet("128 hits", None, 30)],
        vec![facet("MP128", None, 40)],
    );
    let out = suggestions(&fold_needle("128"), &all, &StationSearch::default());
    assert_eq!(out.len(), MAX_SUGGESTIONS);
    assert_eq!(out.first().map(|s| s.chip), Some(ChipFilter::BitrateMin));
}

#[test]
fn an_unprimed_index_offers_only_what_the_needle_shape_says() {
    let empty = FacetIndex::default();
    let out = suggestions(&fold_needle("turkish"), &empty, &StationSearch::default());
    assert!(out.is_empty());
    // A shape needs no list, so it survives a prime that has not landed.
    let out = suggestions(&fold_needle("128"), &empty, &StationSearch::default());
    assert_eq!(out.len(), 1);
}
