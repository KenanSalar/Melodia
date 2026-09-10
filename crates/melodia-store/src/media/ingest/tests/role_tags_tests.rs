//! The role-credit mapping, over in-memory tags.
//!
//! Both directions live here because the interesting failures are asymmetries: a key read and
//! never written, a role a format has no key for, and a name the split takes an instrument off.

use lofty::prelude::ItemKey;
use lofty::tag::{ItemValue, Tag, TagItem, TagType};

use super::super::role_tags::{clear, read_roles, write_roles};
use melodia_core::entities::credits::{CreditRole, ROLES, RoleCredit, RoleCredits};
use melodia_core::entities::tags::RoleCreditEdit;

fn vorbis(values: &[(ItemKey, &str)]) -> Tag {
    let mut tag = Tag::new(TagType::VorbisComments);
    for (key, value) in values {
        tag.push(TagItem::new(*key, ItemValue::Text((*value).to_owned())));
    }
    tag
}

fn credit(role: CreditRole, name: &str, detail: &str) -> RoleCredit {
    RoleCredit {
        role,
        name: name.to_owned(),
        detail: detail.to_owned(),
    }
}

/// Every credit as (role, name, detail), which is the whole of what one holds.
fn spelled(credits: &RoleCredits) -> Vec<(&'static str, &str, &str)> {
    credits
        .all()
        .iter()
        .map(|c| (c.role.as_db_str(), c.name.as_str(), c.detail.as_str()))
        .collect()
}

/// **Only the last parenthesised group, and only when it closes the value.** A name can contain
/// parentheses of its own, and the instrument is what the convention puts at the end.
#[test]
fn a_performers_instrument_is_taken_from_the_end_of_the_value() {
    let tag = vorbis(&[(ItemKey::Performer, "Alice Monroe (cello)")]);

    assert_eq!(spelled(&read_roles(&tag)), vec![("performer", "Alice Monroe", "cello")]);
}

#[test]
fn a_name_that_carries_its_own_parentheses_keeps_them() {
    let tag = vorbis(&[(ItemKey::Performer, "Alice (The Quartet) Monroe (cello)")]);

    assert_eq!(
        spelled(&read_roles(&tag)),
        vec![("performer", "Alice (The Quartet) Monroe", "cello")]
    );
}

/// The three shapes that look like a detail and are not one, each left whole rather than split
/// into a credit naming nobody or crediting nothing.
#[test]
fn a_value_that_only_looks_like_it_carries_a_detail_is_left_whole() {
    let table = [
        // No name in front of the group.
        "(cello)",
        // Nothing inside it.
        "Alice ()",
        // The group does not close the value.
        "Alice (cello) live",
    ];

    for value in table {
        let tag = vorbis(&[(ItemKey::Performer, value)]);

        assert_eq!(spelled(&read_roles(&tag)), vec![("performer", value, "")]);
    }
}

/// Only a performer has an instrument, so the same value under any other role is a name.
#[test]
fn a_role_with_no_detail_reads_the_parentheses_as_part_of_the_name() {
    let tag = vorbis(&[(ItemKey::Composer, "Alice Monroe (cello)")]);

    assert_eq!(spelled(&read_roles(&tag)), vec![("composer", "Alice Monroe (cello)", "")]);
}

/// **`Writer` folds onto the composer and is never written back.** lofty maps both it and
/// `Lyricist` to `ID3v2`'s `TEXT` frame, so writing one would consume the other.
#[test]
fn a_writer_credit_reads_as_a_composer() {
    let tag = vorbis(&[(ItemKey::Writer, "Alice")]);

    assert_eq!(spelled(&read_roles(&tag)), vec![("composer", "Alice", "")]);
}

/// The fold is what makes the check necessary: `track_credits`' key is
/// (`track_id`, `role`, `position`) rather than the name, so nothing downstream would reject the
/// second row.
#[test]
fn a_person_named_under_both_composer_keys_is_credited_once() {
    let tag = vorbis(&[(ItemKey::Composer, "Alice"), (ItemKey::Writer, "alice")]);

    assert_eq!(spelled(&read_roles(&tag)), vec![("composer", "Alice", "")]);
}

