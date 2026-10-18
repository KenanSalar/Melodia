//! Tests for the needle itself: its fold, its shape flags and the three text predicates. What a
//! row is searchable by is `melodia-views`' `row_match_tests`.

use super::*;

#[test]
fn a_needle_with_a_non_digit_never_reaches_the_year() {
    // The guard that keeps the ordinary text query off the formatting path.
    // "199x" can't name a year, so a row whose only candidate is one misses.
    assert!(!fold_needle("199x").matches_number(Some(1998)));
    assert!(!fold_needle("").matches_number(Some(1998)));
}

/// The SQL filter asks this before it names the year column at all, so it has to agree with the
/// gate `matches_number` keeps for itself.
#[test]
fn only_a_run_of_digits_can_name_a_year() {
    assert!(fold_needle(" 1998 ").is_number());
    assert!(!fold_needle("199x").is_number());
    assert!(!fold_needle("-5").is_number());
    assert!(!fold_needle("").is_number());
}

#[test]
fn folding_is_idempotent() {
    // Views differ in whether they fold at write time or read time, and a
    // path that does both must not change the answer.
    let once = fold_needle("Björk");
    assert_eq!(fold_needle(once.as_str()), once);
}

/// Both shape flags are taken off the **folded** text, and nothing else here
/// can fail on it: the two arms of every predicate agree by construction, so
/// asking `raw` instead stays correct and quietly strands every accented needle
/// on the allocating one — which is the cost `Needle` exists to remove.
///
/// `Björk` is the case that shows it. It folds to `bjork`, so the ASCII byte
/// walk is reachable for it; the raw string is not ASCII and would never get
/// there.
#[test]
fn the_shape_flags_are_taken_after_the_fold() {
    assert!(fold_needle("Björk").ascii);
    assert!(fold_needle("  1998 ").digits);
    assert!(!fold_needle("199x").digits);
}

/// `Default` is hand-written so that it and `fold_needle("")` are the *same*
/// needle — a derived one leaves `ascii` false, which folding can never
/// produce. Behaviour is identical either way (an empty needle short-circuits
/// before the flag is read), so what the derive would actually cost is
/// `Needle::clear()`'s result taking the allocating arm of `equals` forever
/// after, and two needles that behave alike comparing unequal.
#[test]
fn the_default_needle_is_the_one_folding_an_empty_string_gives() {
    assert_eq!(Needle::default(), fold_needle(""));

    let mut cleared = fold_needle("Björk");
    cleared.clear();
    assert_eq!(cleared, fold_needle(""));
}

#[test]
fn equals_and_starts_with_fold_case_and_accents() {
    let needle = fold_needle("  BJORK ");
    assert!(needle.equals("Björk"));
    assert!(needle.starts_with("Björk"));
    assert!(needle.starts_with("Björk Guðmundsdóttir"));

    // A prefix is not an equality, and a suffix is neither.
    assert!(!needle.equals("Björk Guðmundsdóttir"));
    assert!(!needle.starts_with("The Björk"));
}

/// The ASCII fast path and the folding path have to answer alike — the fast
/// path skips the allocation, not the rule.
#[test]
fn the_ascii_fast_path_agrees_with_the_folding_path() {
    const CASES: [(&str, &str); 6] = [
        ("Rock", "rock"),
        ("Röck", "rock"),
        ("rock", "Röck"),
        ("Rockabilly", "rock"),
        ("Röckabilly", "rock"),
        ("Pop", "rock"),
    ];
    for (haystack, raw) in CASES {
        let needle = fold_needle(raw);
        let folded_haystack: String = {
            let mut out = String::new();
            push_folded(&mut out, haystack);
            out
        };
        assert_eq!(
            needle.equals(haystack),
            folded_haystack == needle.as_str(),
            "Needle::equals disagrees with the fold on {haystack:?} / {raw:?}"
        );
        assert_eq!(
            needle.starts_with(haystack),
            folded_haystack.starts_with(needle.as_str()),
            "Needle::starts_with disagrees with the fold on {haystack:?} / {raw:?}"
        );
    }
}

/// The empty needle splits the two on purpose: "contains nothing" is what
/// lets a filter walk run with an empty search bar, but "equals nothing" is a
/// real question, and only an empty string answers it.
#[test]
fn the_empty_needle_means_different_things_to_the_three_predicates() {
    let empty = fold_needle("");
    assert!(empty.contains("Björk"));
    assert!(empty.starts_with("Björk"));
    assert!(!empty.equals("Björk"));
    assert!(empty.equals(""));
}
