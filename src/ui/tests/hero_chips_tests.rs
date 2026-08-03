use super::{
    ChipLabels, FavoritesFacts, HeroFold, MostPlayedTotals, album_chips, artist_chips,
    dominant_genre, favorites_chips, fold_most_played, fold_tracks, genre_chips, playlist_chips,
    recently_played_chips, year_span,
};
use crate::entities::album::AlbumStats;
use crate::entities::artist::ArtistStats;
use crate::entities::genre::GenreStats;
use crate::entities::playlist::PlaylistStats;
use crate::entities::track::{MostPlayedFavorite, TrackListRow};
use crate::ui::favorites::FavoritesTab;
use slint::SharedString;

const STRIP: &str = include_str!("../../../melodia-ui/ui/components/meta-chip-strip.slint");
const HERO_CHIPS: &str = include_str!("../hero_chips.rs");

/// Every view that mounts a chip strip on a hero band, and the name of the
/// meta-line property it replaced. Both halves matter — see the two pins below.
const HERO_VIEWS: [(&str, &str); 6] = [
    (
        include_str!("../../../melodia-ui/ui/views/album-detail-view.slint"),
        "album-detail-view.slint",
    ),
    (
        include_str!("../../../melodia-ui/ui/views/artist-detail-view.slint"),
        "artist-detail-view.slint",
    ),
    (
        include_str!("../../../melodia-ui/ui/views/genre-detail-view.slint"),
        "genre-detail-view.slint",
    ),
    (
        include_str!("../../../melodia-ui/ui/views/playlist-detail-view.slint"),
        "playlist-detail-view.slint",
    ),
    (
        include_str!("../../../melodia-ui/ui/views/favorites-view.slint"),
        "favorites-view.slint",
    ),
    (
        include_str!("../../../melodia-ui/ui/views/recently-played-view.slint"),
        "recently-played-view.slint",
    ),
];

/// Stands in for the `HeroChips` global. Production routes through Slint's
/// `@tr` so the plurals follow the running locale; here they only have to be
/// distinguishable, so the labels are the English singular-agnostic form.
struct EnglishLabels;

impl ChipLabels for EnglishLabels {
    fn tracks(&self, count: i32) -> SharedString {
        SharedString::from(format!("{count} tracks"))
    }
    fn albums(&self, count: i32) -> SharedString {
        SharedString::from(format!("{count} albums"))
    }
    fn artists(&self, count: i32) -> SharedString {
        SharedString::from(format!("{count} artists"))
    }
    fn favorites(&self, count: i32) -> SharedString {
        SharedString::from(format!("{count} favorites"))
    }
    fn discs(&self, count: i32) -> SharedString {
        SharedString::from(format!("{count} discs"))
    }
    fn plays(&self, count: i32) -> SharedString {
        SharedString::from(format!("{count} plays"))
    }
    fn compilation(&self) -> SharedString {
        SharedString::from("Compilation")
    }
}

/// Only the four fields the folds and the genre tally read.
fn track(artist_id: Option<i64>, album_id: Option<i64>, genre: Option<&str>) -> TrackListRow {
    TrackListRow {
        id: 0,
        file_path: String::new(),
        file_name: String::new(),
        title: String::new(),
        artist: None,
        album_artist: None,
        album: None,
        genre: genre.map(str::to_owned),
        track_number: None,
        disc_number: None,
        year: None,
        duration_ms: 0,
        artwork_path: None,
        is_favorite: false,
        rating: 0,
        album_id,
        artist_id,
        genre_id: None,
        date_added: String::new(),
        sort_key: None,
    }
}

fn played(duration_ms: i64, play_count: i32) -> MostPlayedFavorite {
    MostPlayedFavorite {
        id: 0,
        title: String::new(),
        artist: None,
        album_artist: None,
        album: None,
        genre: None,
        year: None,
        artwork_path: None,
        play_count,
        duration_ms,
    }
}

fn texts(chips: &[SharedString]) -> Vec<&str> {
    chips.iter().map(SharedString::as_str).collect()
}

fn album(year: Option<i32>, disc_count: Option<i32>, is_compilation: bool) -> AlbumStats {
    AlbumStats {
        id: 1,
        name: "Kind of Blue".into(),
        sort_name: None,
        artist_id: 1,
        artist_name: "Miles Davis".into(),
        year,
        disc_count,
        is_compilation,
        musicbrainz_id: None,
        artwork_path: None,
        track_count: 5,
        total_duration_ms: 2_733_000,
    }
}

#[test]
fn an_album_leads_with_its_year() {
    let chips = album_chips(&EnglishLabels, &album(Some(1959), None, false), None);
    assert_eq!(texts(&chips), vec!["1959", "5 tracks", "45:33"]);
}

