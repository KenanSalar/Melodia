//! Request construction, and the only place the directory's parameter names are spelled.
//!
//! Pure, because the API drops a parameter it doesn't recognise rather than refusing it: a
//! misspelling is a filter that silently does nothing, caught only by asserting the names offline.

use std::collections::BTreeMap;

use melodia_core::entities::radio::{DEFAULT_PAGE_LIMIT, FacetKind, SearchOrder, StationSearch};

/// Ceiling on the tag list, the one facet the directory doesn't curate and so an order of
/// magnitude past the others. High enough to take the tail, where the specific genres live.
/// There is no "no ceiling": the endpoint answers a missing `limit` with a slice of its own, so
/// `fetch_facets` logs a list that comes back sitting on this.
pub const TAG_FACET_LIMIT: u32 = 25_000;

/// For the curated facet lists, which grow slowly and are nowhere near it.
pub const FACET_LIMIT: u32 = 2000;

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
        // camelCase: lowercased it is dropped, not refused, and the unfiltered page reads exactly
        // like a filter that works.
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

/// By station count, the server's alphabetical default putting punctuation and typos first on
/// tags. The limit has to be sent: omitting it takes an arbitrary alphabetical slice.
pub fn facet_params(kind: FacetKind) -> BTreeMap<&'static str, String> {
    let mut params = BTreeMap::new();
    params.insert("order", "stationcount".to_owned());
    params.insert("reverse", "true".to_owned());
    params.insert("hidebroken", "true".to_owned());
    params.insert("limit", facet_limit(kind).to_string());
    params
}

/// The wire name for an order and whether it reads best descending, which is the order's to say:
/// popularity wants the top first and alphabetical wants A first.
fn order_params(order: SearchOrder) -> (&'static str, bool) {
    match order {
        SearchOrder::Name => ("name", false),
        SearchOrder::Votes => ("votes", true),
        SearchOrder::ClickCount => ("clickcount", true),
        SearchOrder::Bitrate => ("bitrate", true),
        SearchOrder::Random => ("random", false),
    }
}

pub(super) fn facet_limit(kind: FacetKind) -> u32 {
    match kind {
        FacetKind::Tags => TAG_FACET_LIMIT,
        FacetKind::Countries | FacetKind::Languages | FacetKind::Codecs => FACET_LIMIT,
    }
}

/// Split out so the check is testable without serving [`TAG_FACET_LIMIT`] rows.
pub(super) fn facet_list_is_capped(kind: FacetKind, received: usize) -> bool {
    received >= facet_limit(kind) as usize
}

/// The caller's page size or the default, and so also what a full page *is*: `search` reads its
/// `has_more` off the raw length against this.
pub(super) fn page_limit(requested: u32) -> u32 {
    if requested == 0 { DEFAULT_PAGE_LIMIT } else { requested }
}

/// A blank value is no filter at all, not a filter for the empty string.
fn insert_if_set(params: &mut BTreeMap<&'static str, String>, key: &'static str, value: &str) {
    if !value.is_empty() {
        params.insert(key, value.to_owned());
    }
}

#[cfg(test)]
#[path = "tests/query_tests.rs"]
mod tests;
