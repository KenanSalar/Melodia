//! The finder's groups as an open dialog holds them, and the rules a removal pick follows. Pure,
//! with no Slint and no I/O.
//!
//! No copy's text is kept: a folder is an index interned while the set is built and a file hash a
//! class within its group, so an open finder costs a few dozen bytes a copy. The rows re-read what
//! they show by id.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use crate::ui::util::clamp_i64_to_i32;
use melodia_core::entities::duplicates::{DismissalSet, DuplicateGroup, GroupRemoval, MatchKind};

/// What a header row states about its group.
pub(super) struct HeaderFacts {
    pub(super) kind: MatchKind,
    pub(super) copy_count: usize,
    pub(super) dismissed: bool,
    /// The best copy, whose title and artist the header shows.
    pub(super) lead: i64,
}

struct Group {
    kind: MatchKind,
    dismissed: Option<DismissalSet>,
    /// Its run of [`DuplicateSet::copies`], the best first.
    copies: Range<usize>,
}

struct CopyEntry {
    id: i64,
    group: usize,
    size: i64,
    folder: u32,
    /// Copies of one group sharing a class are one byte-identical file.
    hash_class: Option<u32>,
    picked: bool,
}

#[derive(Default)]
pub(super) struct DuplicateSet {
    groups: Vec<Group>,
    copies: Vec<CopyEntry>,
    /// `(copy id, index into copies)`, sorted, so a click finds its copy.
    by_id: Vec<(i64, usize)>,
}

/// A group's header id: negative, so it never meets a track's.
pub(super) fn header_id(group: usize) -> i64 {
    i64::try_from(group).map_or(i64::MIN, |group| -1 - group)
}

fn group_index(header_id: i64) -> Option<usize> {
    usize::try_from(-1 - header_id).ok()
}

impl DuplicateSet {
    /// The find's groups, in its list order, with nothing picked.
    pub(super) fn new(groups: &[DuplicateGroup]) -> Self {
        let mut set = Self::default();
        let mut folders: HashMap<&Path, u32> = HashMap::new();
        for (index, group) in groups.iter().enumerate() {
            let start = set.copies.len();
            let mut hashes: HashMap<&str, u32> = HashMap::new();
            for copy in &group.copies {
                let folder = Path::new(&copy.file_path).parent().unwrap_or(Path::new(""));
                let next_folder = u32::try_from(folders.len()).unwrap_or(u32::MAX);
                let next_class = u32::try_from(hashes.len()).unwrap_or(u32::MAX);
                set.copies.push(CopyEntry {
                    id: copy.id,
                    group: index,
                    size: copy.file_size.unwrap_or_default(),
                    folder: *folders.entry(folder).or_insert(next_folder),
                    hash_class: copy
                        .file_hash
                        .as_deref()
                        .map(|hash| *hashes.entry(hash).or_insert(next_class)),
                    picked: false,
                });
            }
            set.groups.push(Group {
                kind: group.kind,
                dismissed: group.dismissed,
                copies: start..set.copies.len(),
            });
        }
        set.by_id = set.copies.iter().enumerate().map(|(at, copy)| (copy.id, at)).collect();
        set.by_id.sort_unstable();
        set
    }

    /// The suggested picks in every group: all but the best copy where the match is sure, only
    /// the byte-identical spares where it isn't, nothing in a dismissed group.
    pub(super) fn suggest_all(&mut self) {
        for group in 0..self.groups.len() {
            self.suggest(group);
        }
    }

    pub(super) fn clear_picks(&mut self) {
        for copy in &mut self.copies {
            copy.picked = false;
        }
    }

    /// Picks or unpicks one copy. Picking a group's last unpicked copy unpicks the one after it
    /// instead, so every group keeps a copy.
    pub(super) fn toggle(&mut self, copy_id: i64) {
        let Some(at) = self.pickable(copy_id) else {
            return;
        };
        let picking = !self.copies[at].picked;
        self.copies[at].picked = picking;
        let run = self.groups[self.copies[at].group].copies.clone();
        if picking && self.copies[run.clone()].iter().all(|copy| copy.picked) {
            let next = run.start + (at - run.start + 1) % run.len();
            self.copies[next].picked = false;
        }
    }

    /// Keep This Copy: every other copy of its group picked.
    pub(super) fn keep(&mut self, copy_id: i64) {
        let Some(at) = self.pickable(copy_id) else {
            return;
        };
        self.keep_only(self.copies[at].group, at);
    }

    /// Keep Copies in This Folder: in every group with a copy in that folder, the best copy there
    /// is kept and the rest picked. The folder is the file's own, never the ones below it.
    pub(super) fn keep_folder(&mut self, copy_id: i64) {
        let Some(at) = self.pickable(copy_id) else {
            return;
        };
        let folder = self.copies[at].folder;
        for group in 0..self.groups.len() {
            if self.groups[group].dismissed.is_some() {
                continue;
            }
            let mut run = self.groups[group].copies.clone();
            if let Some(keeper) = run.find(|&index| self.copies[index].folder == folder) {
                self.keep_only(group, keeper);
            }
        }
    }

    /// Not Duplicates landed: the group takes its set and gives up its picks.
    pub(super) fn mark_dismissed(&mut self, header_id: i64, dismissal: DismissalSet) {
        let Some(group) = self.group(header_id) else {
            return;
        };
        self.groups[group].dismissed = Some(dismissal);
        self.suggest(group);
    }

    /// Count as Duplicates landed: the group is suggested again like any other.
    pub(super) fn mark_restored(&mut self, header_id: i64) {
        let Some(group) = self.group(header_id) else {
            return;
        };
        self.groups[group].dismissed = None;
        self.suggest(group);
    }