#[test]
fn an_album_omits_the_facts_it_does_not_have() {
    // Year 0 is the "unknown" sentinel the projection writes, a single disc is
    // every album's default, and a non-compilation has nothing to say.
    let chips = album_chips(&EnglishLabels, &album(Some(0), Some(1), false), None);
    assert_eq!(texts(&chips), vec!["5 tracks", "45:33"]);

    let chips = album_chips(&EnglishLabels, &album(None, None, false), None);
    assert_eq!(texts(&chips), vec!["5 tracks", "45:33"]);
}

#[test]
fn an_album_states_its_discs_compilation_flag_and_genre_when_it_has_them() {
    // Genre last — the one chip here that isn't about this release, so it is
    // the one worth losing first when the band runs out of room.
    let chips = album_chips(&EnglishLabels, &album(Some(1998), Some(3), true), Some("Jazz"));
    assert_eq!(
        texts(&chips),
        vec!["1998", "5 tracks", "45:33", "3 discs", "Compilation", "Jazz"]
    );
}

#[test]
fn an_artist_states_albums_only_when_it_has_some() {
    let mut artist = ArtistStats {
        id: 1,
        name: "Miles Davis".into(),
        sort_name: None,
        musicbrainz_id: None,
        image_path: None,
        track_count: 12,
        album_count: 4,
        total_duration_ms: 3_600_000,
    };
    assert_eq!(
        texts(&artist_chips(&EnglishLabels, &artist, Some((1957, 1963)))),
        vec!["12 tracks", "4 albums", "1:00:00", "1957–1963"]
    );

    artist.album_count = 0;
    assert_eq!(
        texts(&artist_chips(&EnglishLabels, &artist, None)),
        vec!["12 tracks", "1:00:00"]
    );
}

#[test]
fn a_one_year_discography_reads_as_a_year_not_a_span() {
    let artist = ArtistStats {
        id: 1,
        name: "One Hit".into(),
        sort_name: None,
        musicbrainz_id: None,
        image_path: None,
        track_count: 3,
        album_count: 1,
        total_duration_ms: 600_000,
    };
    assert_eq!(
        texts(&artist_chips(&EnglishLabels, &artist, Some((1999, 1999)))),
        vec!["3 tracks", "1 albums", "10:00", "1999"]
    );
}

#[test]
fn a_genre_states_its_count_running_time_and_spread() {
    let genre = GenreStats {
        id: 1,
        name: "Jazz".into(),
        track_count: 30,
        total_duration_ms: 5_400_000,
    };
    assert_eq!(
        texts(&genre_chips(
            &EnglishLabels,
            &genre,
            HeroFold { artists: 12, albums: 18 }
        )),
        vec!["30 tracks", "1:30:00", "12 artists", "18 albums"]
    );
}

#[test]
fn a_spread_of_one_is_not_worth_a_chip() {
    // A single-artist, single-album list says nothing by saying "1 artist".
    let genre = GenreStats {
        id: 1,
        name: "Jazz".into(),
        track_count: 5,
        total_duration_ms: 600_000,
    };
    assert_eq!(
        texts(&genre_chips(
            &EnglishLabels,
            &genre,
            HeroFold { artists: 1, albums: 1 }
        )),
        vec!["5 tracks", "10:00"]
    );
}

#[test]
fn a_playlist_does_not_repeat_the_smart_badge() {
    // The title already carries `auto_awesome` when `is_smart`, so the chips
    // stay the same either way.
    let mut playlist = PlaylistStats {
        id: 1,
        name: "Focus".into(),
        description: None,
        thumbnail_path: None,
        is_smart: true,
        smart_criteria: None,
        created_at: String::new(),
        updated_at: String::new(),
        custom_thumbnail: false,
        track_count: 8,
        total_duration_ms: 1_800_000,
    };
    let fold = HeroFold { artists: 6, albums: 7 };
    let smart = texts(&playlist_chips(&EnglishLabels, &playlist, fold)).join("|");
    playlist.is_smart = false;
    let plain = texts(&playlist_chips(&EnglishLabels, &playlist, fold)).join("|");
    assert_eq!(smart, plain);
    assert_eq!(smart, "8 tracks|30:00|6 artists|7 albums");
}

#[test]
fn an_empty_entity_says_nothing_about_its_running_time() {
    let playlist = PlaylistStats {
        id: 1,
        name: "New Playlist".into(),
        description: None,
        thumbnail_path: None,
        is_smart: false,
        smart_criteria: None,
        created_at: String::new(),
        updated_at: String::new(),
        custom_thumbnail: false,
        track_count: 0,
        total_duration_ms: 0,
    };
    assert_eq!(
        texts(&playlist_chips(&EnglishLabels, &playlist, HeroFold::default())),
        vec!["0 tracks"]
    );
}

fn favorites_facts(tab: FavoritesTab) -> FavoritesFacts {
    FavoritesFacts {
        tab,
        tracks: 142,
        duration_ms: 33_273_000, // 9:14:33
        songs: HeroFold { artists: 37, albums: 51 },
        most_played: MostPlayedTotals {
            tracks: 88,
            duration_ms: 20_462_000,
            plays: 1204,
        },
        artists: 9,
    }
}

