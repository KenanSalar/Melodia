use super::*;
use crate::entities::track::TrackListRow as RsTrackListRow;

fn folder(name: &str, path: &str) -> BrowseFolder {
    BrowseFolder {
        name: name.to_owned(),
        path: path.to_owned(),
    }
}

fn file(id: i64, title: &str, artist: Option<&str>, in_library: bool) -> BrowseFile {
    BrowseFile {
        row: RsTrackListRow {
            id,
            file_path: format!("/music/{title}.flac"),
            file_name: format!("{title}.flac"),
            title: title.to_owned(),
            artist: artist.map(str::to_owned),
            album_artist: None,
            album: None,
            genre: None,
            track_number: None,
            disc_number: None,
            year: None,
            duration_ms: 0,
            artwork_path: Some(format!("/art/{title}.jpg")),
            is_favorite: false,
            rating: 0,
            album_id: None,
            artist_id: None,
            genre_id: None,
            date_added: "2026-01-01T00:00:00Z".to_owned(),
            sort_key: Some(title.to_lowercase()),
        },
        in_library,
    }
}

#[test]
fn folders_lead_and_every_file_keeps_its_row_index() {
    let folders = [folder("Live Sets", "/music/live"), folder("Demos", "/music/demos")];
    let files = [
        file(7, "One", Some("Artist"), true),
        file(9, "Two", Some("Artist"), true),
    ];

    let cards = to_browse_card_rows(&folders, &files);

    assert_eq!(cards.len(), 4);
    assert!(cards[0].is_folder);
    assert!(cards[1].is_folder);
    assert_eq!(cards[0].title.as_str(), "Live Sets");
    assert_eq!(cards[0].path.as_str(), "/music/live");
    // A folder carries no row to index into, and its subtitle is supplied as a
    // translated literal by the grid delegate rather than pushed from here.
    assert_eq!(cards[0].row_index, -1);
    assert_eq!(cards[0].subtitle.as_str(), "");

    // `row_index` indexes `files`, not the card list — the folders ahead of them
    // must not shift it, or a card activation would play the wrong track.
    assert!(!cards[2].is_folder);
    assert_eq!(cards[2].row_index, 0);
    assert_eq!(cards[2].id, 7);
    assert_eq!(cards[3].row_index, 1);
    assert_eq!(cards[3].id, 9);
    assert_eq!(cards[2].subtitle.as_str(), "Artist");
    assert_eq!(cards[2].artwork_path.as_str(), "/art/One.jpg");
}

#[test]
fn a_disk_only_file_becomes_an_inert_card() {
    let files = [file(0, "Stray", None, false)];

    let cards = to_browse_card_rows(&[], &files);

    assert_eq!(cards.len(), 1);
    assert!(!cards[0].enabled);
    assert_eq!(cards[0].id, 0);
    assert_eq!(cards[0].subtitle.as_str(), "");
}

#[test]
fn folders_are_skipped_by_the_prewarm_rather_than_counted() {
    // The cap bounds kept *paths*, so a directory led by subfolders still warms a
    // full screenful of the files behind them.
    let files: Vec<BrowseFile> = (0..GRID_PREWARM_AHEAD + 5)
        .map(|i| file(i64::try_from(i).unwrap_or(0) + 1, &format!("t{i}"), None, true))
        .collect();

    let paths = first_screenful_paths(&files);

    assert_eq!(paths.len(), GRID_PREWARM_AHEAD);
    assert!(paths[0].ends_with("t0.jpg"));
}

#[test]
fn the_prewarm_cap_fits_inside_the_cache_it_fills() {
    // Prewarming more than the LRU can hold would evict the visible prefix it just
    // decoded — the same floor every other grid tier keeps.
    assert!(GRID_PREWARM_AHEAD <= DEFAULT_GRID_COVER_CAP.get());
}

const GLOBAL: &str = include_str!("../../../../melodia-ui/ui/globals/tracks.slint");
const VIEW: &str = include_str!("../../../../melodia-ui/ui/views/browse-view.slint");

/// Every `out property <int> mode-<name>: N;` on the `Browse` global, as its `N`.
fn declared_mode_values() -> Vec<i32> {
    GLOBAL
        .lines()
        .map(str::trim_start)
        .filter_map(|line| line.strip_prefix("out property <int> mode-"))
        .filter_map(|rest| rest.split_once(':'))
        .filter_map(|(_, value)| value.trim().trim_end_matches(';').parse().ok())
        .collect()
}

/// `Browse.view-mode-count` is the sole definition of how many presentations exist —
/// [`super::super::seed_from_settings`] clamps the persisted `browse_view_mode` against
/// it rather than carrying a Rust const. Two ways that drifts without the build
/// noticing, and the second is the quiet one: a count that undershoots makes a mode
/// unrestorable from `views.json`, and a `mode-*` value outside `0..count` can never
/// survive the clamp at all, so that presentation becomes unreachable while every
/// constant still reads correctly on its own line.
#[test]
fn the_mode_constants_are_exactly_the_range_the_clamp_admits() {
    let count: i32 = GLOBAL
        .split_once("out property <int> view-mode-count:")
        .and_then(|(_, rest)| rest.split_once(';'))
        .and_then(|(digits, _)| digits.trim().parse().ok())
        .unwrap_or_default();
    assert!(count > 0, "tracks.slint must declare `out property <int> view-mode-count: N;`");

    let mut values = declared_mode_values();
    values.sort_unstable();
    assert_eq!(
        values,
        (0..count).collect::<Vec<_>>(),
        "Browse's `mode-*` constants must be exactly 0..view-mode-count — \
         `ui::tab_bar::clamp_tab` can't produce an index outside that range"
    );

    // The other end of the pair: the view has to *read* the card constant, else the
    // mode is stored, clamped and persisted while both bodies stay on the list.
    assert!(
        VIEW.contains("Browse.view-mode == Browse.mode-card"),
        "browse-view.slint must derive `card-mode` from `Browse.mode-card`"
    );
}
