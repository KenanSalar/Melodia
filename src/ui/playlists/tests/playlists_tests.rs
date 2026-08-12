//! The Playlists grid's filter + sort pass — the last of the four entity
//! grids to get one. Same three cases its siblings pin, since all four run
//! the identical `row_match::Needle::contains` walk over a single name.

use super::*;
use crate::test_support::{MIN_SLINT_SOURCES, UI_DIR, stripped_sources};

/// Minimal `PlaylistStats` builder — only the fields the grid filter / sort
/// read matter. Regular playlist, so `smart_criteria` never parses.
fn playlist(id: i64, name: &str, track_count: i32, updated_at: &str) -> PlaylistStats {
    PlaylistStats {
        id,
        name: name.to_owned(),
        description: None,
        thumbnail_path: None,
        is_smart: false,
        smart_criteria: None,
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        updated_at: updated_at.to_owned(),
        custom_thumbnail: false,
        track_count,
        total_duration_ms: 0,
    }
}

fn names(data: &GridData, indices: &[usize]) -> Vec<String> {
    indices.iter().map(|&i| data.playlists[i].name.clone()).collect()
}

#[test]
fn compute_indices_with_empty_filter_keeps_all_playlists() {
    let data = GridData::new(vec![
        playlist(1, "Alpha", 1, "2026-01-02T00:00:00Z"),
        playlist(2, "Bravo", 1, "2026-01-01T00:00:00Z"),
    ]);
    let idx = compute_indices(&data, "name", "asc", "");
    assert_eq!(names(&data, &idx), ["Alpha", "Bravo"]);
}

#[test]
fn compute_indices_filter_matches_name_case_insensitively() {
    let data = GridData::new(vec![
        playlist(1, "Late Night Drive", 1, "2026-01-03T00:00:00Z"),
        playlist(2, "Night Shift", 1, "2026-01-02T00:00:00Z"),
        playlist(3, "Morning Coffee", 1, "2026-01-01T00:00:00Z"),
    ]);
    let by_name = compute_indices(&data, "name", "asc", "NIGHT");
    assert_eq!(names(&data, &by_name), ["Late Night Drive", "Night Shift"]);
}

#[test]
fn compute_indices_filter_ignores_accents_the_way_the_search_view_does() {
    // A playlist name is whatever the user typed, so it carries whatever
    // their keyboard makes easy — and the query later may not.
    let data = GridData::new(vec![
        playlist(1, "Café Sessions", 1, "2026-01-02T00:00:00Z"),
        playlist(2, "Morning Coffee", 1, "2026-01-01T00:00:00Z"),
    ]);
    let by_name = compute_indices(&data, "name", "asc", "cafe");
    assert_eq!(names(&data, &by_name), ["Café Sessions"]);
}

#[test]
fn compute_indices_default_sort_is_most_recently_updated_first() {
    // `playlist_stats` already returns `updated_at DESC`, so the default arm
    // re-derives that order rather than leaving whatever the filter produced.
    let data = GridData::new(vec![
        playlist(1, "Oldest", 1, "2026-01-01T00:00:00Z"),
        playlist(2, "Newest", 1, "2026-03-01T00:00:00Z"),
        playlist(3, "Middle", 1, "2026-02-01T00:00:00Z"),
    ]);
    let desc = compute_indices(&data, "updated", "desc", "");
    assert_eq!(names(&data, &desc), ["Newest", "Middle", "Oldest"]);
    let asc = compute_indices(&data, "updated", "asc", "");
    assert_eq!(names(&data, &asc), ["Oldest", "Middle", "Newest"]);
}

/// **The three multi-caller playlist dialogs are opened, never populated.**
///
/// Create, Rename and Delete each have more than one entry point — Create has three (the
/// Playlists tab's New pill, Ctrl+N, and a track row's "New Playlist…"), Rename and
/// Delete two each — and every one of them used to spell out the same eight-to-eleven
/// `Dialog.*` assignments. They are `Dialog.open-{create,rename,delete}-playlist()` now.
///
/// The bug that fold retired is exactly what this guards: Ctrl+N's copy had drifted to
/// `@tr("Create Playlist")` under a comment claiming it matched the other two — one
/// dialog, two headings, and two msgids translated separately in all six catalogues,
/// with nothing failing.
///
/// **The remaining fifteen `Dialog.kind` writes, across fourteen kinds, stay inline and
/// stay out of this.** Thirteen of those kinds have one caller each and earn nothing by
/// moving: the populate block is already stated once, where it is used. The fourteenth is
/// `smart-playlist-editor`, which is the interesting one — two sites, and deliberately
/// absent anyway, because they share a `kind` and nothing else: Edit Rules / Save over an
/// existing list, New Smart Playlist / Create. Two callers is the trigger for folding only
/// when the two are meant to be the same dialog.
#[test]
fn every_multi_caller_playlist_dialog_opens_through_its_own_function() {
    const OWNER: &str = "globals/dialog.slint";
    const FOLDED_KINDS: [&str; 3] = ["create-playlist", "rename-playlist", "delete-playlist"];

    let offenders: Vec<String> = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES)
        .into_iter()
        .filter(|(path, _)| !path.ends_with(OWNER))
        .flat_map(|(path, src)| {
            FOLDED_KINDS
                .iter()
                .filter(|kind| src.contains(&format!("Dialog.kind = \"{kind}\"")))
                .map(|kind| format!("{path}: Dialog.kind = \"{kind}\""))
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "these three dialogs are opened through `{OWNER}`'s own \
         `open-create-playlist` / `open-rename-playlist` / `open-delete-playlist`, so the \
         title, the confirm label and the `destructive` flag are stated once. A site that \
         re-spells the populate block compiles, opens the right dialog, and is free to \
         drift on any of them — which is how Ctrl+N came to raise a second heading with a \
         msgid of its own:\n{}",
        offenders.join("\n")
    );
}