#[test]
fn the_favorites_chips_follow_the_tab() {
    assert_eq!(
        texts(&favorites_chips(
            &EnglishLabels,
            &favorites_facts(FavoritesTab::Songs)
        )),
        vec!["142 favorites", "9:14:33", "37 artists", "51 albums"]
    );
    assert_eq!(
        texts(&favorites_chips(
            &EnglishLabels,
            &favorites_facts(FavoritesTab::Artists)
        )),
        vec!["9 artists"]
    );
}

/// Most Played is `is_favorite AND play_count > 0` — a strict subset of Songs,
/// so it states its *own* duration. Borrowing the Songs one would overstate it
/// by every favourite never played, which is exactly the bug this pins.
#[test]
fn most_played_sums_itself_and_not_the_songs_tab() {
    let facts = favorites_facts(FavoritesTab::MostPlayed);
    assert_eq!(
        texts(&favorites_chips(&EnglishLabels, &facts)),
        vec!["88 tracks", "5:41:02", "1204 plays"]
    );
    let songs = texts(&favorites_chips(
        &EnglishLabels,
        &favorites_facts(FavoritesTab::Songs),
    ))[1]
    .to_owned();
    assert_ne!(
        texts(&favorites_chips(&EnglishLabels, &facts))[1],
        songs,
        "Most Played must not reuse the Songs tab's duration"
    );
}

/// A favourite that has never been played is still on the tab (the query is
/// `play_count > 0`, so in practice it isn't — but the chip is gated on the sum,
/// not on the query). Zero plays says nothing worth a chip.
#[test]
fn a_never_played_most_played_tab_states_no_plays() {
    let facts = FavoritesFacts {
        most_played: MostPlayedTotals { tracks: 3, duration_ms: 600_000, plays: 0 },
        ..favorites_facts(FavoritesTab::MostPlayed)
    };
    assert_eq!(
        texts(&favorites_chips(&EnglishLabels, &facts)),
        vec!["3 tracks", "10:00"]
    );
}

/// Every empty band leaves the copy to the view — a sentence on the Songs and
/// Recently Played heroes, a `GridEmptyState` on the two Favorites grids. A lone
/// "0 …" chip beside one of those is redundant at best.
#[test]
fn an_empty_hero_leaves_its_empty_state_to_the_view() {
    let empty = FavoritesFacts {
        tracks: 0,
        duration_ms: 0,
        songs: HeroFold::default(),
        most_played: MostPlayedTotals::default(),
        artists: 0,
        ..favorites_facts(FavoritesTab::Songs)
    };
    for tab in [
        FavoritesTab::Songs,
        FavoritesTab::MostPlayed,
        FavoritesTab::Artists,
    ] {
        let facts = FavoritesFacts { tab, ..empty };
        assert!(
            favorites_chips(&EnglishLabels, &facts).is_empty(),
            "the {tab:?} tab states a count over an empty list — the grid beneath it already \
             says so, and says it better"
        );
    }
    assert!(recently_played_chips(&EnglishLabels, 0, 0, HeroFold::default()).is_empty());
}

#[test]
fn recently_played_states_its_count_running_time_and_spread() {
    assert_eq!(
        texts(&recently_played_chips(
            &EnglishLabels,
            200,
            43_451_000, // 12:04:11
            HeroFold { artists: 44, albums: 60 }
        )),
        vec!["200 tracks", "12:04:11", "44 artists", "60 albums"]
    );
}

#[test]
fn the_fold_counts_distinct_ids_and_skips_the_untagged() {
    // A track with no album belongs to none, so it is skipped rather than
    // pooled into an "unknown" bucket that would read as one more album.
    let rows = [
        track(Some(1), Some(10), None),
        track(Some(1), Some(10), None),
        track(Some(2), Some(11), None),
        track(None, None, None),
    ];
    assert_eq!(fold_tracks(&rows), HeroFold { artists: 2, albums: 2 });
    assert_eq!(fold_tracks(&[]), HeroFold::default());
}

#[test]
fn most_played_totals_sum_duration_and_plays() {
    let rows = [played(180_000, 12), played(240_000, 30)];
    assert_eq!(
        fold_most_played(&rows),
        MostPlayedTotals {
            tracks: 2,
            duration_ms: 420_000,
            plays: 42,
        }
    );
}

#[test]
fn a_genre_is_named_only_when_it_actually_dominates() {
    let mostly_jazz = [
        track(None, None, Some("Jazz")),
        track(None, None, Some("Jazz")),
        track(None, None, Some("Blues")),
    ];
    assert_eq!(dominant_genre(&mostly_jazz).as_deref(), Some("Jazz"));

    // An even split has no majority — naming either would misrepresent the
    // other half, so a genuinely mixed compilation gets no chip.
    let split = [
        track(None, None, Some("Jazz")),
        track(None, None, Some("Blues")),
    ];
    assert_eq!(dominant_genre(&split), None);

    // Untagged tracks don't count toward the total, so one tagged track among
    // three still dominates the tracks that have a genre at all.
    let sparse = [
        track(None, None, Some("Jazz")),
        track(None, None, None),
        track(None, None, Some("")),
    ];
    assert_eq!(dominant_genre(&sparse).as_deref(), Some("Jazz"));
    assert_eq!(dominant_genre(&[]), None);
}

