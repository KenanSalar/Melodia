use super::apply::build_filtered_grids;
use super::sort::{set_artist_sort, sort_artists};
use super::warm::mounted_content;
use crate::entities::artist::FavoriteArtist;
use crate::entities::track::MostPlayedFavorite;
use crate::media::image::cover_thumbs::CoverThumbs;
use crate::services::settings::SortDir;
use crate::ui::favorites::{FavoritesTab, FavoritesUi};

/// Exhaustive struct literals on purpose: a new field on either entity fails
/// this file to compile, which is the prompt to check that it also belongs in
/// the derived `Hash` the grid compares against.
fn most_played(play_count: i32) -> MostPlayedFavorite {
    MostPlayedFavorite {
        id: 1,
        title: "Title".to_owned(),
        artist: Some("Artist".to_owned()),
        album_artist: None,
        album: Some("Album".to_owned()),
        genre: Some("Shoegaze".to_owned()),
        year: Some(1991),
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
    let fav_ui = FavoritesUi::new(std::sync::Arc::new(CoverThumbs::new()), None);
    *fav_ui.state().most_played.lock() = vec![most_played(play_count)];
    *fav_ui.state().fav_artists.lock() = vec![favorite_artist("Artist", favorite_count)];
    fav_ui
}

/// The same, with `tab` mounted. Only the mounted tab's cache is walked, so a
/// test asserting anything about a grid's rows, count or hash has to say which
/// tab it is asking from.
fn seeded_on(tab: FavoritesTab, play_count: i32, favorite_count: i32) -> FavoritesUi {
    let fav_ui = seeded(play_count, favorite_count);
    fav_ui.set_active_tab(tab);
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
    let before = build_filtered_grids(&seeded_on(FavoritesTab::MostPlayed, 5, 3));
    let after = build_filtered_grids(&seeded_on(FavoritesTab::MostPlayed, 6, 3));
    assert_ne!(
        before.most_played_content, after.most_played_content,
        "a play-count change must rebuild Most Played — it reranks the grid and retitles a badge"
    );
    assert_ne!(
        mounted_content(FavoritesTab::MostPlayed, &before),
        mounted_content(FavoritesTab::MostPlayed, &after),
        "Most Played must see its own change"
    );

    // Asked from the Artists tab, where that grid *is* the one walked — the tab
    // the flush must leave alone.
    let before = build_filtered_grids(&seeded_on(FavoritesTab::Artists, 5, 3));
    let after = build_filtered_grids(&seeded_on(FavoritesTab::Artists, 6, 3));
    assert_eq!(
        before.artists_content, after.artists_content,
        "a play-count change must not rebuild the Artists grid — nothing on those cards reads it"
    );
    assert_ne!(
        before.artists_content, 0,
        "…and it must be a real hash, else this passes on a grid that walked nothing"
    );
    assert_eq!(
        mounted_content(FavoritesTab::Artists, &before),
        mounted_content(FavoritesTab::Artists, &after),
        "the Artists tab must not see a change that belongs to the other grid"
    );
}

/// The two grids are mutually exclusive `if`s, so a row built for the tab that
/// isn't mounted reaches nothing and is dropped — which `write_filtered_grids`
/// always knew, one layer too late to stop the allocation. The Songs tab is the
/// case that paid for both.
///
/// The mutation this catches is building the wrong tab: swap the two arms and
/// every grid still paints, because the *write* side would silently hand the
/// mounted model an empty Vec.
#[test]
fn only_the_mounted_tabs_rows_are_built() {
    let fav_ui = seeded(7, 3);

    fav_ui.set_active_tab(FavoritesTab::MostPlayed);
    let on_most_played = build_filtered_grids(&fav_ui);
    assert_eq!(on_most_played.most_played.len(), 1, "the mounted grid owes its rows");
    assert!(on_most_played.artists.is_empty(), "the hidden grid must build none");

    fav_ui.set_active_tab(FavoritesTab::Artists);
    let on_artists = build_filtered_grids(&fav_ui);
    assert!(on_artists.most_played.is_empty(), "the hidden grid must build none");
    assert_eq!(on_artists.artists.len(), 1, "the mounted grid owes its rows");

    fav_ui.set_active_tab(FavoritesTab::Songs);
    let on_songs = build_filtered_grids(&fav_ui);
    assert!(
        on_songs.most_played.is_empty() && on_songs.artists.is_empty(),
        "Songs mounts neither grid, so it must build neither"
    );
}

/// **The count follows the rows, and so does the hash — an unmounted grid is not
/// walked at all.**
///
/// Both used to be published on every tab, on the reasoning that the counts gate
/// the two `GridEmptyState`s and feed the hero band. Only the first half is
/// true, and it is true *within* a tab: each count's readers sit inside its own
/// tab's branch (`favorites/{most-played,artists}-tab.slint`, plus the two pill
/// rows in `favorites-view.slint`, all gated on `tab-idx`), and the band takes
/// its facts from the folds on `FavoritesUiState`. So the extra walk answered
/// nobody — while folding the needle against every cached entity and pushing
/// each survivor's strings through a hasher, on the UI thread whenever
/// `apply_filtered_grids_now` is the caller.
///
/// The mutation to catch is restoring the both-tabs walk. Nothing visible
/// breaks, so only the cost comes back.
#[test]
fn only_the_mounted_tabs_cache_is_walked() {
    for (tab, most_played, artists) in [
        (FavoritesTab::Songs, 0, 0),
        (FavoritesTab::MostPlayed, 1, 0),
        (FavoritesTab::Artists, 0, 1),
    ] {
        let prepared = build_filtered_grids(&seeded_on(tab, 7, 3));
        assert_eq!(prepared.most_played_count, most_played, "Most Played count on {tab:?}");
        assert_eq!(prepared.artists_count, artists, "Artists count on {tab:?}");
        assert_eq!(
            prepared.most_played_content == 0,
            most_played == 0,
            "the Most Played hash must be taken exactly when its cache is walked, on {tab:?}"
        );
        assert_eq!(
            prepared.artists_content == 0,
            artists == 0,
            "the Artists hash must be taken exactly when its cache is walked, on {tab:?}"
        );
    }
}

/// A hash is taken off the *source* entities rather than the built rows, so a
/// re-mount of the same tab over the same cache answers identically. If it
/// didn't, `grid_signature` — which already folds the tab in separately — could
/// not tell a pick from a data change, and the apply that has to run on a pick
/// is exactly the one that would then be skipped.
#[test]
fn re_mounting_a_tab_over_the_same_cache_moves_no_hash() {
    for tab in [FavoritesTab::MostPlayed, FavoritesTab::Artists] {
        let fav_ui = seeded_on(tab, 7, 3);
        let first = build_filtered_grids(&fav_ui);

        fav_ui.set_active_tab(FavoritesTab::Songs);
        let _ = build_filtered_grids(&fav_ui);
        fav_ui.set_active_tab(tab);
        let second = build_filtered_grids(&fav_ui);

        assert_eq!(
            mounted_content(tab, &first),
            mounted_content(tab, &second),
            "{tab:?} must hash the same cache to the same value across a pick away and back"
        );
        assert_ne!(mounted_content(tab, &first), 0, "{tab:?} must walk something to hash");
    }
}

/// The artist card's subtitle is a translated plural over `favorite_count`, so
/// that field has to reach the hash even though it never changes the card's
/// identity. Favouriting a song is a `library_changed` tick, not a stats one.
#[test]
fn a_favorite_count_change_moves_the_artists_hash() {
    let before = build_filtered_grids(&seeded_on(FavoritesTab::Artists, 5, 3));
    let after = build_filtered_grids(&seeded_on(FavoritesTab::Artists, 5, 4));

    assert_ne!(
        before.artists_content, after.artists_content,
        "the count behind the artist subtitle must rebuild the grid that renders it"
    );
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
    let fav_ui = FavoritesUi::new(std::sync::Arc::new(CoverThumbs::new()), None);
    *fav_ui.state().fav_artists.lock() = unsorted();
    // The grid is only walked while its own tab is mounted, and a re-sort is a
    // thing the user does from that tab.
    fav_ui.set_active_tab(FavoritesTab::Artists);

    set_artist_sort(&fav_ui, "favorite_count".to_owned(), SortDir::Desc);
    let by_count = build_filtered_grids(&fav_ui).artists_content;

    set_artist_sort(&fav_ui, "name".to_owned(), SortDir::Asc);
    let by_name = build_filtered_grids(&fav_ui).artists_content;

    assert_ne!(
        by_count, by_name,
        "a sort change must move the content hash — the same cards in a new order is still a repaint"
    );
}

/// The prewarm reads the cache, not the filtered copy, so the setter has to
/// leave the cache itself in display order.
#[test]
fn set_artist_sort_reorders_the_cache_the_prewarm_reads() {
    let fav_ui = FavoritesUi::new(std::sync::Arc::new(CoverThumbs::new()), None);
    *fav_ui.state().fav_artists.lock() = unsorted();

    set_artist_sort(&fav_ui, "name".to_owned(), SortDir::Asc);

    let names: Vec<String> =
        fav_ui.state().fav_artists.lock().iter().map(|a| a.name.clone()).collect();
    assert_eq!(names, ["Aphex Twin", "beach house", "Chromatics"]);
}
