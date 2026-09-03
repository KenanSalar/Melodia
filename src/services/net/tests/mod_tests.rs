use crate::test_support::spellings_outside;

/// A URL scheme tested by prefix, in the substring both spellings share.
const RAW_SCHEME_TEST: &str = "starts_with(\"http";

/// Nobody, this one included: the needle carries a quote, so the declaration above spells it `\"`
/// in the source and cannot match itself the way [`RAW_BODY_READ`]'s does.
const SCHEME_EXEMPT: [(&str, usize); 0] = [];

/// Four sites tested a scheme by prefix — a station's website field, its logo URL, a `.pls`/`.m3u`
/// line and an import line — and two admitted a bare `http://`, which names no host; on the import
/// path that became a row. [`super::http_url`] is the one parse now, and only a corpus walk can
/// see a fifth copy: a prefix test reads as ordinary code and is wrong only on input nobody types
/// by hand.
#[test]
fn nothing_tests_a_url_scheme_by_prefix() {
    let raw = spellings_outside(RAW_SCHEME_TEST, &SCHEME_EXEMPT);

    assert!(
        raw.is_empty(),
        "{raw:?} test a URL's scheme by prefix — use `services::net::http_url`, whose parse also \
         rejects a scheme naming no host"
    );
}

/// The streamed body read [`super::read_capped`] owns.
const RAW_BODY_READ: &str = "bytes_stream()";

/// Where it may appear, and how often. Paths are relative to the crate root that holds them.
const BODY_READ_EXEMPT: [(&str, usize); 3] = [
    ("services/net/mod.rs", 1),
    // The updater's download, and a genuinely different shape: it streams to a file and reports
    // progress rather than collecting a capped `Vec` in memory.
    ("services/updater/install/download.rs", 1),
    // This pin.
    ("services/net/tests/mod_tests.rs", 1),
];

/// Five bounded fetches each had their own copy of the stream-under-a-cap loop, and one of them
/// (the artist image) was a `bytes()` measured afterwards — which had already allocated whatever
/// the host sent. A sixth copy is invisible in review, so it is walked for rather than reviewed.
#[test]
fn every_capped_body_read_goes_through_the_shared_one() {
    let raw = spellings_outside(RAW_BODY_READ, &BODY_READ_EXEMPT);

    assert!(
        raw.is_empty(),
        "{raw:?} stream a response body themselves — use `services::net::read_capped`, which \
         refuses as soon as the body crosses the caller's cap"
    );
}
