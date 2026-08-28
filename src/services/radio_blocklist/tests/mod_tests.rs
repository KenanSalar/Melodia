//! **Every test builds its own list and none reads the baked one.** The baked terms
//! come from a source that is not in the repo, so they differ per machine and are
//! empty in CI — a test asserting against them would pass here and prove nothing
//! there, or worse, start failing the day a term happened to match a fixture.

use super::source::{self, MIN_PATTERN_CHARS, TermKind};
use super::{Blocklist, StationTerms};
use crate::entities::radio::{Facet, FacetKind};

/// The key every fixture hashes under, so a pinned fingerprint stays pinned.
const SOURCE_KEY: &str = "key: test-key\n";

fn blocklist(rules: &str) -> Result<Blocklist, String> {
    let source = format!("{SOURCE_KEY}{rules}");
    source::parse_source(&source).map(Blocklist::from_terms)
}

/// A station matching nothing, for a test to override one field of.
fn station() -> StationTerms<'static> {
    StationTerms {
        station_uuid: None,
        name: "",
        stream_url: "",
        country_code: "",
        language: "",
        codec: "",
        tags: "",
    }
}

fn facet(name: &str, code: Option<&str>) -> Facet {
    Facet {
        name: name.to_owned(),
        code: code.map(str::to_owned),
        station_count: 1,
    }
}

#[test]
fn a_known_term_keeps_its_fingerprint() {
    // Pinned so a change to the fold, the key derivation or the hash fails here
    // rather than silently unblocking every term in every install.
    let key = source::key_from(Some("test-key"));
    assert_eq!(source::fingerprint(&key, TermKind::Tag, "jazz"), 6_603_931_626_869_841_344);
}

#[test]
fn one_spelling_cannot_block_two_axes() {
    let key = source::key_from(Some("test-key"));
    assert_ne!(
        source::fingerprint(&key, TermKind::Tag, "xx"),
        source::fingerprint(&key, TermKind::Country, "xx")
    );
}

#[test]
fn two_keys_disagree_about_one_term() {
    let one = source::key_from(Some("first"));
    let other = source::key_from(Some("second"));
    assert_ne!(
        source::fingerprint(&one, TermKind::Tag, "jazz"),
        source::fingerprint(&other, TermKind::Tag, "jazz")
    );
}

#[test]
fn case_and_whitespace_fold_away() -> Result<(), String> {
    let list = blocklist("tag: classic rock\n")?;

    for spelling in [
        "classic rock",
        "Classic Rock",
        "  CLASSIC   rock  ",
        "classic\trock",
    ] {
        assert!(
            list.blocks_station(&StationTerms {
                tags: spelling,
                ..station()
            }),
            "{spelling}"
        );
    }
    Ok(())
}

#[test]
fn each_exact_axis_blocks_its_own_field() -> Result<(), String> {
    let list = blocklist(
        "country: XX\nlanguage: klingon\ncodec: WMA\nstation: abc-123\n\
         name: Some Station\nurl: http://example.invalid/s\ntag: polka\n",
    )?;

    assert!(list.blocks_station(&StationTerms {
        country_code: "XX",
        ..station()
    }));
    assert!(list.blocks_station(&StationTerms {
        language: "klingon",
        ..station()
    }));
    assert!(list.blocks_station(&StationTerms {
        codec: "WMA",
        ..station()
    }));
    assert!(list.blocks_station(&StationTerms {
        station_uuid: Some("abc-123"),
        ..station()
    }));
    assert!(list.blocks_station(&StationTerms {
        name: "Some Station",
        ..station()
    }));
    assert!(list.blocks_station(&StationTerms {
        stream_url: "http://example.invalid/s",
        ..station()
    }));
    // One tag among several, which is how the directory serves them.
    assert!(list.blocks_station(&StationTerms {
        tags: "jazz,polka,folk",
        ..station()
    }));
    Ok(())
}

#[test]
fn an_exact_term_does_not_match_a_value_containing_it() -> Result<(), String> {
    let list = blocklist("name: Some Station\ntag: rock\n")?;

    assert!(!list.blocks_station(&StationTerms {
        name: "Some Station 100.5 FM",
        ..station()
    }));
    assert!(!list.blocks_station(&StationTerms {
        tags: "classic rock",
        ..station()
    }));
    Ok(())
}

#[test]
fn countries_match_a_facet_on_the_code_and_never_on_the_name() -> Result<(), String> {
    let list = blocklist("country: XX\n")?;

    assert!(list.blocks_facet(FacetKind::Countries, &facet("Faraway", Some("XX"))));
    // The name is what a chip shows; the code is what a pick filters by.
    assert!(!list.blocks_facet(FacetKind::Countries, &facet("XX", None)));
    Ok(())
}

