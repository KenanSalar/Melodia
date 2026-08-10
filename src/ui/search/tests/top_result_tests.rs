//! Unit tests for the 9-step Top Result ranking. The fixtures keep the
//! album/artist/genre sets small and deliberately ambiguous so each
//! ranking tier is exercised independently — a single regression in the
//! function body would flip the corresponding tier's expected winner.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::{TopKind, TopSubtitle, compute_top_result};
use crate::library::search::SearchResults;
use crate::entities::album::AlbumStats;
use crate::entities::artist::ArtistStats;
use crate::entities::genre::GenreStats;

const CARD: &str =
    include_str!("../../../../melodia-ui/ui/views/search/top-result-card.slint");
const ROUTER: &str = include_str!("../callbacks/results.rs");
const ARTWORK_IMAGE: &str =
    include_str!("../../../../melodia-ui/ui/components/artwork-image.slint");
const GENRE_GRID: &str =
    include_str!("../../../../melodia-ui/ui/components/grid/genre-grid.slint");

/// Collapse every run of whitespace so a `.slint` binding can be matched
/// as one string regardless of how it happens to be wrapped.
fn squeeze(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The value a `key: … ;` binding carries, taken from already-squeezed
/// source. Used to read a default out of the component that declares it
/// rather than restating the default in this test too.
fn value_after(squeezed: &str, key: &str) -> String {
    squeezed
        .split(key)
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .map(str::trim)
        .map(str::to_owned)
        .expect("binding present in source")
}

fn album(id: i64, name: &str) -> AlbumStats {
    AlbumStats {
        id,
        name: name.to_owned(),
        sort_name: None,
        artist_id: 1,
        artist_name: "X".to_owned(),
        year: None,
        disc_count: None,
        is_compilation: false,
        musicbrainz_id: None,
        artwork_path: None,
        track_count: 1,
        total_duration_ms: 0,
    }
}

fn artist(id: i64, name: &str) -> ArtistStats {
    ArtistStats {
        id,
        name: name.to_owned(),
        sort_name: None,
        musicbrainz_id: None,
        image_path: None,
        track_count: 1,
        album_count: 1,
        total_duration_ms: 0,
    }
}

fn genre(id: i64, name: &str) -> GenreStats {
    GenreStats {
        id,
        name: name.to_owned(),
        track_count: 1,
        total_duration_ms: 0,
    }
}

/// Most tiers only need two of the three lists, so this leaves genres
/// empty and [`results_with_genres`] is the opt-in for the rest.
fn results(albums: Vec<AlbumStats>, artists: Vec<ArtistStats>) -> SearchResults {
    results_with_genres(albums, artists, Vec::new())
}

fn results_with_genres(
    albums: Vec<AlbumStats>,
    artists: Vec<ArtistStats>,
    genres: Vec<GenreStats>,
) -> SearchResults {
    SearchResults {
        tracks: Vec::new(),
        albums,
        artists,
        genres,
    }
}

#[test]
fn empty_query_returns_none() {
    let r = results(vec![album(1, "Metal Album")], vec![artist(1, "Metal")]);
    assert!(compute_top_result(&r, "").is_none());
    assert!(compute_top_result(&r, "   ").is_none());
}

#[test]
fn empty_results_returns_none() {
    let r = results(Vec::new(), Vec::new());
    assert!(compute_top_result(&r, "metal").is_none());
}

#[test]
fn tier_1_exact_album_wins_over_exact_artist() {
    // Both an exact-album and an exact-artist match are present.
    // Tier 1 (album) should beat tier 2 (artist).
    let r = results(
        vec![album(10, "Metal")],
        vec![artist(20, "Metal"), artist(21, "Metallica")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Album);
    assert_eq!(top.id, 10);
}

#[test]
fn tier_2_exact_artist_wins_when_no_exact_album() {
    let r = results(
        vec![album(10, "Metal Album"), album(11, "Metallic Sounds")],
        vec![artist(20, "Metal")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Artist);
    assert_eq!(top.id, 20);
}

/// An exact genre outranks an album that merely *starts with* the query:
/// exactness wins the band, which is the same rule tiers 1-2 encode.
#[test]
fn tier_3_exact_genre_beats_a_starts_with_album() {
    let r = results_with_genres(
        vec![album(10, "Metal Album")],
        vec![artist(20, "Metallica")],
        vec![genre(30, "Metal")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Genre);
    assert_eq!(top.id, 30);
}

/// ...but never an exact album or artist. Genre is last in its band.
#[test]
fn tier_3_exact_genre_loses_to_an_exact_album_or_artist() {
    let with_album = results_with_genres(
        vec![album(10, "Metal")],
        Vec::new(),
        vec![genre(30, "Metal")],
    );
    assert_eq!(
        compute_top_result(&with_album, "metal").expect("top result").kind,
        TopKind::Album
    );

    let with_artist = results_with_genres(
        Vec::new(),
        vec![artist(20, "Metal")],
        vec![genre(30, "Metal")],
    );
    assert_eq!(
        compute_top_result(&with_artist, "metal").expect("top result").kind,
        TopKind::Artist
    );
}

#[test]
fn tier_4_album_starts_with_wins_when_no_exact() {
    let r = results(
        vec![album(10, "Metal Album")],
        vec![artist(20, "Metallica")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Album);
    assert_eq!(top.id, 10);
}

#[test]
fn tier_5_artist_starts_with_when_no_starts_with_album() {
    let r = results(
        // Album exists but doesn't start with the query.
        vec![album(10, "Heavy Metal")],
        vec![artist(20, "Metallica")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Artist);
    assert_eq!(top.id, 20);
}

#[test]
fn tier_6_genre_starts_with_when_neither_name_does() {
    let r = results_with_genres(
        vec![album(10, "Heavy Metal")],
        vec![artist(20, "Iron Maiden")],
        vec![genre(30, "Metalcore")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Genre);
    assert_eq!(top.id, 30);
}

#[test]
fn tier_7_first_album_when_no_exact_or_starts_with() {
    let r = results(
        vec![album(10, "Heavy Metal"), album(11, "Death Metal")],
        vec![artist(20, "Iron Maiden")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Album);
    assert_eq!(top.id, 10); // first in vec
}

#[test]
fn tier_8_first_artist_when_no_albums() {
    let r = results(
        Vec::new(),
        vec![artist(20, "Iron Maiden"), artist(21, "Megadeth")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Artist);
    assert_eq!(top.id, 20);
}

/// The card used to be hidden outright when a query matched no album and
/// no artist — which is exactly what a genre-only search is.
#[test]
fn tier_9_first_genre_when_nothing_else_matched() {
    let r = results_with_genres(
        Vec::new(),
        Vec::new(),
        vec![genre(30, "Nu Metal"), genre(31, "Doom Metal")],
    );
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Genre);
    assert_eq!(top.id, 30);
}

#[test]
fn case_insensitive_exact_match() {
    let r = results(vec![album(10, "METAL")], vec![]);
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.kind, TopKind::Album);
}

#[test]
fn whitespace_trimmed_from_query() {
    let r = results(vec![album(10, "Metal")], vec![]);
    let top = compute_top_result(&r, "  metal  ").expect("top result");
    assert_eq!(top.kind, TopKind::Album);
}

/// The counts stay counts all the way to the UI thread — a sentence built
/// here would reach the card untranslated, since `@tr` only sees literals
/// inside `.slint`.
#[test]
fn subtitle_for_artist_top_uses_album_count() {
    let mut a = artist(20, "Metallica");
    a.album_count = 11;
    let r = results(vec![], vec![a]);
    let top = compute_top_result(&r, "metallica").expect("top result");
    assert_eq!(top.subtitle, TopSubtitle::AlbumCount(11));
}

#[test]
fn subtitle_for_genre_top_uses_track_count() {
    let mut g = genre(30, "Metal");
    g.track_count = 42;
    let r = results_with_genres(vec![], vec![], vec![g]);
    let top = compute_top_result(&r, "metal").expect("top result");
    assert_eq!(top.subtitle, TopSubtitle::TrackCount(42));
}

#[test]
fn subtitle_for_album_top_uses_artist_name() {
    let mut a = album(10, "Master of Puppets");
    a.artist_name = "Metallica".to_owned();
    let r = results(vec![a], vec![]);
    let top = compute_top_result(&r, "master of puppets").expect("top result");
    assert_eq!(top.subtitle, TopSubtitle::Text("Metallica".to_owned()));
}

/// `top-kind` is a bare string crossing three files — Rust writes the
/// token, the card branches on it for the badge, the fallback glyph and
/// the genre gradient, and `results.rs` routes the click. A typo in any
/// one of them still builds and silently drops that kind back to a
/// default: the wrong badge, the wrong glyph, a dead card, or the grey
/// tile the genre gradient exists to replace. Nothing about that looks
/// wrong in either source.
#[test]
fn every_top_kind_token_reaches_the_view_and_the_router() {
    for token in ["album", "artist", "genre"] {
        assert!(
            CARD.contains(&format!("Search.top-kind == \"{token}\"")),
            "top-result-card.slint branches on no `{token}` top result"
        );
        assert!(
            ROUTER.contains(&format!("\"{token}\" =>")),
            "results.rs routes no click for a `{token}` top result"
        );
    }
}

/// Both of the tile's fills are ternaries, so neither arm can *fall
/// through* to the default it would otherwise inherit — all four values
/// are spelled out in the card, and all four can drift in silence.
///
/// The genre arm drifting stops the card matching that genre's grid card.
/// The other arm drifting stops it matching every track row and entity
/// card in the app — which is exactly how this tile came to be a lone
/// grey square while everything around it wore the accent placeholder.
/// So each arm is pinned against the file that owns it, and the two
/// bindings are matched whole: `top-kind == "genre"` appears three times
/// in the card, and a looser check would keep passing with a fill gone.
#[test]
fn the_top_tile_matches_artwork_image_and_the_genre_grid() {
    let view = squeeze(CARD);
    let component = squeeze(ARTWORK_IMAGE);
    let grid = squeeze(GENRE_GRID);

    let placeholder_bg = value_after(&component, "in property <brush> tile-bg:");
    let placeholder_icon = value_after(&component, "in property <brush> tile-icon-color:");
    let genre_icon = value_after(&grid, "tile-icon-color:");

    assert!(
        view.contains(&format!(
            "tile-bg: Search.top-kind == \"genre\" ? @linear-gradient(135deg, \
             Search.top-tile-color-1, Search.top-tile-color-2) : {placeholder_bg};"
        )),
        "the tile's fill no longer pairs the hashed gradient with \
         ArtworkImage's placeholder (`{placeholder_bg}`)"
    );
    assert!(
        view.contains(&format!(
            "tile-icon-color: Search.top-kind == \"genre\" ? {genre_icon} \
             : {placeholder_icon};"
        )),
        "the tile's glyph colour no longer pairs GenreGrid's \
         (`{genre_icon}`) with ArtworkImage's (`{placeholder_icon}`)"
    );
}

/// The card folds accents, because every other surface on the page already
/// does.
///
/// The Songs list and both strips come out of FTS, whose
/// `unicode61 remove_diacritics 2` tokenizer folds them — so a query typed
/// without the accent fills the page and, on a bare `to_lowercase`, missed
/// the exact-name band entirely. The card then fell through to rule 7 and
/// showed whatever album happened to sort first, which reads as the ranking
/// being wrong rather than as a folding bug.
#[test]
fn an_accent_stripped_query_still_wins_the_exact_band() {
    let r = results(
        vec![album(1, "Debut"), album(2, "Homogenic")],
        vec![artist(9, "Björk")],
    );

    let top = compute_top_result(&r, "bjork").expect("a top result");
    assert_eq!(top.kind, TopKind::Artist, "expected the exact artist match");
    assert_eq!(top.id, 9);
}

/// And the other direction: an accented query finds an unaccented name, since
/// the fold runs over both sides.
#[test]
fn an_accented_query_reaches_an_unaccented_name() {
    let r = results(vec![album(1, "Zoo")], vec![artist(9, "Beyonce")]);

    let top = compute_top_result(&r, "Beyoncé").expect("a top result");
    assert_eq!(top.kind, TopKind::Artist);
    assert_eq!(top.id, 9);
}

/// The prefix band folds too, and still ranks below every exact match.
#[test]
fn the_prefix_band_folds_and_stays_below_the_exact_band() {
    let prefix_only = results(vec![album(1, "Amelie Soundtrack")], vec![]);
    let top = compute_top_result(&prefix_only, "amélie").expect("a top result");
    assert_eq!(top.kind, TopKind::Album);
    assert_eq!(top.id, 1);

    // An accent-folded *exact* genre outranks an accent-folded album prefix,
    // exactly as the unaccented pair already did.
    let both = results_with_genres(
        vec![album(1, "Née Again")],
        vec![],
        vec![genre(7, "Née")],
    );
    let top = compute_top_result(&both, "nee").expect("a top result");
    assert_eq!(top.kind, TopKind::Genre, "exact genre must beat album prefix");
    assert_eq!(top.subtitle, TopSubtitle::TrackCount(genre(7, "Née").track_count));
}
