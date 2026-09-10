//! Pure diff logic for the tag editor. No UI, no fixtures — the risky part of the commit path is
//! turning the form snapshot into a `TagEdit`.

use super::*;
use melodia_core::entities::credits::{CreditRole, RoleCredit};

use crate::ui::callbacks::tags::form::TextFields;

#[test]
fn diff_str_keep_clear_set() {
    assert_eq!(diff_str("Album", "Album"), FieldEdit::Keep);
    assert_eq!(diff_str("", "Album"), FieldEdit::Clear);
    assert_eq!(diff_str("   ", "Album"), FieldEdit::Clear);
    assert_eq!(diff_str("New", "Album"), FieldEdit::Set("New".to_owned()));
    // An untouched empty field (the multi-value baseline) stays Keep — it can't
    // be cleared across a whole selection, by design.
    assert_eq!(diff_str("", ""), FieldEdit::Keep);
}

#[test]
fn diff_parsed_u16_parses_and_guards() {
    assert_eq!(diff_parsed::<u16>("2020", "2020"), FieldEdit::Keep);
    assert_eq!(diff_parsed::<u16>("2021", "2020"), FieldEdit::Set(2021));
    assert_eq!(diff_parsed::<u16>("", "2020"), FieldEdit::Clear);
    // Non-numeric / out-of-range degrade to Keep — never write garbage.
    assert_eq!(diff_parsed::<u16>("nope", "2020"), FieldEdit::Keep);
    assert_eq!(diff_parsed::<u16>("99999", "2020"), FieldEdit::Keep);
}

#[test]
fn diff_parsed_u32_parses_and_guards() {
    assert_eq!(diff_parsed::<u32>("3", "1"), FieldEdit::Set(3));
    assert_eq!(diff_parsed::<u32>("", "1"), FieldEdit::Clear);
    assert_eq!(diff_parsed::<u32>("x", "1"), FieldEdit::Keep);
}

#[test]
fn diff_bpm_rejects_nan_inf_negative() {
    assert!(matches!(diff_bpm("128", "x"), FieldEdit::Set(b) if (b - 128.0).abs() < 1e-9));
    assert!(matches!(diff_bpm("128.5", "x"), FieldEdit::Set(b) if (b - 128.5).abs() < 1e-9));
    assert!(matches!(diff_bpm("", "128"), FieldEdit::Clear));
    assert!(matches!(diff_bpm("128", "128"), FieldEdit::Keep));
    // `parse::<f64>()` accepts these — the guard must reject them.
    assert!(matches!(diff_bpm("nan", "x"), FieldEdit::Keep));
    assert!(matches!(diff_bpm("inf", "x"), FieldEdit::Keep));
    assert!(matches!(diff_bpm("-5", "x"), FieldEdit::Keep));
    assert!(matches!(diff_bpm("junk", "x"), FieldEdit::Keep));
}

/// One ladder behind all three multi-value fields, so a `Clear` cannot start meaning `Keep` for
/// one of them alone.
#[test]
fn diff_multi_keep_clear_set_for_every_multi_value_field() {
    let rock = GenreList::from_name("Rock");
    assert_eq!(diff_multi(&rock, &rock), FieldEdit::Keep);
    assert_eq!(diff_multi(&GenreList::default(), &rock), FieldEdit::Clear);
    assert_eq!(diff_multi(&rock, &GenreList::default()), FieldEdit::Set(rock.clone()));

    let artist = ArtistCredit::from_name("Nadia Vance");
    assert_eq!(diff_multi(&artist, &artist), FieldEdit::Keep);
    assert_eq!(diff_multi(&ArtistCredit::default(), &artist), FieldEdit::Clear);

    let roles = RoleCredits::new(vec![RoleCredit {
        role: CreditRole::Composer,
        name: "Nadia Vance".to_owned(),
        detail: String::new(),
    }]);
    assert_eq!(diff_multi(&roles, &roles), FieldEdit::Keep);
    assert_eq!(diff_multi(&RoleCredits::default(), &roles), FieldEdit::Clear);
}

#[test]
fn build_edit_touches_only_changed_fields() {
    let orig = FormState {
        text: TextFields {
            title: "Title".to_owned(),
            album: "Album".to_owned(),
            year: "2020".to_owned(),
            track_number: "1".to_owned(),
            bpm: "120".to_owned(),
            ..TextFields::default()
        },
        compilation: false,
    };
    let lists = ListFields {
        credits: [ArtistCredit::from_name("Artist"), ArtistCredit::default()],
        genres: GenreList::from_name("Rock"),
        roles: RoleCredits::new(vec![RoleCredit {
            role: CreditRole::Composer,
            name: "Nadia Vance".to_owned(),
            detail: String::new(),
        }]),
    };

    let mut cur = orig.clone();
    cur.text.album = "New Album".to_owned();
    cur.text.title = String::new();

    let edit = build_edit(&cur, &orig, &lists, &lists, ArtworkEdit::Keep);
    assert_eq!(edit.album, FieldEdit::Set("New Album".to_owned()));
    assert_eq!(edit.title, FieldEdit::Clear);
    assert_eq!(edit.artist, FieldEdit::Keep);
    assert_eq!(edit.year, FieldEdit::Keep);
    // Every list field is untouched, and the role set staying `Keep` is what stops one edited
    // role clearing the nine beside it.
    assert_eq!(edit.credits, FieldEdit::Keep);
    assert_eq!(edit.genres, FieldEdit::Keep);
    assert!(!edit.is_noop());

    // An unchanged form diffs to an all-Keep no-op.
    let noop = build_edit(&orig, &orig, &lists, &lists, ArtworkEdit::Keep);
    assert!(noop.is_noop());
}
