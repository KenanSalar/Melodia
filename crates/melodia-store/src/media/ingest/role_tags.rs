//! Role credits on the tag boundary: the one place a [`CreditRole`] becomes a key a file stores,
//! and back, so the per-format holes are spelled once.
//!
//! Producer, arranger, engineer, mixer and DJ-mixer reach `ID3v2` through `TIPL`, which lofty
//! converts into the matching [`ItemKey`]s. **Three holes upstream won't close**: `Arranger` has no
//! MP4 mapping, and `Performer` neither an MP4 nor an `ID3v2` one, which `WriteOutcome` reports.
//! lofty parses `TMCL` but leaves it in a companion tag with no public accessor, having removed
//! `ItemKey::MusicianCredits` on purpose, so reaching it would take a second `Id3v2Tag` parse of
//! every MP3 on the scan path.
//!
//! **`Writer` is read and never written**: lofty maps it and `Lyricist` both to `ID3v2`'s `TEXT`,
//! so writing one would consume the other. It reads as [`CreditRole::Composer`], as other players
//! take it.

use lofty::prelude::ItemKey;
use lofty::tag::Tag;
use melodia_core::entities::credits::{CreditRole, ROLES, RoleCredit, RoleCredits};
use melodia_core::entities::tags::RoleCreditEdit;

use super::metadata::trimmed_values;

/// The key each role is read from and written to.
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

/// Keys folded onto a role on the way in and never written back.
const FOLDED_KEYS: [(CreditRole, ItemKey); 1] = [(CreditRole::Composer, ItemKey::Writer)];

/// Every role credit `tag` carries, in [`ROLES`] order. A repeated value is one credit each, and
/// nothing is split on punctuation, for `metadata::read_credit`'s reason.
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

/// Writes `edit` into `tag`, answering with the roles the format has no key for. Every role **in
/// scope** is cleared first, so an emptied role leaves the file; one outside it is left alone, for
/// the reason [`RoleCreditEdit`] gives.
pub fn write_roles(tag: &mut Tag, edit: &RoleCreditEdit) -> Vec<CreditRole> {
    let mut unsupported = Vec::new();
    let credits = edit.credits();

    for role in edit.scope() {
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

/// Removes every role credit, a `FieldEdit::Clear` carrying no scope to narrow it.
pub fn clear(tag: &mut Tag) {
    for role in ROLES {
        tag.remove_key(key_for(role));
    }
}

/// Replaces `key`'s values: the first through `insert_text`, which keeps one and reports an
/// unmapped key, the rest pushed beside it.
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

/// Whether this name is already credited in this role. `Writer` folding needs it: a file naming one
/// person under `COMPOSER` and `WRITER` would get two rows, `track_credits`' key not being the name.
fn already_credited(credits: &[RoleCredit], role: CreditRole, name: &str) -> bool {
    credits.iter().any(|credit| credit.role == role && credit.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
#[path = "tests/role_tags_tests.rs"]
mod tests;
