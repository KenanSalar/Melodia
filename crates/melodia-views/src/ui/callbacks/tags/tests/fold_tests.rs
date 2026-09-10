//! Folding a selection down to what it agrees on, and rendering the result.

use super::*;

#[test]
fn common_str_agree_disagree_single_empty() {
    assert_eq!(common_str(std::iter::empty::<&str>()), (String::new(), false));
    assert_eq!(common_str(["A"].into_iter()), ("A".to_owned(), false));
    assert_eq!(common_str(["A", "A"].into_iter()), ("A".to_owned(), false));
    assert_eq!(common_str(["A", "B"].into_iter()), (String::new(), true));
}

/// [`common_str`]'s answers for a type compared as a whole, which is what the four multi-value
/// fields fold through.
#[test]
fn common_value_agrees_only_on_an_identical_list() {
    use melodia_core::entities::genre::GenreList;

    let rock = GenreList::from_name("Rock");
    let metal = GenreList::from_name("Metal");
    let both = GenreList::new(vec!["Rock".to_owned(), "Metal".to_owned()]);

    assert_eq!(common_value(std::iter::empty::<&GenreList>()), (GenreList::default(), false));
    assert_eq!(common_value([&rock].into_iter()), (rock.clone(), false));
    assert_eq!(common_value([&rock, &rock].into_iter()), (rock.clone(), false));
    assert_eq!(common_value([&rock, &metal].into_iter()), (GenreList::default(), true));
    // A list that merely *contains* the other is still a disagreement — the whole list is the
    // value, which is what stops a two-genre track folding onto a one-genre one.
    assert_eq!(common_value([&rock, &both].into_iter()), (GenreList::default(), true));
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

    assert_eq!(fmt_size(512), "512 B");
    assert_eq!(fmt_size(2048), "2 KB");
    assert_eq!(fmt_size(5 * 1024 * 1024), "5.0 MB");
}