#[test]
fn the_other_curated_facets_match_on_their_name() -> Result<(), String> {
    let list = blocklist("language: klingon\ntag: polka\ncodec: WMA\n")?;

    assert!(list.blocks_facet(FacetKind::Languages, &facet("klingon", Some("tlh"))));
    assert!(list.blocks_facet(FacetKind::Tags, &facet("polka", None)));
    assert!(list.blocks_facet(FacetKind::Codecs, &facet("WMA", None)));
    Ok(())
}

#[test]
fn a_station_level_term_matches_no_facet() -> Result<(), String> {
    let list = blocklist("name: polka\nurl: polka\nstation: polka\n")?;

    for kind in [
        FacetKind::Countries,
        FacetKind::Languages,
        FacetKind::Tags,
        FacetKind::Codecs,
    ] {
        assert!(!list.blocks_facet(kind, &facet("polka", Some("polka"))), "{kind:?}");
    }
    Ok(())
}

#[test]
fn a_build_with_no_source_blocks_nothing() -> Result<(), String> {
    let list = blocklist("")?;

    assert!(!list.blocks_station(&StationTerms {
        tags: "anything",
        ..station()
    }));
    assert!(!list.blocks_facet(FacetKind::Tags, &facet("anything", None)));
    Ok(())
}

#[test]
fn parsing_leaves_every_list_sorted() -> Result<(), String> {
    // Both lookups binary-search, so an unsorted list would miss rather than fail,
    // and `build.rs` emits these in the order it is handed them. Asserted on parsed
    // output rather than on the baked arrays: those come from a source outside the
    // repo, so a test reading them would assert against different data on every
    // machine and against nothing at all in CI.
    let terms = source::parse_source(&format!(
        "{SOURCE_KEY}tag: zebra\ntag: alpha\ntag: middle\n\
         tag-contains: zulu\ntag-contains: alfa\nname-contains: kilo\n"
    ))?;

    assert!(terms.fingerprints.is_sorted());
    assert!(terms.patterns.is_sorted());
    assert!(terms.pattern_lengths.is_sorted());

    // The same has to hold coming back off the wire, where the order is whatever the
    // secret happened to carry.
    let round_tripped = source::parse_hashed(&source::render_hashed(&terms))?;
    assert!(round_tripped.fingerprints.is_sorted());
    assert!(round_tripped.patterns.is_sorted());
    Ok(())
}

#[test]
fn a_pattern_matches_wherever_it_sits() -> Result<(), String> {
    let list = blocklist("tag-contains: polka\n")?;

    for tags in [
        "polka",
        "polka rock",
        "old polka",
        "very old polka music",
        "a,polka,b",
    ] {
        assert!(list.blocks_station(&StationTerms { tags, ..station() }), "{tags}");
    }
    assert!(!list.blocks_station(&StationTerms {
        tags: "pol ka",
        ..station()
    }));
    Ok(())
}

#[test]
fn a_pattern_folds_like_an_exact_term() -> Result<(), String> {
    let list = blocklist("tag-contains: classic rock\n")?;

    assert!(list.blocks_station(&StationTerms {
        tags: "Best CLASSIC   ROCK ever",
        ..station()
    }));
    Ok(())
}

#[test]
fn a_pattern_walks_characters_rather_than_bytes() -> Result<(), String> {
    // Windows are sliced at character boundaries; stepping by bytes would either
    // panic on a multi-byte value or silently compare the wrong window.
    let list = blocklist("tag-contains: über\n")?;

    assert!(list.blocks_station(&StationTerms {
        tags: "schöne über musik",
        ..station()
    }));
    assert!(!list.blocks_station(&StationTerms {
        tags: "uber musik",
        ..station()
    }));
    Ok(())
}

#[test]
fn a_pattern_reaches_the_name_and_url_axes_too() -> Result<(), String> {
    let list = blocklist("name-contains: polka\nurl-contains: badhost\n")?;

    assert!(list.blocks_station(&StationTerms {
        name: "The Polka Hour",
        ..station()
    }));
    assert!(list.blocks_station(&StationTerms {
        stream_url: "http://badhost.invalid/stream",
        ..station()
    }));
    Ok(())
}

#[test]
fn a_pattern_blocks_a_tag_facet_and_leaves_the_others() -> Result<(), String> {
    let list = blocklist("tag-contains: polka\n")?;

    assert!(list.blocks_facet(FacetKind::Tags, &facet("old polka music", None)));
    // Only the tag facet takes a pattern; the rest stay exact.
    assert!(!list.blocks_facet(FacetKind::Languages, &facet("old polka music", None)));
    assert!(!list.blocks_facet(FacetKind::Codecs, &facet("old polka music", None)));
    Ok(())
}

