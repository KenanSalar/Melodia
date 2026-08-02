use super::{
    FavoritesTab, FavoritesUi, build_filtered_grids, grid_signature, mounted_content, set_artist_sort,
    should_announce_warm, sort_artists,
};
use crate::entities::artist::FavoriteArtist;
use crate::entities::track::MostPlayedFavorite;
use crate::media::cover_thumbs::CoverThumbs;
use crate::services::settings::SortDir;

/// Exhaustive struct literals on purpose: a new field on either entity fails
/// this file to compile, which is the prompt to check that it also belongs in
/// the derived `Hash` the grid compares against.
fn most_played(play_count: i32) -> MostPlayedFavorite {
    MostPlayedFavorite {
        id: 1,
        title: "Title".to_owned(),
        artist: Some("Artist".to_owned()),
        artwork_path: Some("/covers/track.jpg".to_owned()),
        play_count,
        duration_ms: 210_000,
    }
}

fn favorite_artist(name: &str, favorite_count: i32) -> FavoriteArtist {
    FavoriteArtist {
        id: 1,
        name: name.to_owned(),
        image_path: Some("/covers/artist.jpg".to_owned()),
        favorite_count,
    }
}

fn seeded(play_count: i32, favorite_count: i32) -> FavoritesUi {
    let fav_ui = FavoritesUi::new(std::sync::Arc::new(CoverThumbs::new()));
    *fav_ui.state().most_played.lock() = vec![most_played(play_count)];
    *fav_ui.state().fav_artists.lock() = vec![favorite_artist("Artist", favorite_count)];
    fav_ui
}

/// `(name, favorite_count)` pairs in the order [`sort_artists`] left them.
fn sorted_names(mut artists: Vec<FavoriteArtist>, field: &str, dir: SortDir) -> Vec<String> {
    sort_artists(&mut artists, field, dir);
    artists.into_iter().map(|a| a.name).collect()
}

/// Deliberately not in any of the orders under test, so a comparator that does
/// nothing fails rather than coincidentally passing.
fn unsorted() -> Vec<FavoriteArtist> {
    vec![
        favorite_artist("beach house", 4),
        favorite_artist("Aphex Twin", 9),
        favorite_artist("Chromatics", 4),
    ]
}

/// The whole point of hashing the two tabs apart. `stats_changed` fires on every
/// play-count flush and reaches both grids, but only Most Played is ranked by
/// play count and only its cards carry the badge — a shared hash would have
/// every flush tear down and rebuild the Artists grid for a number it doesn't
/// show.
#[test]
fn a_play_count_flush_moves_only_the_most_played_hash() {
    let before = build_filtered_grids(&seeded(5, 3));
    let after = build_filtered_grids(&seeded(6, 3));

    assert_ne!(
        before.most_played_content, after.most_played_content,
        "a play-count change must rebuild Most Played — it reranks the grid and retitles a badge"
    );
    assert_eq!(
        before.artists_content, after.artists_content,
        "a play-count change must not rebuild the Artists grid — nothing on those cards reads it"
    );

    // Hashing them apart only pays off if the mounted tab picks one, not both.
    assert_ne!(
        mounted_content(FavoritesTab::MostPlayed, &before),
        mounted_content(FavoritesTab::MostPlayed, &after),
        "Most Played must see its own change"
    );
    assert_eq!(
        mounted_content(FavoritesTab::Artists, &before),
        mounted_content(FavoritesTab::Artists, &after),
        "the Artists tab must not see a change that belongs to the other grid"
    );
}

/// The artist card's subtitle is a translated plural over `favorite_count`, so
/// that field has to reach the hash even though it never changes the card's
/// identity. Favouriting a song is a `library_changed` tick, not a stats one.
#[test]
fn a_favorite_count_change_moves_the_artists_hash() {
    let before = build_filtered_grids(&seeded(5, 3));
    let after = build_filtered_grids(&seeded(5, 4));

    assert_ne!(
        before.artists_content, after.artists_content,
        "the count behind the artist subtitle must rebuild the grid that renders it"
    );
}

/// Both shape what is on screen independently of the data — a tab switch fills
/// one model and empties the other, a column change re-chunks the same cards
/// into different rows. Leave either out of the signature and the apply that
/// most needs to run is the one that gets skipped.
#[test]
fn the_signature_folds_in_the_tab_and_the_column_count() {
    let base = grid_signature(FavoritesTab::Artists, 4, 7);

    assert_ne!(base, grid_signature(FavoritesTab::MostPlayed, 4, 7), "the tab must count");
    assert_ne!(base, grid_signature(FavoritesTab::Artists, 5, 7), "the column count must count");
    assert_ne!(base, grid_signature(FavoritesTab::Artists, 4, 8), "the contents must count");
}