#[test]
fn the_year_span_ignores_albums_with_no_year() {
    let dated = |year: Option<i32>| AlbumStats { year, ..album(None, None, false) };
    assert_eq!(
        year_span(&[dated(Some(1963)), dated(None), dated(Some(1957))]),
        Some((1957, 1963))
    );
    assert_eq!(year_span(&[dated(Some(0)), dated(None)]), None);
    assert_eq!(year_span(&[]), None);
}

/// `MetaChipStrip`'s brushes default to `Theme.*`, which is correct nowhere and
/// wrong invisibly: a chip painting a theme token on a band whose colours are
/// solved per artwork looks plausible under Mocha and washes out under every
/// light variant.
///
/// The six heroes discharge that through `HeroChipStrip`, which fixes both
/// brushes so the wrong mount can't be spelled — this pins that they mount the
/// wrapper rather than reaching past it to the raw strip, and that the wrapper
/// still passes both. Now Playing keeps the raw mount, because its pair is
/// `Player.np-accent-*`, and carries the same obligation by hand.
#[test]
fn every_chip_strip_takes_its_brushes_from_its_backdrop() {
    const NOW_PLAYING: &str = include_str!("../../../melodia-ui/ui/views/now-playing-view.slint");
    const WRAPPER: &str = include_str!("../../../melodia-ui/ui/components/hero-chip-strip.slint");
    const CHIP: &str = include_str!("../../../melodia-ui/ui/components/meta-chip.slint");

    for (src, name) in HERO_VIEWS {
        assert!(
            src.contains("HeroChipStrip {"),
            "{name} must mount `HeroChipStrip` — reaching past it to `MetaChipStrip` reopens the \
             `Theme.*` defaults one mount at a time"
        );
        assert!(
            !src.contains("MetaChipStrip {"),
            "{name} still mounts a raw `MetaChipStrip`; the hero wrapper is what fixes the brushes"
        );
    }

    // Bounded by the `measured` handler rather than by a brace, so the
    // handler's own body can't end the window early.
    let mounts = [
        (WRAPPER, "hero-chip-strip.slint", "HeroBackdrop."),
        (NOW_PLAYING, "now-playing-view.slint", "Player.np-accent-"),
    ];
    for (src, name, tier) in mounts {
        let mount = src
            .split_once("MetaChipStrip {")
            .and_then(|(_, rest)| rest.split_once("measured("))
            .map_or("", |(body, _)| body);
        assert!(
            !mount.is_empty(),
            "{name} no longer mounts a `MetaChipStrip` ahead of its `measured` handler"
        );
        for prop in ["chip-fill", "chip-label-color"] {
            assert!(
                mount.contains(&format!("{prop}: {tier}")),
                "{name}'s MetaChipStrip must pass `{prop}` a `{tier}` tier — omitting it falls \
                 back to the component's `Theme.*` default, a theme value on a solved backdrop"
            );
        }
    }

    // The layering the wrapper exists to preserve: the strip and the chip stay
    // global-free, which is what lets one component sit on a blurred cover
    // *and* on a blurred banner.
    for (src, name) in [(STRIP, "meta-chip-strip.slint"), (CHIP, "meta-chip.slint")] {
        // Import lines only — both files *mention* `HeroBackdrop` in the prose
        // arguing why they don't reach for it.
        let imports = src.lines().filter(|l| l.trim_start().starts_with("import "));
        for line in imports {
            assert!(
                !line.contains("hero-backdrop.slint") && !line.contains("player.slint"),
                "{name} must import neither hero nor player globals — the defaulted brushes are \
                 what let the same component paint on both, and the wrapper is where a tier is \
                 chosen"
            );
        }
    }
}

/// The chips *replace* the meta line rather than joining it. A half-done
/// conversion leaves the old property and its `Text` in place, and the band
/// then says the same thing twice.
#[test]
fn no_hero_view_still_declares_a_meta_line() {
    for (src, name) in HERO_VIEWS {
        for prop in ["meta-line", "stats-line", "stats-text"] {
            assert!(
                !src.contains(&format!("property <string> {prop}")),
                "{name} still declares `{prop}` — the hero's counts are chips now, and a \
                 surviving meta line duplicates every one of them"
            );
        }
    }
}

