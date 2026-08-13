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
fn common_str_agree_disagree_single_empty() {
    assert_eq!(common_str(std::iter::empty::<&str>()), (String::new(), false));
    assert_eq!(common_str(["A"].into_iter()), ("A".to_owned(), false));
    assert_eq!(common_str(["A", "A"].into_iter()), ("A".to_owned(), false));
    assert_eq!(common_str(["A", "B"].into_iter()), (String::new(), true));
}

#[test]
fn common_by_collapses_display_equal_values() {
    // Empty selection.
    assert_eq!(
        common_by(std::iter::empty::<Option<i32>>(), int_key, fmt_int),
        (String::new(), false)
    );
    // Agreement formats the winner once.
    assert_eq!(
        common_by([Some(5), Some(5)].into_iter(), int_key, fmt_int),
        ("5".to_owned(), false)
    );
    // Some(0) and None both render empty ⇒ they must agree (not disagree).
    assert_eq!(common_by([Some(0), None].into_iter(), int_key, fmt_int), (String::new(), false));
    // Genuinely different values disagree.
    assert_eq!(common_by([Some(5), Some(6)].into_iter(), int_key, fmt_int), (String::new(), true));
    // BPM: NaN and None both render empty ⇒ agree; distinct finite values disagree.
    assert_eq!(
        common_by([Some(f64::NAN), None].into_iter(), bpm_key, fmt_bpm),
        (String::new(), false)
    );
    assert_eq!(
        common_by([Some(128.0), Some(128.0)].into_iter(), bpm_key, fmt_bpm),
        ("128".to_owned(), false)
    );
    assert_eq!(
        common_by([Some(128.0), Some(130.0)].into_iter(), bpm_key, fmt_bpm),
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
