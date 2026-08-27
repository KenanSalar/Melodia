//! Request construction for the directory, and the only place its parameter
//! names are spelled.
//!
//! Split from the sending half because the API drops a parameter it does not
//! recognise instead of rejecting it: a misspelling is a filter that silently
//! does nothing, and the only way to catch one is to assert on the names
//! offline.

use std::collections::BTreeMap;

use crate::entities::radio::{FacetKind, SearchOrder, StationSearch};

/// Stations per page, and the fallback whenever a caller leaves
/// [`StationSearch::limit`] at zero.
///
/// There is no "unlimited" option to offer instead: the API's own default is the
/// entire directory, tens of thousands of rows.
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// Ceiling on the tag list, which is the one facet the directory does not curate.
///
/// Tags are free-form and user-entered, so this list is an order of magnitude
/// past the others and grows without anyone approving an entry. Its own knob for
/// that reason, and a ceiling rather than no limit at all because the size of the
/// response is a third party's to decide.
///
/// **High enough to take the whole list, not just its head.** A tag the picker
/// never fetched is one nothing can filter by and no scope pill can offer, and the
/// tail is where a specific genre lives — the head is all general. What the picker
/// *draws* with nothing typed is a separate question, answered by
/// `ui::radio::facets::UNFILTERED_ROW_CAP`.
///
/// There is no "no ceiling" to raise this to: the endpoint answers a call carrying
/// no `limit` with a default slice of its own, so a number is going to be sent
/// whatever this says. Since the list outgrowing it would otherwise go unnoticed —
/// the missing tags are simply never offered and no surface looks wrong —
/// `fetch_facets` logs a list that comes back sitting exactly on it.
pub const TAG_FACET_LIMIT: u32 = 25_000;

/// Ceiling for the curated facet lists, with room for them to grow.
///
/// Well clear of all three today. They are approved rather than typed, so they
/// grow slowly and none is near it.
pub const FACET_LIMIT: u32 = 2000;

/// Query parameters for `/json/stations/search`.
pub fn search_params(search: &StationSearch) -> BTreeMap<&'static str, String> {
    let mut params = BTreeMap::new();

    insert_if_set(&mut params, "name", &search.name);
    insert_if_set(&mut params, "countrycode", &search.country_code);
    insert_if_set(&mut params, "language", &search.language);
    insert_if_set(&mut params, "codec", &search.codec);

    if !search.tags.is_empty() {
        params.insert("tagList", search.tags.join(","));
    }
    if search.bitrate_min > 0 {
        // camelCase. Lowercased it is dropped rather than refused, and the page
        // that comes back is unfiltered — 32 and 128 kbps stations under a
        // 320 floor, which reads exactly like a filter that works.
        params.insert("bitrateMin", search.bitrate_min.to_string());
    }
    if search.offset > 0 {
        params.insert("offset", search.offset.to_string());
    }

    let (order, reverse) = order_params(search.order);
    params.insert("order", order.to_owned());
    params.insert("reverse", reverse.to_string());
    params.insert("limit", page_limit(search.limit).to_string());
    params.insert("hidebroken", "true".to_owned());

    params
}

/// The path segment under `/json/` that lists a facet.
pub fn facet_path(kind: FacetKind) -> &'static str {
    match kind {
        FacetKind::Countries => "countries",
        FacetKind::Languages => "languages",
        FacetKind::Tags => "tags",
        FacetKind::Codecs => "codecs",
    }
}

/// Query parameters for a facet list.
///
/// Ordered by station count because the server's default is alphabetical, which
/// on tags puts punctuation and typos ahead of `pop` and `rock`. The limit is
/// not optional either: the tag list is capped server-side well below its own
/// size, so omitting one takes an arbitrary alphabetical thousand.
pub fn facet_params(kind: FacetKind) -> BTreeMap<&'static str, String> {
    let mut params = BTreeMap::new();
    params.insert("order", "stationcount".to_owned());
    params.insert("reverse", "true".to_owned());
    params.insert("hidebroken", "true".to_owned());
    params.insert("limit", facet_limit(kind).to_string());
    params
}

/// The wire name for an order, and whether it reads best descending.
///
/// Direction belongs to the order rather than to the caller: every popularity
/// order wants the top of the list first and alphabetical wants A first, so a
/// separate flag could only ever be set one way per order.
fn order_params(order: SearchOrder) -> (&'static str, bool) {
    match order {
        SearchOrder::Name => ("name", false),
        SearchOrder::Votes => ("votes", true),
        SearchOrder::ClickCount => ("clickcount", true),
        SearchOrder::Bitrate => ("bitrate", true),
        SearchOrder::Random => ("random", false),
    }
}

/// How many entries of a facet list to take.
pub(super) fn facet_limit(kind: FacetKind) -> u32 {
    match kind {
        FacetKind::Tags => TAG_FACET_LIMIT,
        FacetKind::Countries | FacetKind::Languages | FacetKind::Codecs => FACET_LIMIT,
    }
}

/// The caller's page size, or the default where they left it unset.
///
/// Reachable from the sending half because it is also what a full page *is*:
/// `search` reads its `has_more` off the raw response length against this.
pub(super) fn page_limit(requested: u32) -> u32 {
    if requested == 0 {
        DEFAULT_PAGE_LIMIT
    } else {
        requested
    }
}

/// Add a filter, or leave it out entirely. A blank value is not a filter for the
/// empty string; it is no filter at all.
fn insert_if_set(params: &mut BTreeMap<&'static str, String>, key: &'static str, value: &str) {
    if !value.is_empty() {
        params.insert(key, value.to_owned());
    }
}

#[cfg(test)]
#[path = "tests/query_tests.rs"]
mod tests;