/// The six banners are one band under different content, so the tile each leads
/// with is one number — `Theme.hero-artwork`.
///
/// Pinned beside the chip tests because `HERO_MAX_ROWS` is measured against that
/// tile: a view sizing its own artwork moves the slack the cap was picked for,
/// and it moves the band's height with it. Neither family of hero spells a size
/// any more — the four detail views route through `DetailHeader` and the two
/// mosaic heroes through `MosaicHeroTile`, and neither component exposes a knob
/// to override the token with.
///
/// The tile check has to name `MosaicHeroTile` rather than the two views: the
/// square moved into it, and each view's surviving `Theme.hero-artwork` is now
/// the *band height* (`2 * pad-lg + hero-artwork`), which is a different fact.
/// Asserted against the view alone, a `140px` hardcoded in the component passes.
#[test]
fn no_hero_view_sizes_its_own_artwork_tile() {
    const THEME: &str = include_str!("../../../melodia-ui/ui/theme.slint");
    const HEADER: &str = include_str!("../../../melodia-ui/ui/components/detail-header.slint");
    const MOSAIC_TILE: &str =
        include_str!("../../../melodia-ui/ui/components/hero/mosaic-hero-tile.slint");

    assert!(
        THEME.contains("out property <length> hero-artwork:"),
        "Theme must own the hero tile size — it is the only thing keeping the six bands aligned"
    );
    assert!(
        HEADER.contains("tile-size: Theme.hero-artwork;"),
        "DetailHeader must size its tile from the token"
    );
    assert!(
        !HEADER.contains("artwork-size"),
        "DetailHeader must not reintroduce a per-view size knob — four of the six bands would \
         then be free to drift from the two that have no header to route through"
    );
    for (src, name) in HERO_VIEWS {
        assert!(
            !src.contains("artwork-size"),
            "{name} sizes its hero tile itself — it belongs on `Theme.hero-artwork`"
        );
        assert!(
            !src.contains("property <length> hero-artwork"),
            "{name} restates the hero tile size as a local property"
        );
    }
    // The mosaic square itself, which both heroes now mount rather than draw.
    for axis in ["width", "height"] {
        assert!(
            MOSAIC_TILE.contains(&format!("{axis}: Theme.hero-artwork;")),
            "MosaicHeroTile must take its {axis} from `Theme.hero-artwork` — a literal here \
             drifts both mosaic heroes away from the four detail bands at once, and every \
             view-level check still passes because each one keeps the token for its band height"
        );
    }

    let mosaic = HERO_VIEWS
        .iter()
        .filter(|(_, name)| matches!(*name, "favorites-view.slint" | "recently-played-view.slint"));
    let mut checked = 0;
    for (src, name) in mosaic {
        assert!(
            src.contains("MosaicHeroTile {"),
            "{name} must mount `MosaicHeroTile` — drawing the square inline is what put the same \
             chrome in two files and let the two drift"
        );
        assert!(
            src.contains("Theme.hero-artwork"),
            "{name} derives its hero band height from the tile token, so it must still read it"
        );
        checked += 1;
    }
    assert_eq!(checked, 2, "both mosaic heroes must still be in HERO_VIEWS");
}

/// Favorites is the one hero assembled from three fetches rather than one, and
/// `kick_full_refresh` runs them concurrently — so no ordering can be assumed.
/// Each fetch folds its own answer and republishes; the fold and the republish
/// therefore belong to the same function as the fill.
///
/// Left to the grid path alone this is not covered: `write_filtered_grids`
/// publishes past a signature early-return, and `mounted_content` is a constant
/// `0` on the Songs tab, so there that publish fires only on a column change.
#[test]
fn each_favorites_fetch_folds_and_republishes_what_it_feeds() {
    // The fold call and the store are checked apart, not as one glued
    // statement: the walk belongs on this worker but *outside* the section
    // gate, so the two are deliberately separate statements and a pin that
    // demanded them joined would fail the correct code. Nothing is lost by
    // splitting them — a fold whose result never reaches the store is an unused
    // binding, which `-D warnings` already refuses to compile.
    const FILLS: [(&str, &str, &str, &str, &str); 2] = [
        (
            include_str!("../favorites/songs.rs"),
            "favorites/songs.rs",
            "pub async fn refresh_tracks(",
            "crate::ui::hero_chips::fold_tracks(&rows)",
            "songs_fold.lock() =",
        ),
        (
            include_str!("../favorites/grids/fetch.rs"),
            "favorites/grids/fetch.rs",
            "pub async fn refresh_grids(",
            "crate::ui::hero_chips::fold_most_played(&rows)",
            "most_played_totals.lock() =",
        ),
    ];

    for (src, name, func, fold, store) in FILLS {
        let body = src
            .split_once(func)
            .and_then(|(_, rest)| rest.split_once("\n}"))
            .map_or("", |(body, _)| body);
        // Whitespace-normalised, so the pin holds a *statement* rather than the
        // line breaks it happens to be wrapped at — a fold long enough to wrap
        // is exactly the one this has to keep catching.
        let body: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            body.contains(fold),
            "{name}: `{func}` must fold `&rows` — the local it just awaited. Folding at publish \
             time instead puts the walk on the UI thread and makes the answer depend on which \
             sibling fetch landed first"
        );
        assert!(
            body.contains(store),
            "{name}: `{func}` folds `&rows` but stores the result nowhere the band can read it"
        );
        assert!(
            body.contains("republish_chips("),
            "{name}: `{func}` moves a number the Favorites band states but never republishes \
             it — the sibling fetch that did publish ran against the previous fold, and nothing \
             corrects it"
        );
    }
}

