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
            TagEdit { label: FieldEdit::Clear, ..TagEdit::default() },
            [true, false, false, false, false, false, false],
        ),
        (
            TagEdit { catalog_number: FieldEdit::Clear, ..TagEdit::default() },
            [false, true, false, false, false, false, false],
        ),
        (
            TagEdit { barcode: FieldEdit::Clear, ..TagEdit::default() },
            [false, false, true, false, false, false, false],
        ),
        (
            TagEdit { media: FieldEdit::Clear, ..TagEdit::default() },
            [false, false, false, true, false, false, false],
        ),
        (
            TagEdit { release_type: FieldEdit::Clear, ..TagEdit::default() },
            [false, false, false, false, true, false, false],
        ),
        (
            TagEdit { release_country: FieldEdit::Clear, ..TagEdit::default() },
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
        let cleared = TagEdit { compilation: edit, ..TagEdit::default() }.cleared_release_tags();

        assert_eq!(flags(cleared), [false, false, false, false, false, false, true]);
    }

    let ticked =
        TagEdit { compilation: FieldEdit::Set(true), ..TagEdit::default() }.cleared_release_tags();
    assert!(ticked.is_empty());
}

/// The three tags a lyrics directory identifies a recording by. A true answer drops the stored
/// sheet, so a field that stops counting leaves a miss standing that was earned under tags the
/// file no longer carries.
#[test]
fn only_the_tags_a_recording_is_identified_by_rename_it() {
    let renaming = [
        TagEdit { title: FieldEdit::Set("New".into()), ..TagEdit::default() },
        TagEdit { artist: FieldEdit::Clear, ..TagEdit::default() },
        TagEdit { album: FieldEdit::Set("New".into()), ..TagEdit::default() },
    ];
    for edit in renaming {
        assert!(edit.renames_recording());
    }

    let leaving_it_alone = [
        TagEdit::default(),
        TagEdit { comment: FieldEdit::Set("New".into()), ..TagEdit::default() },
        TagEdit { rating: FieldEdit::Set(4), ..TagEdit::default() },
    ];
    for edit in leaving_it_alone {
        assert!(!edit.renames_recording());
    }
}

/// `TagField::label_index` is a position in a list written in a different language in a different
/// crate, so the two can drift into naming each other's fields with nothing failing to compile.
///
/// Length is what catches the realistic drift, a variant gained or lost on one side. The credit
/// tail is checked against the *other* list that claims `ROLES` order, `tag-editor-body.slint`'s
/// `role-labels`: two lists written independently agreeing on ten strings is a real cross-check,
/// and it is the only pin `role-labels` has ever had.
///
/// What it cannot see is two *plain* labels swapped. `PLAIN_TAG_FIELDS` is the definition of that
/// order and there is no second spelling to compare it against, the labels being English prose no
/// `as_db_str` derives. Re-read the two lists side by side when adding to either.
#[test]
fn the_slint_label_lists_line_up_with_the_rust_order() {
    use super::super::credits::ROLES;
    use super::super::tags::PLAIN_TAG_FIELDS;

    const SETTINGS: &str = include_str!("../../../../melodia-ui/ui/settings.slint");
    const TAG_EDITOR: &str =
        include_str!("../../../../melodia-ui/ui/components/dialog/tag-editor-body.slint");

    /// The `@tr("…")` literals of `property <[string]> <name>: [ … ];`, in order. `None` where the
    /// list was renamed or removed, which fails the assertion rather than passing vacuously.
    fn labels(source: &str, decl: &str) -> Option<Vec<String>> {
        let (_, tail) = source.split_once(decl)?;
        let (body, _) = tail.split_once("];")?;
        Some(
            body.split("@tr(\"")
                .skip(1)
                .filter_map(|entry| entry.split_once('"').map(|(label, _)| label.to_owned()))
                .collect(),
        )
    }

    let field_labels =
        labels(SETTINGS, "out property <[string]> tag-field-labels: [").unwrap_or_default();
    let role_labels =
        labels(TAG_EDITOR, "private property <[string]> role-labels: [").unwrap_or_default();

    assert_eq!(
        field_labels.len(),
        PLAIN_TAG_FIELDS.len() + ROLES.len(),
        "every `TagField` owes one label, credits last"
    );
    assert_eq!(role_labels.len(), ROLES.len(), "the credits editor owes one label per role");
    assert_eq!(
        &field_labels[PLAIN_TAG_FIELDS.len()..],
        role_labels.as_slice(),
        "both lists claim `ROLES` order, so a reorder in either shows up here"
    );
}

/// A duplicate or a gap in `PLAIN_TAG_FIELDS` silently points two fields at one label.
///
/// A variant *missing* from the array is not checkable here, `TagField` having no runtime
/// enumeration; it degrades to `label_index` answering `None` and the toast naming no field, which
/// is why the caller falls back rather than indexing blind.
#[test]
fn every_plain_field_owns_exactly_one_label_slot() {
    use super::super::tags::PLAIN_TAG_FIELDS;

    let slots: Vec<Option<usize>> = PLAIN_TAG_FIELDS.iter().map(|f| f.label_index()).collect();
    let expected: Vec<Option<usize>> = (0..PLAIN_TAG_FIELDS.len()).map(Some).collect();
    assert_eq!(slots, expected, "each entry's slot is its own position, in order");

    let mut names: Vec<&str> = PLAIN_TAG_FIELDS.iter().map(|f| f.as_db_str()).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(names.len(), count, "two entries share a stable name");
}
