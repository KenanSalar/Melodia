use super::{FavoritesTab, FavoritesUi, build_filtered_grids, grid_signature, mounted_content};
use crate::entities::artist::FavoriteArtist;
use crate::entities::track::MostPlayedFavorite;
use crate::media::cover_thumbs::CoverThumbs;

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

fn favorite_artist(favorite_count: i32) -> FavoriteArtist {
    FavoriteArtist {
        id: 1,
        name: "Artist".to_owned(),
        image_path: Some("/covers/artist.jpg".to_owned()),
        favorite_count,
    }
}

fn seeded(play_count: i32, favorite_count: i32) -> FavoritesUi {
    let fav_ui = FavoritesUi::new(std::sync::Arc::new(CoverThumbs::new()));
    *fav_ui.state().most_played.lock() = vec![most_played(play_count)];
    *fav_ui.state().fav_artists.lock() = vec![favorite_artist(favorite_count)];
    fav_ui
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