/// The teardown half of the same contract: a fold may not outlive the cache it
/// was folded from.
///
/// `release_section_state` empties `tracks_all` / `most_played` / `fav_artists`
/// and zeroes `stats`, and the two folds have to go with them. Leaving them is
/// not the harmless failure it looks like — the band doesn't come back empty,
/// it comes back **wrong**: `kick_full_refresh` joins the three fetches and
/// `refresh_hero` is the shortest, so the first publish after a re-enter pairs
/// a fresh count with a pre-leave spread ("3 favorites · 37 artists") until
/// `refresh_tracks` lands. Folding at publish time used to hide this, because
/// the fold read a cache that *was* cleared.
#[test]
fn the_section_leave_drops_the_folds_with_the_caches_they_summarise() {
    const MOD: &str = include_str!("../favorites/mod.rs");

    let body = MOD
        .split_once("pub fn release_section_state(")
        .and_then(|(_, rest)| rest.split_once("\n    }"))
        .map_or("", |(body, _)| body);
    // Anchored on the statement the teardown ends with, so a window that
    // truncated early fails here rather than passing over nothing.
    assert!(
        body.contains("heap_trim::trim()"),
        "favorites/mod.rs no longer defines `release_section_state` as a body ending in \
         `heap_trim::trim()` — move this pin with it, or the checks below hold over an \
         empty window"
    );
    for (field, reset) in [
        ("songs_fold", "HeroFold::default()"),
        ("most_played_totals", "MostPlayedTotals::default()"),
    ] {
        assert!(
            body.contains(&format!("{field}.lock() = {reset}")),
            "`release_section_state` clears the caches `{field}` was folded from but leaves \
             `{field}` itself — the next publish then states a spread the count beside it no \
             longer matches"
        );
    }
}

/// No hero folds at publish time — every one of them folds a local its own
/// fetch just awaited, on that worker. A fold whose argument is a `.lock()` is
/// reading a cache some *other* task fills, which is both a UI-thread walk and
/// an ordering assumption.
///
/// `hero_chips.rs` is in the list precisely because it is where the temptation
/// lives: it holds the `FavoritesUi` handle, so every cache is one `.lock()`
/// away.
#[test]
fn no_hero_folds_out_of_a_shared_cache() {
    const FOLDERS: [(&str, &str); 6] = [
        (include_str!("../albums/detail.rs"), "albums/detail.rs"),
        (include_str!("../artists/detail.rs"), "artists/detail.rs"),
        (include_str!("../genres/detail.rs"), "genres/detail.rs"),
        (include_str!("../playlists/detail.rs"), "playlists/detail.rs"),
        (
            include_str!("../recently_played/tracks.rs"),
            "recently_played/tracks.rs",
        ),
        (HERO_CHIPS, "hero_chips.rs"),
    ];

    for (src, name) in FOLDERS {
        for fold in [
            "fold_tracks(",
            "fold_most_played(",
            "dominant_genre(",
            "year_span(",
        ] {
            for (i, m) in src.match_indices(fold) {
                let tail = &src[i + m.len()..];
                // Scan to the first `)`, which a `.lock()` argument reaches
                // *inside* itself — hence matching `.lock(` rather than the
                // full call, or the very shape being banned slips through.
                let args = tail.find(')').map_or(tail, |end| &tail[..end]);
                assert!(
                    !args.contains(".lock("),
                    "{name}: `{fold}` reads a shared cache — fold the rows this function \
                     awaited instead, or it inherits the Favorites ordering problem"
                );
            }
        }
    }
}

/// A publisher reads no Slint property it doesn't own.
///
/// Both mosaic heroes used to take their counts back off the global the caller
/// had just written, which made the write order load-bearing and left a
/// "after the counts, never before" comment standing in for a compiler check.
/// Facts arrive as arguments, or off the section's own handle.
#[test]
fn no_publisher_reads_its_facts_back_off_a_slint_global() {
    for publisher in ["pub fn publish_favorites(", "pub fn publish_recently_played("] {
        let body = HERO_CHIPS
            .split_once(publisher)
            .and_then(|(_, rest)| rest.split_once("\n}"))
            .map_or("", |(body, _)| body);
        // Anchored on the call every publisher ends with, so a window that
        // truncated early would fail here rather than pass by holding nothing.
        assert!(
            body.contains("publish(ui, chips,"),
            "hero_chips.rs no longer defines `{publisher}` as a body ending in `publish(...)` — \
             move this pin with it, or the checks below hold over an empty window"
        );
        for getter in ["get_track_count(", "get_duration_text(", "get_artist_count("] {
            assert!(
                !body.contains(getter),
                "`{publisher}` reads `{getter}` back off a Slint global. Take the fact as an \
                 argument or off the handle — reading it back makes the caller's write order \
                 part of the contract, and nothing enforces that"
            );
        }
    }
}