/// Case-insensitive, so a lowercased name doesn't sort below every capitalised
/// one — which is the whole difference between this and a raw `sort()`.
#[test]
fn name_sort_is_case_insensitive_and_honours_the_direction() {
    assert_eq!(
        sorted_names(unsorted(), "name", SortDir::Asc),
        ["Aphex Twin", "beach house", "Chromatics"]
    );
    assert_eq!(
        sorted_names(unsorted(), "name", SortDir::Desc),
        ["Chromatics", "beach house", "Aphex Twin"]
    );
}

/// The default, and the order the SQL used to hand over — except that the SQL's
/// bare `ORDER BY favorite_count DESC` broke ties not at all, so the two
/// four-count artists could swap places between refreshes. Ties break by name.
#[test]
fn favorite_count_sort_breaks_ties_by_name() {
    assert_eq!(
        sorted_names(unsorted(), "favorite_count", SortDir::Asc),
        ["beach house", "Chromatics", "Aphex Twin"]
    );
    // `Desc` is a reverse of the ascending order, so the tie-break reverses with
    // it. Consistent either way, which is all the guarantee that's needed.
    assert_eq!(
        sorted_names(unsorted(), "favorite_count", SortDir::Desc),
        ["Aphex Twin", "Chromatics", "beach house"]
    );
}

/// An unknown field falls through to the default rather than leaving the rows
/// in whatever order they arrived — a `views.json` carrying a token from a
/// later build must still land on a defined order.
#[test]
fn an_unknown_sort_field_falls_back_to_favorite_count() {
    assert_eq!(
        sorted_names(unsorted(), "album_count", SortDir::Asc),
        sorted_names(unsorted(), "favorite_count", SortDir::Asc)
    );
}

/// `grid_signature` folds in the tab and the column count but not the sort, and
/// this is why it doesn't have to: `artists_content` is a *sequential* hash over
/// the survivors, so re-ordering the same artists moves it. Were it
/// order-insensitive, `write_filtered_grids` would recognise a re-sort as its
/// own output and skip the one apply that had to run.
#[test]
fn a_re_sort_moves_the_artists_hash() {
    let fav_ui = FavoritesUi::new(std::sync::Arc::new(CoverThumbs::new()));
    *fav_ui.state().fav_artists.lock() = unsorted();

    set_artist_sort(&fav_ui, "favorite_count".to_owned(), SortDir::Desc);
    let by_count = build_filtered_grids(&fav_ui).artists_content;

    set_artist_sort(&fav_ui, "name".to_owned(), SortDir::Asc);
    let by_name = build_filtered_grids(&fav_ui).artists_content;

    assert_ne!(
        by_count, by_name,
        "a sort change must move the content hash — the same cards in a new order is still a repaint"
    );
}

/// The re-enter case, and the one that was broken: the grid's rows can land
/// before the prewarm returns (the view's mount-time `columns-changed` writes
/// them), so by the time the decodes are done there is nothing left to repaint —
/// and the tier is warm regardless. Gating the announcement on the write left
/// `covers-generation` at its cold 0 and every card on a placeholder until the
/// next tab pick.
#[test]
fn a_landed_prewarm_announces_even_when_the_rows_did_not_move() {
    assert!(should_announce_warm(
        Some(FavoritesTab::MostPlayed),
        /* section_active */ true,
        FavoritesTab::MostPlayed,
    ));
}

/// A leave that landed mid-refresh has already rewound the counter and dropped
/// the buffers, so there is no tier to announce and nothing on screen to hear it.
#[test]
fn a_section_left_mid_refresh_announces_nothing() {
    assert!(!should_announce_warm(
        Some(FavoritesTab::MostPlayed),
        /* section_active */ false,
        FavoritesTab::MostPlayed,
    ));
    // `None` is the same refresh finding the section already hidden before it
    // ever spawned the prewarm — no decode ran, so nothing is warm.
    assert!(!should_announce_warm(None, true, FavoritesTab::MostPlayed));
}

/// A tab pick that overtook the decodes owns a different tier — `swap_tab_covers`
/// cleared the one this task warmed. Announcing it would put the entering tab's
/// cards straight back on the UI-thread decoding path.
#[test]
fn a_tab_pick_that_overtook_the_prewarm_announces_nothing() {
    assert!(!should_announce_warm(
        Some(FavoritesTab::MostPlayed),
        true,
        FavoritesTab::Artists,
    ));
}

/// The prewarm reads the cache, not the filtered copy, so the setter has to
/// leave the cache itself in display order.
#[test]
fn set_artist_sort_reorders_the_cache_the_prewarm_reads() {
    let fav_ui = FavoritesUi::new(std::sync::Arc::new(CoverThumbs::new()));
    *fav_ui.state().fav_artists.lock() = unsorted();

    set_artist_sort(&fav_ui, "name".to_owned(), SortDir::Asc);

    let names: Vec<String> =
        fav_ui.state().fav_artists.lock().iter().map(|a| a.name.clone()).collect();
    assert_eq!(names, ["Aphex Twin", "beach house", "Chromatics"]);
}
