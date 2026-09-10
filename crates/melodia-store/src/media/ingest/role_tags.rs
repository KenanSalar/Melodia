//! Role credits on the tag boundary: the one place a [`CreditRole`] becomes a key a file stores,
//! and back.
//!
//! Its own module for [`super::rating_tags`]' reason — the reader and the writer both need the
//! mapping, and the per-format holes in it are the kind of fact that rots when it is spelled twice.
//!
//! **Five of the ten roles reach `ID3v2` for free.** Producer, arranger, engineer, mixer and
//! DJ-mixer live in the `TIPL` frame, and lofty's `Id3v2Tag → Tag` conversion consumes it into the
//! matching [`ItemKey`]s, so nothing here special-cases MP3 for them.
//!
//! **Three holes, none of them ours to close.** `Arranger` and `Performer` have no MP4 mapping and
//! `Performer` has no `ID3v2` one, so a credit in those roles is read and written on FLAC, Ogg and
//! APE only — [`super::tag_writer`]'s `UnsupportedFields` reports the write half rather than
//! dropping it silently. The `ID3v2` performer hole is the one worth knowing *why* about: `TMCL`
//! carries exactly that credit, lofty parses the frame, and then `split_tag` consumes only `TIPL`
//! and leaves `TMCL` in a `pub(crate)` companion tag with no public accessor. Reaching it needs a
//! second `Id3v2Tag` parse of the file, and
//! [`crate::media::ingest::metadata::read_tags`] is the tree's only lofty open.
//!
//! **`Writer` is read and never written.** lofty maps both `Writer` and `Lyricist` to `ID3v2`'s
//! `TEXT` frame, so on an MP3 the two are one field: writing a writer credit would consume the
//! lyricist and a round trip would move names between roles. Reading it onto
//! [`CreditRole::Composer`] is what other players do with the tag.

use lofty::prelude::ItemKey;
use lofty::tag::Tag;
use melodia_core::entities::credits::{CreditRole, ROLES, RoleCredit, RoleCredits};

use super::metadata::trimmed_values;

/// The key each role is read from and written to. One entry per [`ROLES`] member; the array is
/// exhaustive by construction and [`key_for`] would not compile without every arm.
fn key_for(role: CreditRole) -> ItemKey {
    match role {
        CreditRole::Composer => ItemKey::Composer,
        CreditRole::Lyricist => ItemKey::Lyricist,
        CreditRole::Conductor => ItemKey::Conductor,
        CreditRole::Remixer => ItemKey::Remixer,
        CreditRole::Arranger => ItemKey::Arranger,
        CreditRole::Producer => ItemKey::Producer,
        CreditRole::Engineer => ItemKey::Engineer,
        CreditRole::Mixer => ItemKey::MixEngineer,
        CreditRole::DjMixer => ItemKey::MixDj,
        CreditRole::Performer => ItemKey::Performer,
    }
}

/// Keys folded onto a role on the way in and never written back. See the module doc for why
/// `Writer` is one.
const FOLDED_KEYS: [(CreditRole, ItemKey); 1] = [(CreditRole::Composer, ItemKey::Writer)];

/// Every role credit `tag` carries, in [`ROLES`] order.
///
/// A value repeated under one key is one credit each — which is what a tagger writes for multiple
/// composers, and what [`trimmed_values`] already unpacks per format. Nothing is split on
/// punctuation: a credit is a name, and the exceptions list that splitting would need is the same
/// one `metadata::read_credit` declines to start.
pub fn read_roles(tag: &Tag) -> RoleCredits {
    let mut credits = Vec::new();

    for role in ROLES {
        let folded = FOLDED_KEYS.iter().filter(|(r, _)| *r == role).map(|(_, key)| *key);
        for key in std::iter::once(key_for(role)).chain(folded) {
            for value in trimmed_values(tag, key) {
                let (name, detail) = split_detail(&value, role);
                if name.is_empty() || already_credited(&credits, role, &name) {
                    continue;
                }
                credits.push(RoleCredit { role, name, detail });
            }
        }
    }

    RoleCredits::new(credits)
}

/// Write `credits` into `tag`, answering with the roles the format has no key for.
///
/// Every role is cleared first, including the ones `credits` says nothing about: a role the user
/// emptied has to leave the file, and a partial write would leave the old names behind it.
pub fn write_roles(tag: &mut Tag, credits: &RoleCredits) -> Vec<CreditRole> {
    let mut unsupported = Vec::new();

    for role in ROLES {
        let key = key_for(role);
        tag.remove_key(key);

        let values: Vec<String> = credits.for_role(role).map(join_detail).collect();
        if values.is_empty() {
            continue;
        }
        if !push_values(tag, key, values) {
            unsupported.push(role);
        }
    }

    unsupported
}

/// Remove every role credit the tag carries.
pub fn clear(tag: &mut Tag) {
    for role in ROLES {
        tag.remove_key(key_for(role));
    }
}

/// Push one item per value under `key`, replacing whatever was there.
///
/// `insert_text` would keep only the last, so the first value goes through it — which is also what
/// reports an unmapped key — and the rest are pushed beside it. lofty renders the repetition per
/// format: a NUL-separated `ID3v2` frame, a repeated Vorbis field, a repeated MP4 atom value.
fn push_values(tag: &mut Tag, key: ItemKey, values: Vec<String>) -> bool {
    let mut values = values.into_iter();
    let Some(first) = values.next() else {
        return true;
    };
    if !tag.insert_text(key, first) {
        return false;
    }
    for value in values {
        tag.push(lofty::tag::TagItem::new(key, lofty::tag::ItemValue::Text(value)));
    }
    true
}

/// `"Alice Monroe (cello)"` for a performer, the whole string for anyone else.
///
/// Only the *last* parenthesised group is taken, and only when it closes the value: a name can
/// contain parentheses and the instrument is what the convention puts at the end.
fn split_detail(value: &str, role: CreditRole) -> (String, String) {
    if !role.carries_detail() || !value.ends_with(')') {
        return (value.to_owned(), String::new());
    }

    let Some(open) = value.rfind('(') else {
        return (value.to_owned(), String::new());
    };
    let name = value[..open].trim();
    let detail = value[open + 1..value.len() - 1].trim();
    if name.is_empty() || detail.is_empty() {
        return (value.to_owned(), String::new());
    }
    (name.to_owned(), detail.to_owned())
}

/// The inverse of [`split_detail`].
fn join_detail(credit: &RoleCredit) -> String {
    if credit.detail.is_empty() {
        return credit.name.clone();
    }
    format!("{} ({})", credit.name, credit.detail)
}

/// Whether this name is already credited in this role.
///
/// `Writer` folding is what makes the check necessary: a file naming the same person in both
/// `COMPOSER` and `WRITER` would otherwise get two composer rows, and `track_credits`' primary key
/// is `(track_id, role, position)` rather than the name, so nothing downstream would reject it.
fn already_credited(credits: &[RoleCredit], role: CreditRole, name: &str) -> bool {
    credits.iter().any(|credit| credit.role == role && credit.name.eq_ignore_ascii_case(name))
}
