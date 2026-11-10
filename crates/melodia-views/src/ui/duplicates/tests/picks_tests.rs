//! The removal picks over the finder's groups, which Remove hands to the store as they stand.
//!
//! A dismissed group's rules go untested here: a `DismissalSet` comes only from the store, which
//! this crate can't name.

use std::path::PathBuf;

use super::*;
use melodia_core::entities::audio_quality::AudioFormat;
use melodia_core::entities::duplicates::{DuplicateCopy, ReviewReason};

const REVIEW: MatchKind = MatchKind::Review(ReviewReason::OtherRelease);

/// Copy `id` in the folder `music/<folder…>`, the facts a pick reads being its folder and hash.
fn copy_in(id: i64, folder: &[&str], file_hash: Option<&str>) -> DuplicateCopy {
    let dir = folder.iter().fold(PathBuf::from("music"), |dir, name| dir.join(name));
    DuplicateCopy {
        id,
        file_path: dir.join(format!("{id}.mp3")).to_string_lossy().into_owned(),
        file_hash: file_hash.map(str::to_owned),
        file_size: Some(1_000),
        span: None,
        title: "Song".to_owned(),
        artist: Some("Artist".to_owned()),
        album: None,
        disc_number: None,
        track_number: None,
        year: None,
        musicbrainz_track_id: None,
        musicbrainz_release_id: None,
        duration_ms: 200_000,
        date_added: "2024-01-01T00:00:00+00:00".to_owned(),
        format: AudioFormat { codec: None, bitrate: None, sample_rate: None, bit_depth: None },
        dismissal: None,
        play_count: 0,
        rating: 0,
        is_favorite: false,
        playlist_count: 0,
    }
}

fn copy(id: i64) -> DuplicateCopy {
    copy_in(id, &["a"], None)
}

fn group(kind: MatchKind, copies: Vec<DuplicateCopy>) -> DuplicateGroup {
    DuplicateGroup { kind, copies, dismissed: None }
}

fn suggested(groups: &[DuplicateGroup]) -> DuplicateSet {
    let mut set = DuplicateSet::new(groups);
    set.suggest_all();
    set
}

/// The groups with `ids` picked by hand, one click each, none of them the group's last.
fn clicked(groups: &[DuplicateGroup], ids: &[i64]) -> DuplicateSet {
    let mut set = DuplicateSet::new(groups);
    for &id in ids {
        set.toggle(id);
    }
    set
}

fn picked(set: &DuplicateSet) -> Vec<i32> {
    let mut ids: Vec<i32> = set.picked_ids().into_iter().collect();
    ids.sort_unstable();
    ids
}

#[test]
fn suggested_picks_take_all_but_the_best_copy_of_a_sure_match() {
    let set = suggested(&[
        group(MatchKind::Identical, vec![copy(1), copy(2)]),
        group(MatchKind::SameRelease, vec![copy(3), copy(4), copy(5)]),
    ]);

    assert_eq!(picked(&set), [2, 4, 5]);
}

#[test]
fn suggested_picks_in_a_review_group_take_only_byte_identical_extras() {
    let set = suggested(&[group(
        REVIEW,
        vec![
            copy_in(1, &["a"], Some("h")),
            copy_in(2, &["b"], None),
            copy_in(3, &["c"], Some("h")),
            copy_in(4, &["d"], Some("g")),
        ],
    )]);

    assert_eq!(picked(&set), [3]);
}

#[test]
fn picking_a_groups_last_unpicked_copy_keeps_the_next_one_instead() {
    let mut set =
        clicked(&[group(MatchKind::SameRelease, vec![copy(1), copy(2), copy(3)])], &[2, 3]);

    set.toggle(1);

    assert_eq!(picked(&set), [1, 3]);
}

#[test]
fn picking_the_last_copy_as_the_last_unpicked_keeps_the_first() {
    let mut set =
        clicked(&[group(MatchKind::SameRelease, vec![copy(1), copy(2), copy(3)])], &[1, 2]);

    set.toggle(3);

    assert_eq!(picked(&set), [2, 3]);
}

/// The user's own choice, so a group nothing is suggested in takes it too.
#[test]
fn keep_this_copy_picks_every_other_copy_of_its_group() {
    let mut set = DuplicateSet::new(&[group(REVIEW, vec![copy(1), copy(2), copy(3), copy(4)])]);

    set.keep(3);

    assert_eq!(picked(&set), [1, 2, 4]);
}

/// A library copied into a second folder makes one group per track, so one choice keeps the whole
/// folder.
#[test]
fn keep_copies_in_this_folder_keeps_the_best_copy_there_in_every_group_with_one() {
    let mut set = DuplicateSet::new(&[
        group(MatchKind::SameRelease, vec![copy_in(1, &["a"], None), copy_in(2, &["b"], None)]),
        group(
            MatchKind::SameRelease,
            vec![copy_in(3, &["a"], None), copy_in(4, &["b"], None), copy_in(5, &["b"], None)],
        ),
    ]);

    set.keep_folder(2);

    assert_eq!(picked(&set), [1, 3, 5]);
}

#[test]
fn keep_copies_in_this_folder_leaves_groups_with_no_copy_in_that_very_folder_alone() {
    let mut set = DuplicateSet::new(&[
        group(MatchKind::SameRelease, vec![copy_in(1, &["a"], None), copy_in(2, &["b"], None)]),
        group(
            MatchKind::SameRelease,
            vec![copy_in(3, &["b", "Disc 2"], None), copy_in(4, &["c"], None)],
        ),
        group(MatchKind::SameRelease, vec![copy_in(5, &["d"], None), copy_in(6, &["e"], None)]),
    ]);

    set.keep_folder(2);

    assert_eq!(picked(&set), [1]);
}

#[test]
fn a_spare_merges_into_the_kept_copy_sharing_its_file() {
    let set = clicked(
        &[group(
            REVIEW,
            vec![
                copy_in(1, &["a"], None),
                copy_in(2, &["b"], Some("h")),
                copy_in(3, &["c"], Some("h")),
            ],
        )],
        &[3],
    );

    let removals = set.removals();

    assert_eq!(removals, [GroupRemoval { keeper: 2, spares: vec![3] }]);
}

#[test]
fn a_spare_with_no_twin_merges_into_its_groups_best_kept_copy() {
    let set = clicked(
        &[group(MatchKind::SameRelease, vec![copy(1), copy(2), copy(3), copy(4)])],
        &[1, 3],
    );

    let removals = set.removals();

    assert_eq!(removals, [GroupRemoval { keeper: 2, spares: vec![1, 3] }]);
}

#[test]
fn the_list_holds_each_groups_header_then_its_copies() {
    let set = DuplicateSet::new(&[
        group(MatchKind::SameRelease, vec![copy(1), copy(2)]),
        group(REVIEW, vec![copy(3), copy(4)]),
    ]);

    let listed = set.listed_ids(false);

    assert_eq!(*listed, [-1, 1, 2, -2, 3, 4]);
}