#[test]
fn a_pattern_under_the_floor_is_refused() {
    let short = "x".repeat(MIN_PATTERN_CHARS - 1);
    assert!(blocklist(&format!("tag-contains: {short}\n")).is_err());
    assert!(blocklist(&format!("tag-contains: {}\n", "x".repeat(MIN_PATTERN_CHARS))).is_ok());
}

#[test]
fn only_the_free_text_axes_take_a_pattern() {
    for exact_only in ["country", "language", "codec", "station"] {
        assert!(
            blocklist(&format!("{exact_only}-contains: something\n")).is_err(),
            "{exact_only}-contains should not be a kind"
        );
    }
}

#[test]
fn a_malformed_line_is_refused_rather_than_skipped() {
    // A skipped line unblocks a station with nothing anywhere to report it, so every
    // one of these has to stop the build.
    for broken in [
        "genre: rock\n",      // unknown kind
        "just some text\n",   // no separator
        "tag:\n",             // empty value
        "tag: rock  # why\n", // inline comment would join the value
        "country: DEU\n",     // not a two-letter code
        "key: a\nkey: b\n",   // a second key
    ] {
        assert!(source::parse_source(broken).is_err(), "{broken:?}");
    }
}

#[test]
fn comments_and_blank_lines_carry_nothing() -> Result<(), String> {
    let list = blocklist("# a note\n\n   \ntag: polka\n# another\n")?;

    assert!(list.blocks_station(&StationTerms {
        tags: "polka",
        ..station()
    }));
    assert!(!list.blocks_station(&StationTerms {
        tags: "a note",
        ..station()
    }));
    Ok(())
}

#[test]
fn no_error_quotes_the_line_it_refused() {
    // These reach a public CI log, so a message carrying the term would hand over
    // the entry it was protecting.
    let secret = "verysecrettermnobodyshouldsee";
    for broken in [
        format!("wrongkind: {secret}\n"),
        format!("tag: {secret}  # note\n"),
        format!("country: {secret}\n"),
    ] {
        let Err(reason) = source::parse_source(&broken) else {
            continue;
        };
        assert!(!reason.contains(secret), "{reason}");
    }
}

#[test]
fn hashed_output_reads_back_as_the_same_terms() -> Result<(), String> {
    let source = format!(
        "{SOURCE_KEY}country: XX\ntag: polka\ntag-contains: something\nname-contains: elsewhere\n"
    );
    let original = source::parse_source(&source)?;
    let round_tripped = source::parse_hashed(&source::render_hashed(&original))?;

    assert_eq!(original.key, round_tripped.key);
    assert_eq!(original.fingerprints, round_tripped.fingerprints);
    assert_eq!(original.patterns, round_tripped.patterns);
    assert_eq!(original.pattern_lengths, round_tripped.pattern_lengths);
    Ok(())
}

#[test]
fn a_hashed_source_blocks_what_the_list_it_came_from_blocked() -> Result<(), String> {
    let source = format!("{SOURCE_KEY}tag: polka\ntag-contains: something\n");
    let hashed = source::render_hashed(&source::parse_source(&source)?);
    let list = Blocklist::from_terms(source::parse_any(&hashed)?);

    assert!(list.blocks_station(&StationTerms {
        tags: "polka",
        ..station()
    }));
    assert!(list.blocks_station(&StationTerms {
        tags: "well something else",
        ..station()
    }));
    assert!(!list.blocks_station(&StationTerms {
        tags: "jazz",
        ..station()
    }));
    Ok(())
}

#[test]
fn parse_any_dispatches_on_the_marker() -> Result<(), String> {
    let plaintext = source::parse_any(&format!("{SOURCE_KEY}tag: polka\n"))?;
    assert_eq!(plaintext.fingerprints.len(), 1);

    let hashed = source::parse_any(&source::render_hashed(&plaintext))?;
    assert_eq!(hashed.fingerprints, plaintext.fingerprints);
    Ok(())
}

#[test]
fn a_malformed_pre_hashed_source_is_refused() {
    let key = "0".repeat(64);
    for broken in [
        format!("{}\nkey = {key}\nterms =\n", source::HASHED_MARKER), // missing fields
        format!("{}\nkey = abcd\nterms =\npatterns =\nlengths =\n", source::HASHED_MARKER),
        format!("{}\nkey = {key}\nterms = abc\npatterns =\nlengths =\n", source::HASHED_MARKER),
        format!(
            "{}\nkey = {key}\nterms =\npatterns =\nlengths =\nbogus = 1\n",
            source::HASHED_MARKER
        ),
    ] {
        assert!(source::parse_hashed(&broken).is_err(), "{broken:?}");
    }
}
