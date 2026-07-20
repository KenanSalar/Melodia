//! Pure diff + formatting logic for the tag editor. No UI, no fixtures — the
//! risky part of the commit path is turning the form snapshot into a `TagEdit`.

use super::*;

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
fn diff_u16_parses_and_guards() {
    assert_eq!(diff_u16("2020", "2020"), FieldEdit::Keep);
    assert_eq!(diff_u16("2021", "2020"), FieldEdit::Set(2021));
    assert_eq!(diff_u16("", "2020"), FieldEdit::Clear);
    // Non-numeric / out-of-range degrade to Keep — never write garbage.
    assert_eq!(diff_u16("nope", "2020"), FieldEdit::Keep);
    assert_eq!(diff_u16("99999", "2020"), FieldEdit::Keep);
}

#[test]
fn diff_u32_parses_and_guards() {
    assert_eq!(diff_u32("3", "1"), FieldEdit::Set(3));
    assert_eq!(diff_u32("", "1"), FieldEdit::Clear);
    assert_eq!(diff_u32("x", "1"), FieldEdit::Keep);
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

#[test]
fn build_edit_touches_only_changed_fields() {
    // title, artist, album_artist, album, genre, year, original_year,
    // track_number, disc_number, composer, comment, bpm, lyrics
    let orig: Vec<String> = [
        "Title", "Artist", "", "Album", "", "2020", "", "1", "", "", "", "120", "",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();

    let mut cur = orig.clone();
    cur[3] = "New Album".to_owned(); // change album
    cur[0] = String::new(); // clear title

    let edit = build_edit(&cur, &orig, ArtworkEdit::Keep);
    assert_eq!(edit.album, FieldEdit::Set("New Album".to_owned()));
    assert_eq!(edit.title, FieldEdit::Clear);
    assert_eq!(edit.artist, FieldEdit::Keep);
    assert_eq!(edit.year, FieldEdit::Keep);
    assert!(!edit.is_noop());

    // An unchanged form diffs to an all-Keep no-op.
    let noop = build_edit(&orig, &orig, ArtworkEdit::Keep);
    assert!(noop.is_noop());
}

#[test]
fn common_agree_disagree_single_empty() {
    assert_eq!(common(std::iter::empty::<String>()), (String::new(), false));
    assert_eq!(common(["A".to_owned()].into_iter()), ("A".to_owned(), false));
    assert_eq!(
        common(["A".to_owned(), "A".to_owned()].into_iter()),
        ("A".to_owned(), false)
    );
    assert_eq!(
        common(["A".to_owned(), "B".to_owned()].into_iter()),
        (String::new(), true)
    );
}

#[test]
fn formatting_helpers() {
    assert_eq!(fmt_int(Some(2020)), "2020");
    assert_eq!(fmt_int(Some(0)), "");
    assert_eq!(fmt_int(None), "");

    assert_eq!(fmt_bpm(Some(128.0)), "128");
    assert_eq!(fmt_bpm(Some(128.5)), "128.5");
    assert_eq!(fmt_bpm(Some(f64::NAN)), "");
    assert_eq!(fmt_bpm(None), "");

    assert_eq!(fmt_sample_rate(44100), "44.1 kHz");
    assert_eq!(fmt_sample_rate(48000), "48 kHz");

    assert_eq!(fmt_channels(1), "Mono");
    assert_eq!(fmt_channels(2), "Stereo");
    assert_eq!(fmt_channels(6), "6 channels");

    assert_eq!(fmt_size(512), "512 B");
    assert_eq!(fmt_size(2048), "2 KB");
    assert_eq!(fmt_size(5 * 1024 * 1024), "5.0 MB");
}