/// The strip owns the width mirror and the mount seed so that no host has to
/// restate either. Both are documented pitfalls: `changed` rejects a path
/// expression, and it does not fire on the first binding evaluation — so a
/// window opened at its final size would never report a width, and every hero
/// would collapse its chips into the single unlaid-out row forever.
#[test]
fn the_strip_reports_its_width_on_resize_and_at_mount() {
    assert!(
        STRIP.contains("property <length> watched-w: self.width;")
            && STRIP.contains("changed watched-w =>"),
        "MetaChipStrip must mirror its width into a local property — `changed` cannot watch \
         `self.width` through a path expression"
    );
    assert!(
        STRIP.contains("Timer {") && STRIP.contains("interval: 1ms;"),
        "MetaChipStrip must keep its 1 ms mount Timer — `changed` doesn't fire when the first \
         layout settles directly on the final value, which is every window opened at its own size"
    );
    assert_eq!(
        STRIP.matches("root.measured(").count(),
        2,
        "MetaChipStrip should report through `measured` exactly twice: once from the resize \
         handler and once from the mount seed"
    );
}

/// The strip's rows must sit under a plain layout, not directly under the root.
///
/// A root inheriting `Rectangle` folds a child *layout* into its own layout
/// info but not a conditional one — an `if` lowers to a repeater, which
/// `gen_layout_info_prop` skips. With the `if` as the direct child the strip
/// reported `preferred: 0`, so a `VerticalLayout` handed it its 26 px
/// `min-height` and every row past the first drew *outside* that box: onto the
/// hero's action pill, or through the bottom of a fixed-height band. Nothing
/// about it fails to build, and it only shows once the chips actually wrap —
/// a narrow window, or a locale whose plurals run long.
///
/// Pinned as a shape rather than by counting braces: what matters is that
/// something non-conditional stands between the root and the `if`.
#[test]
fn the_strip_rows_hang_off_a_layout_not_off_the_root() {
    let normalized: String = STRIP.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized.contains("VerticalLayout { if root.show-rows:"),
        "MetaChipStrip must wrap its conditional rows in a plain layout — without it the root \
         reports `preferred: 0` vertically and a wrapped second row paints outside the strip. \
         See the `if`-child entry in `.claude/rules/slint-pitfalls.md`"
    );
}

/// ...and that wrapper owes a `min-width: 0px`, which is the same decision seen
/// from the other axis.
///
/// A layout child is folded into a `Rectangle` root's layout info in *both*
/// orientations, so the wrapper above hands the root a horizontal `min` equal to
/// the widest published row. A box layout never shrinks a cell past its `min`
/// (`i-slint-core::layout::layout_items`), so the strip keeps being allotted the
/// width it was chunked at — and keeps reporting that width through `measured` —
/// while the panel around it narrows: the chunker never learns there is less
/// room, and the row clips against the panel edge instead of wrapping. Which
/// makes `HERO_MAX_ROWS` reachable only by *mounting* narrow.
///
/// An explicit `min-width` replaces the merged constraint rather than maxing
/// with it, exactly as the `min-height` beside it does — so this one line is the
/// whole fix. `settings/chip-group.slint` carries it for the same reason.
#[test]
fn the_strip_leaks_no_width_floor() {
    assert!(
        STRIP.contains("min-width: 0px;"),
        "MetaChipStrip must pin `min-width: 0px` — its rows wrapper is a real layout, so without \
         it the widest chip row becomes a floor the hero column can't shrink past, and the chips \
         clip on a narrowing window instead of wrapping into a second row"
    );
}

/// The strip's two gaps are different numbers, and only the horizontal one
/// crosses into Rust.
///
/// `ui::chips::SPACING` restates the *inter-chip* gap because the wrap packs
/// each row against it; the gap between rows is Slint's alone. Collapsing the
/// two back into one `pad-sm` is the tempting simplification and it costs the
/// hero band 4 px it doesn't have — a wrapped second row then overruns the
/// artwork tile and grows the whole banner. Going the other way is worse and
/// quieter: `pad-xs` between chips leaves `SPACING` over-estimating every row,
/// so the strip wraps earlier than it needs to and drops chips that would fit.
#[test]
fn the_strip_spaces_its_rows_tighter_than_its_chips() {
    // Split on the repeater, so each half holds exactly one `spacing:` — the
    // rows layout's before it, the row layout's after.
    let (rows, chips) = STRIP
        .split_once("for row in root.rows:")
        .unwrap_or_default();

    let row_gap = rows.rsplit_once("spacing: Theme.").map_or("", |(_, gap)| gap);
    assert!(
        row_gap.starts_with("pad-xs;"),
        "the gap *between* chip rows must be `pad-xs` — at `pad-sm` a wrapped second row \
         overruns the hero's artwork tile and the band grows on every wrap"
    );

    let chip_gap = chips.split_once("spacing: Theme.").map_or("", |(_, gap)| gap);
    assert!(
        chip_gap.starts_with("pad-sm;"),
        "the gap *between chips within a row* must stay `pad-sm` — `ui::chips::SPACING` is 8.0 \
         and packs every row against exactly this number, so a change here desynchronises the \
         wrap silently"
    );
}