/// A repeated key is what a tagger writes for two composers, and nothing is split on punctuation:
/// a credit is a name.
#[test]
fn a_key_repeated_is_one_credit_each() {
    let tag = vorbis(&[
        (ItemKey::Composer, "Alice"),
        (ItemKey::Composer, "Bob & Carol"),
        (ItemKey::Composer, "   "),
    ]);

    assert_eq!(
        spelled(&read_roles(&tag)),
        vec![("composer", "Alice", ""), ("composer", "Bob & Carol", "")]
    );
}

/// Credits come back in `ROLES` order whatever order the file wrote them in, so a list reads the
/// same way whichever tagger produced it.
#[test]
fn credits_come_back_in_the_order_the_list_reads_best() {
    let tag = vorbis(&[
        (ItemKey::MixEngineer, "Dave"),
        (ItemKey::Composer, "Alice"),
        (ItemKey::Producer, "Carol"),
    ]);

    let roles: Vec<&'static str> =
        read_roles(&tag).all().iter().map(|c| c.role.as_db_str()).collect();

    assert_eq!(roles, ["composer", "producer", "mixer"]);
}

#[test]
fn a_written_credit_reads_back_as_the_one_that_went_in() {
    let mut tag = Tag::new(TagType::VorbisComments);
    let credits = RoleCredits::new(vec![
        credit(CreditRole::Composer, "Alice", ""),
        credit(CreditRole::Performer, "Bob", "cello"),
        credit(CreditRole::Producer, "Carol", ""),
    ]);

    assert!(write_roles(&mut tag, &RoleCreditEdit::whole(credits.clone())).is_empty());

    assert_eq!(spelled(&read_roles(&tag)), spelled(&credits));
}

/// **Every role is cleared before anything is written**, including the ones the new set says
/// nothing about — a role the user emptied has to leave the file rather than keep the old names.
#[test]
fn a_role_the_new_set_no_longer_names_leaves_the_file() {
    let mut tag = vorbis(&[(ItemKey::Composer, "Alice"), (ItemKey::Producer, "Bob")]);

    write_roles(
        &mut tag,
        &RoleCreditEdit::whole(RoleCredits::new(vec![credit(CreditRole::Composer, "Alice", "")])),
    );

    assert_eq!(spelled(&read_roles(&tag)), vec![("composer", "Alice", "")]);
}

#[test]
fn clearing_takes_every_role_the_file_carried() {
    let mut tag = vorbis(&[
        (ItemKey::Composer, "Alice"),
        (ItemKey::Performer, "Bob (cello)"),
        (ItemKey::MixDj, "Carol"),
    ]);

    clear(&mut tag);

    assert!(read_roles(&tag).is_empty());
}

/// **The holes are the format's, and reporting them is what stops a credit disappearing without
/// the user being told.** Pinned as an exact set per format, so one closed by accident cannot stay
/// unreported and one opened by accident cannot go unnoticed. `tag_writer_tests` carries the same
/// question against real files, which is where the `ID3v2` set is settled: `TIPL` reaches the
/// generic tag on the way in and there is no key to put it back through.
#[test]
fn each_format_names_exactly_the_roles_it_has_no_key_for() {
    let every_role: Vec<RoleCredit> =
        ROLES.into_iter().map(|role| credit(role, "Alice", "")).collect();
    let credits = RoleCredits::new(every_role);

    let table = [
        (TagType::VorbisComments, &[][..]),
        (TagType::Mp4Ilst, &["arranger", "performer"][..]),
        (
            TagType::Id3v2,
            &[
                "arranger",
                "producer",
                "engineer",
                "mixer",
                "dj_mixer",
                "performer",
            ][..],
        ),
    ];

    for (tag_type, expected) in table {
        let mut tag = Tag::new(tag_type);
        let mut unsupported: Vec<&'static str> =
            write_roles(&mut tag, &RoleCreditEdit::whole(credits.clone()))
                .into_iter()
                .map(CreditRole::as_db_str)
                .collect();
        unsupported.sort_unstable();
        let mut expected: Vec<&'static str> = expected.to_vec();
        expected.sort_unstable();

        assert_eq!(unsupported, expected, "{tag_type:?} reported a different set");
    }
}
