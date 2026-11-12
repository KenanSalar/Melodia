//! What makes two tracks one song, and how sure the match is. Pure, over what the store reads.
//!
//! Copies match on artist and title through `fold_words`, then on length: tags alone would join
//! different songs a tagger named alike. Length is measured from the group's shortest copy, so a
//! run of near misses can't chain into one group. Two rows of one file never match: they are a cue
//! sheet's tracks of one image, however alike they are named.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};

use melodia_core::entities::duplicates::{
    DismissalSet, DuplicateCopy, DuplicateGroup, MatchKind, ReviewReason,
};
use melodia_core::utils::fold::fold_words;
use melodia_store::database::queries::track::DuplicateKeyRow;

/// How far a copy's length may sit from its group's shortest: wide enough for one song's encodes,
/// narrow enough that a radio edit falls outside.
const DURATION_TOLERANCE_MS: i64 = 1_500;

/// All a find holds per track before it has candidates, fixed-size so a library-sized find sorts
/// keys rather than strings. `DefaultHasher` may change between Rust releases, which a key dropped
/// when the find returns doesn't mind.
pub(super) struct SongKey {
    hash: u64,
    id: i64,
}

impl SongKey {
    pub(super) fn of(row: &DuplicateKeyRow) -> Self {
        let mut hasher = DefaultHasher::new();
        SongWords::of(&row.title, row.artist.as_deref()).hash(&mut hasher);
        Self { hash: hasher.finish(), id: row.id }
    }
}

/// A track's artist and title as matching compares them, artist first for the list's order.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
struct SongWords {
    artist: String,
    title: String,
}

impl SongWords {
    fn of(title: &str, artist: Option<&str>) -> Self {
        Self { artist: fold_words(artist.unwrap_or_default()), title: fold_words(title) }
    }
}

/// Every track sharing its key's hash with another, which is all pass 2 reads.
pub(super) fn candidate_ids(mut keys: Vec<SongKey>) -> Vec<i64> {
    keys.sort_unstable_by_key(|key| key.hash);
    keys.chunk_by(|a, b| a.hash == b.hash)
        .filter(|run| run.len() > 1)
        .flatten()
        .map(|key| key.id)
        .collect()
}

/// The candidates as groups in list order. Re-keyed on their words, so a hash collision can't join
/// two songs.
pub(super) fn group(copies: Vec<DuplicateCopy>) -> Vec<DuplicateGroup> {
    let mut keyed: Vec<(SongWords, DuplicateCopy)> = copies
        .into_iter()
        .map(|copy| (SongWords::of(&copy.title, copy.artist.as_deref()), copy))
        .collect();
    keyed.sort_unstable_by(|(a_words, a), (b_words, b)| {
        a_words.cmp(b_words).then(a.duration_ms.cmp(&b.duration_ms))
    });

    let mut groups = Vec::new();
    let mut keyed = keyed.into_iter().peekable();
    while let Some((words, shortest)) = keyed.next() {
        let reach = shortest.duration_ms + DURATION_TOLERANCE_MS;
        let mut copies = vec![shortest];
        while let Some((_, copy)) =
            keyed.next_if(|(next_words, next)| *next_words == words && next.duration_ms <= reach)
        {
            copies.push(copy);
        }
        let copies = one_per_file(copies);
        if copies.len() > 1 {
            let group = build_group(copies, &words);
            groups.push((words, group));
        }
    }

    groups.sort_by(|(a_words, a), (b_words, b)| {
        a.kind.cmp(&b.kind).then_with(|| a_words.cmp(b_words))
    });
    groups.into_iter().map(|(_, group)| group).collect()
}

/// `copies` with one row per file, the keeper's.
fn one_per_file(mut copies: Vec<DuplicateCopy>) -> Vec<DuplicateCopy> {
    copies.sort_by(keeper_first);
    let mut files = HashSet::with_capacity(copies.len());
    copies.retain(|copy| files.insert(copy.file_path.clone()));
    copies
}

fn build_group(mut copies: Vec<DuplicateCopy>, words: &SongWords) -> DuplicateGroup {
    copies.sort_by(keeper_first);
    DuplicateGroup { kind: kind_of(&copies, words), dismissed: shared_dismissal(&copies), copies }
}

/// The better encode first, then the copy in the library longest, then the lowest id so the order
/// is total.
fn keeper_first(a: &DuplicateCopy, b: &DuplicateCopy) -> Ordering {
    b.format
        .quality()
        .cmp(&a.format.quality())
        .then_with(|| a.date_added.cmp(&b.date_added))
        .then(a.id.cmp(&b.id))
}

fn kind_of(copies: &[DuplicateCopy], words: &SongWords) -> MatchKind {
    if is_identical(copies) {
        return MatchKind::Identical;
    }
    review_reason(copies, words).map_or(MatchKind::SameRelease, MatchKind::Review)
}

/// One hash and one span on every copy, a cue sheet's tracks sharing their image's hash. A row the
/// hash backfill hasn't reached yet matches nothing here.
fn is_identical(copies: &[DuplicateCopy]) -> bool {
    let Some((first, rest)) = copies.split_first() else {
        return false;
    };
    let plays_the_same =
        |copy: &DuplicateCopy| copy.file_hash == first.file_hash && copy.span == first.span;
    first.file_hash.is_some() && rest.iter().all(plays_the_same)
}

/// The first doubt that holds, in [`ReviewReason`]'s precedence.
fn review_reason(copies: &[DuplicateCopy], words: &SongWords) -> Option<ReviewReason> {
    if words.artist.is_empty() {
        return Some(ReviewReason::NoArtist);
    }
    if differ(copies, |copy| copy.musicbrainz_track_id.as_deref()) {
        return Some(ReviewReason::OtherRecording);
    }
    let other_release = !same_album(copies)
        || differ(copies, |copy| copy.year)
        || differ(copies, |copy| copy.musicbrainz_release_id.as_deref());
    if other_release {
        return Some(ReviewReason::OtherRelease);
    }
    let other_position =
        differ(copies, |copy| copy.disc_number) || differ(copies, |copy| copy.track_number);
    other_position.then_some(ReviewReason::OtherPosition)
}

/// Whether two copies carry different values. A copy without one agrees with any, a tag being
/// missing far more often than wrong.
fn differ<'a, T: PartialEq>(
    copies: &'a [DuplicateCopy],
    field: impl Fn(&'a DuplicateCopy) -> Option<T>,
) -> bool {
    let mut values = copies.iter().filter_map(field);
    let Some(first) = values.next() else {
        return false;
    };
    values.any(|value| value != first)
}

/// Unlike [`differ`], a missing album counts: an album copy beside a loose single is another
/// release.
fn same_album(copies: &[DuplicateCopy]) -> bool {
    let mut albums =
        copies.iter().map(|copy| fold_words(copy.album.as_deref().unwrap_or_default()));
    let Some(first) = albums.next() else {
        return true;
    };
    albums.all(|album| album == first)
}

/// The set every copy carries, if one does.
fn shared_dismissal(copies: &[DuplicateCopy]) -> Option<DismissalSet> {
    let (first, rest) = copies.split_first()?;
    let set = first.dismissal?;
    rest.iter().all(|copy| copy.dismissal == Some(set)).then_some(set)
}

#[cfg(test)]
#[path = "tests/matcher_tests.rs"]
mod tests;
