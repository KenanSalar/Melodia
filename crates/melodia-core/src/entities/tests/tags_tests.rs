//! What a tag edit says it emptied, which is the half `upsert_album` structurally cannot write.

use super::super::tags::{ClearedReleaseTags, FieldEdit, TagEdit};

/// The six text flags plus the compilation one, in declaration order.
fn flags(cleared: ClearedReleaseTags) -> [bool; 7] {
    [
        cleared.label,
        cleared.catalog_number,
        cleared.barcode,
        cleared.media,
        cleared.release_type,
        cleared.release_country,
        cleared.compilation,
    ]
}

#[test]
fn an_edit_that_emptied_nothing_names_no_column() {
    let cleared = TagEdit::default().cleared_release_tags();

    assert!(cleared.is_empty());
    assert_eq!(flags(cleared), [false; 7]);
}

/// One flag per column, and the failure of a copy-paste slip here is a column emptied that the
/// user never touched.
#[test]
fn each_cleared_field_names_its_own_column() {
    let table: [(TagEdit, [bool; 7]); 6] = [
        (
            TagEdit {
                label: FieldEdit::Clear,
                ..TagEdit::default()
            },
            [true, false, false, false, false, false, false],
        ),
        (
            TagEdit {
                catalog_number: FieldEdit::Clear,
                ..TagEdit::default()
            },
            [false, true, false, false, false, false, false],
        ),
        (
            TagEdit {
                barcode: FieldEdit::Clear,
                ..TagEdit::default()
            },
            [false, false, true, false, false, false, false],
        ),
        (
            TagEdit {
                media: FieldEdit::Clear,
                ..TagEdit::default()
            },
            [false, false, false, true, false, false, false],
        ),
        (
            TagEdit {
                release_type: FieldEdit::Clear,
                ..TagEdit::default()
            },
            [false, false, false, false, true, false, false],
        ),
        (
            TagEdit {
                release_country: FieldEdit::Clear,
                ..TagEdit::default()
            },
            [false, false, false, false, false, true, false],
        ),
    ];

    for (edit, expected) in table {
        assert_eq!(flags(edit.cleared_release_tags()), expected);
    }
}

/// A switch has no third state, so an un-ticked box arrives as `Set(false)` — and the column it
/// writes to is an `OR` no re-ingest can bring back down, which makes this the one flag that has
/// to answer to two spellings.
#[test]
fn an_unticked_compilation_switch_clears_the_column_like_an_emptied_field() {
    for edit in [FieldEdit::Clear, FieldEdit::Set(false)] {
        let cleared = TagEdit {
            compilation: edit,
            ..TagEdit::default()
        }
        .cleared_release_tags();

        assert_eq!(flags(cleared), [false, false, false, false, false, false, true]);
    }

    let ticked = TagEdit {
        compilation: FieldEdit::Set(true),
        ..TagEdit::default()
    }
    .cleared_release_tags();
    assert!(ticked.is_empty());
}

/// The three tags a lyrics directory identifies a recording by. A true answer drops the stored
/// sheet, so a field that stops counting leaves a miss standing that was earned under tags the
/// file no longer carries.
#[test]
fn only_the_tags_a_recording_is_identified_by_rename_it() {
    let renaming = [
        TagEdit {
            title: FieldEdit::Set("New".into()),
            ..TagEdit::default()
        },
        TagEdit {
            artist: FieldEdit::Clear,
            ..TagEdit::default()
        },
        TagEdit {
            album: FieldEdit::Set("New".into()),
            ..TagEdit::default()
        },
    ];
    for edit in renaming {
        assert!(edit.renames_recording());
    }

    let leaving_it_alone = [
        TagEdit::default(),
        TagEdit {
            comment: FieldEdit::Set("New".into()),
            ..TagEdit::default()
        },
        TagEdit {
            rating: FieldEdit::Set(4),
            ..TagEdit::default()
        },
    ];
    for edit in leaving_it_alone {
        assert!(!edit.renames_recording());
    }
}