/// Album and Playlist are the two heroes carrying a second text line, and both
/// keep it *inside* the title row — under the title, beside the `SearchBar`.
///
/// That placement is the whole of `HERO_MAX_ROWS`' headroom, not a styling
/// choice. The title row is as tall as the `SearchBar` and `hero-title-size`
/// leaves several of those pixels unused, so a line nested there is nearly
/// free; on a row of its own it costs its full line box plus a `pad-xs` gap,
/// which is about what a wrapped chip row needs. Promoted back out, the band
/// still looks right and simply grows every time the chips wrap — which takes a
/// narrow window or a long-plural locale to notice.
///
/// Pinned by position rather than by nesting depth: the line must come before
/// the title row's `Rectangle { horizontal-stretch: 1; }`, the spacer that pins
/// the `SearchBar` right. A subtitle promoted back to a direct child of
/// `DetailHeader` lands after the entire row, and so after that spacer.
#[test]
fn the_two_subtitled_heroes_keep_that_line_inside_the_title_row() {
    const TITLE_ROW_SPACER: &str = "Rectangle { horizontal-stretch: 1; }";
    const SUBTITLES: [(&str, &str); 2] = [
        (
            "album-detail-view.slint",
            "if AlbumDetail.album.artist_name != \"\": Text {",
        ),
        (
            "playlist-detail-view.slint",
            "if PlaylistDetail.playlist.description != \"\": Text {",
        ),
    ];

    for (name, subtitle) in SUBTITLES {
        let src = HERO_VIEWS
            .iter()
            .find(|(_, view)| *view == name)
            .map_or("", |(src, _)| *src);
        let normalized: String = src.split_whitespace().collect::<Vec<_>>().join(" ");

        let title_block = normalized
            .split_once(TITLE_ROW_SPACER)
            .map_or("", |(before, _)| before);
        assert!(
            title_block.contains(subtitle),
            "{name}'s second line must be declared before the title row's stretch spacer, i.e. \
             inside the title block beside the SearchBar. After it, the line is back on a row of \
             its own — which spends about a wrapped chip row's worth of the hero tile, so the \
             band grows on every wrap. See `ui::hero_chips::HERO_MAX_ROWS`"
        );

        let block = normalized
            .split_once(subtitle)
            .and_then(|(_, rest)| rest.split_once('}'))
            .map_or("", |(block, _)| block);
        assert!(
            block.contains("font-size: Theme.font-size-md;"),
            "{name}'s second line must stay at `font-size-md` — the title row has only the \
             SearchBar's leftover height to lend it, and `hero-title-size` plus a larger line \
             spends the slack `HERO_MAX_ROWS` is measured against"
        );
    }
}

/// All six hero titles are one number, and it lives on `Theme`.
///
/// The same argument `hero-artwork` makes: the banners are the same band under
/// different content, so a title that changes size between them reads as the
/// page shifting under a nav. This is not hypothetical — the six had already
/// drifted to *two* sizes, the four detail views at 24 px against the two
/// mosaic heroes at 28 px, with nothing recording which was intended.
///
/// A literal is what reintroduces that, and a literal is invisible in review
/// because each view reads correctly on its own.
#[test]
fn every_hero_title_reads_the_same_token() {
    const THEME: &str = include_str!("../../../melodia-ui/ui/theme.slint");
    // A hero title is the one `Text` in each view carrying
    // `page-title-weight`, and its size is always the binding directly above.
    // Counting the pair against the weight alone is what catches a literal:
    // the view still builds, still looks right on its own, and only diverges
    // from the other five.
    const WEIGHT: &str = "font-weight: Theme.page-title-weight;";
    const TITLE: &str = "font-size: Theme.hero-title-size; font-weight: Theme.page-title-weight;";

    assert!(
        THEME.contains("out property <length> hero-title-size:"),
        "Theme must own the hero title size — it is the only thing keeping the six banners' \
         headings one size"
    );
    for (src, name) in HERO_VIEWS {
        let normalized: String = src.split_whitespace().collect::<Vec<_>>().join(" ");
        let titles = normalized.matches(WEIGHT).count();
        assert!(titles > 0, "{name} declares no hero title");
        assert_eq!(
            normalized.matches(TITLE).count(),
            titles,
            "{name} sizes a hero title with something other than `Theme.hero-title-size` — the \
             six banners are one band under different content, and they had already drifted to \
             two sizes once"
        );
    }
}
