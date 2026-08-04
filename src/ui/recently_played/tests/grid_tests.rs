use super::apply::build_filtered_grid;
use super::warm::mounted_content;
use crate::entities::track::MostPlayedFavorite;
use crate::media::cover_thumbs::CoverThumbs;
use crate::ui::recently_played::{RecentlyPlayedTab, RecentlyPlayedUi};

/// An exhaustive struct literal on purpose: a new field on the entity fails this
/// file to compile, which is the prompt to check that it also belongs in the
/// derived `Hash` the grid compares against.
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

/// A handle with one played track cached and `tab` mounted.
fn seeded(tab: RecentlyPlayedTab, play_count: i32) -> RecentlyPlayedUi {
    let rp_ui = RecentlyPlayedUi::new(std::sync::Arc::new(CoverThumbs::new()));
    *rp_ui.state().most_played.lock() = vec![most_played(play_count)];
    rp_ui.set_active_tab(tab);
    rp_ui
}

/// The two sub-views are mutually exclusive `if`s, so a card row built for the
/// unmounted one reaches a grid nothing can scroll — one `EntityStripRow` of
/// `SharedString`s per played track in the library, built on the UI thread per
/// throttled keystroke and per `stats_changed` tick.
///
/// The mutation to catch is building the *wrong* tab, which the write side would
/// absorb in silence: it drops a `PreparedGrid` whose tab is no longer mounted,
/// so the grid would simply never fill and look like a fetch problem.
#[test]
fn only_the_mounted_tabs_rows_are_built() {
    assert_eq!(
        build_filtered_grid(&seeded(RecentlyPlayedTab::MostPlayed, 3))
            .most_played
            .len(),
        1,
        "the Most Played tab must build its own rows"
    );
    assert!(
        build_filtered_grid(&seeded(RecentlyPlayedTab::Songs, 3))
            .most_played
            .is_empty(),
        "the Songs tab mounts no grid, so building card rows for one is pure cost"
    );
}

/// The count is the half that *is* owed on both tabs: it gates the
/// `GridEmptyState` and feeds the hero band, and counting costs nothing extra
/// because the walk that hashes runs either way.
#[test]
fn the_count_is_published_whichever_tab_is_mounted() {
    for tab in [RecentlyPlayedTab::Songs, RecentlyPlayedTab::MostPlayed] {
        assert_eq!(
            build_filtered_grid(&seeded(tab, 3)).most_played_count,
            1,
            "{tab:?} must still publish the filtered card count"
        );
    }
}

/// The hash comes off the *source* entities rather than the built rows, which is
/// what lets the rows be built for one tab while the signature stays answerable
/// for both. Take it off the rows and the Songs tab reports a constant hash, so
/// a play-count flush arriving there is indistinguishable from nothing having
/// moved — and the first apply after switching back skips.
#[test]
fn the_content_hash_survives_the_tab_that_built_no_rows() {
    let on_grid = build_filtered_grid(&seeded(RecentlyPlayedTab::MostPlayed, 3));
    let on_songs = build_filtered_grid(&seeded(RecentlyPlayedTab::Songs, 3));
    assert_eq!(
        on_grid.most_played_content, on_songs.most_played_content,
        "the content hash must be tab-independent — `grid_signature` already folds the tab in \
         separately, so hashing it twice would make a pick indistinguishable from a data change"
    );

    let bumped = build_filtered_grid(&seeded(RecentlyPlayedTab::Songs, 4));
    assert_ne!(
        on_songs.most_played_content, bumped.most_played_content,
        "a play-count flush must move the hash even while the grid is unmounted"
    );
}

/// The Songs tab mounts no grid, so nothing the grid could rebuild is on screen
/// there. A constant `0` is what stops a `stats_changed` tick — which reaches
/// both tabs — forcing a `set_vec` reset nobody can see.
#[test]
fn the_songs_tab_contributes_no_mounted_content() {
    let prepared = build_filtered_grid(&seeded(RecentlyPlayedTab::Songs, 3));
    assert_eq!(mounted_content(RecentlyPlayedTab::Songs, &prepared), 0);
    assert_eq!(
        mounted_content(RecentlyPlayedTab::MostPlayed, &prepared),
        prepared.most_played_content,
        "the mounted grid must contribute its own hash"
    );
}

/// The grid narrows with the hero search bar, so the ids a card hands to the
/// queue have to come through the same predicate the model build uses. Walking
/// the raw cache would enqueue cards that aren't on screen.
#[test]
fn the_queue_ids_follow_the_same_filter_as_the_cards() {
    let rp_ui = seeded(RecentlyPlayedTab::MostPlayed, 3);
    crate::ui::recently_played::set_filter(&rp_ui, "shoegaze");
    assert_eq!(
        build_filtered_grid(&rp_ui).most_played_count,
        rp_ui.most_played_track_ids().len(),
        "the cards and the queue they load must agree about what's on screen"
    );

    crate::ui::recently_played::set_filter(&rp_ui, "nothing matches this");
    assert_eq!(build_filtered_grid(&rp_ui).most_played_count, 0);
    assert!(rp_ui.most_played_track_ids().is_empty());
}