    /// The list's keyset: each shown group's header, then its copies.
    pub(super) fn listed_ids(&self, show_dismissed: bool) -> Arc<[i64]> {
        let mut ids = Vec::with_capacity(self.groups.len() + self.copies.len());
        for (index, group) in self.groups.iter().enumerate() {
            if group.dismissed.is_some() && !show_dismissed {
                continue;
            }
            ids.push(header_id(index));
            ids.extend(self.copies[group.copies.clone()].iter().map(|copy| copy.id));
        }
        Arc::from(ids)
    }

    pub(super) fn listed_count(&self, show_dismissed: bool) -> usize {
        if show_dismissed { self.groups.len() } else { self.groups.len() - self.dismissed_count() }
    }

    pub(super) fn dismissed_count(&self) -> usize {
        self.groups.iter().filter(|group| group.dismissed.is_some()).count()
    }

    /// The picks as the list's selection names them.
    pub(super) fn picked_ids(&self) -> HashSet<i32> {
        self.picked().map(|copy| clamp_i64_to_i32(copy.id)).collect()
    }

    pub(super) fn picked_count(&self) -> usize {
        self.picked().count()
    }

    pub(super) fn picked_bytes(&self) -> i64 {
        self.picked().map(|copy| copy.size).sum()
    }

    pub(super) fn header(&self, header_id: i64) -> Option<HeaderFacts> {
        let group = &self.groups[self.group(header_id)?];
        Some(HeaderFacts {
            kind: group.kind,
            copy_count: group.copies.len(),
            dismissed: group.dismissed.is_some(),
            lead: self.copies[group.copies.start].id,
        })
    }

    /// What Not Duplicates writes a set for.
    pub(super) fn copy_ids(&self, header_id: i64) -> Option<Vec<i64>> {
        let group = &self.groups[self.group(header_id)?];
        Some(self.copies[group.copies.clone()].iter().map(|copy| copy.id).collect())
    }

    /// The header's and its copies' ids, every row a dismissal repaints.
    pub(super) fn group_row_ids(&self, header_id: i64) -> Vec<i64> {
        let mut ids = vec![header_id];
        ids.extend(self.copy_ids(header_id).unwrap_or_default());
        ids
    }

    pub(super) fn dismissal(&self, header_id: i64) -> Option<DismissalSet> {
        self.groups[self.group(header_id)?].dismissed
    }

    pub(super) fn copy_dismissed(&self, copy_id: i64) -> bool {
        self.position(copy_id)
            .is_some_and(|at| self.groups[self.copies[at].group].dismissed.is_some())
    }

    /// The picks as removals, one per keeper. A spare's history goes to the kept copy sharing its
    /// file where there is one, else to its group's best kept copy, so an identical extra in a
    /// Review group merges into its twin rather than into another release.
    pub(super) fn removals(&self) -> Vec<GroupRemoval> {
        let mut removals: Vec<GroupRemoval> = Vec::new();
        for group in &self.groups {
            let copies = &self.copies[group.copies.clone()];
            let kept: Vec<&CopyEntry> = copies.iter().filter(|copy| !copy.picked).collect();
            let Some(&best) = kept.first() else {
                continue;
            };
            let group_start = removals.len();
            for spare in copies.iter().filter(|copy| copy.picked) {
                let twin = kept
                    .iter()
                    .find(|kept| kept.hash_class.is_some() && kept.hash_class == spare.hash_class);
                let keeper = twin.map_or(best.id, |twin| twin.id);
                match removals[group_start..].iter_mut().find(|removal| removal.keeper == keeper) {
                    Some(removal) => removal.spares.push(spare.id),
                    None => removals.push(GroupRemoval { keeper, spares: vec![spare.id] }),
                }
            }
        }
        removals
    }

    fn suggest(&mut self, group: usize) {
        let Group { kind, dismissed, copies } = &self.groups[group];
        let (kind, dismissed) = (*kind, dismissed.is_some());
        let copies = &mut self.copies[copies.clone()];
        if dismissed {
            for copy in copies {
                copy.picked = false;
            }
            return;
        }
        match kind {
            MatchKind::Identical | MatchKind::SameRelease => {
                for (at, copy) in copies.iter_mut().enumerate() {
                    copy.picked = at > 0;
                }
            }
            MatchKind::Review(_) => pick_identical_spares(copies),
        }
    }

    fn keep_only(&mut self, group: usize, keeper: usize) {
        for index in self.groups[group].copies.clone() {
            self.copies[index].picked = index != keeper;
        }
    }

    fn picked(&self) -> impl Iterator<Item = &CopyEntry> {
        self.copies.iter().filter(|copy| copy.picked)
    }

    fn group(&self, header_id: i64) -> Option<usize> {
        group_index(header_id).filter(|&group| group < self.groups.len())
    }

    fn position(&self, copy_id: i64) -> Option<usize> {
        let found = self.by_id.binary_search_by_key(&copy_id, |&(id, _)| id).ok()?;
        Some(self.by_id[found].1)
    }

    /// A copy a pick may touch: one in a group the user hasn't dismissed.
    fn pickable(&self, copy_id: i64) -> Option<usize> {
        self.position(copy_id).filter(|&at| self.groups[self.copies[at].group].dismissed.is_none())
    }
}

/// In a group whose copies may be two songs, only a byte-identical file is spare: each hash keeps
/// its first copy, the best by keeper order.
fn pick_identical_spares(copies: &mut [CopyEntry]) {
    let mut seen = HashSet::new();
    for copy in copies {
        copy.picked = copy.hash_class.is_some_and(|class| !seen.insert(class));
    }
}

#[cfg(test)]
#[path = "tests/picks_tests.rs"]
mod tests;
