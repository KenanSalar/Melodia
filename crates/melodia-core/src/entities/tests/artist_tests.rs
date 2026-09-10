//! The credit line and the list behind it, held to each other.
//!
//! [`ArtistCredit::from_tags`] reads two tags that can disagree, and the whole design is that the
//! rendered line and the ordered names can never drift apart — a row displaying "X feat. Y" while
//! filing under whoever a stale list named is a bug nothing reports.

use super::super::artist::{ArtistCredit, JOIN_PHRASES};

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

/// The names and the phrase after each, which is the whole of what a credit holds.
fn spelled(credit: &ArtistCredit) -> Vec<(String, String)> {
    credit
        .artists()
        .iter()
        .map(|artist| (artist.name.clone(), artist.join_phrase.clone()))
        .collect()
}

#[test]
fn a_field_with_nothing_in_it_credits_nobody() {
    for printed in ["", "   "] {
        let credit = ArtistCredit::from_name(printed);

        assert!(credit.is_empty());
        assert_eq!(credit.line(), None);
        assert_eq!(credit.primary_name(), "");
    }
}

#[test]
fn one_name_is_one_credit_with_nothing_after_it() {
    let credit = ArtistCredit::from_name("  Alice  ");

    assert_eq!(spelled(&credit), vec![("Alice".to_owned(), String::new())]);
    assert_eq!(credit.line(), Some("Alice"));
}

/// A single value is one artist whatever punctuation it holds — the exceptions list that splitting
/// on it would need is the one nobody can finish.
#[test]
fn a_name_that_looks_like_a_list_is_still_one_name() {
    for printed in ["AC/DC", "Earth, Wind & Fire", "Alice feat. Bob"] {
        let credit = ArtistCredit::from_name(printed);

        assert_eq!(credit.artists().len(), 1, "{printed} is one artist without a list tag");
        assert_eq!(credit.primary_name(), printed);
    }
}

/// Driven over the picker's own table, so a phrase added to it is covered by construction rather
/// than by somebody remembering to add a case.
#[test]
fn every_join_phrase_the_picker_offers_round_trips() {
    for (label, rendered) in JOIN_PHRASES {
        let printed = format!("Alice{rendered}Bob");
        let credit = ArtistCredit::from_tags(&printed, &names(&["Alice", "Bob"]));

        assert_eq!(
            spelled(&credit),
            vec![
                ("Alice".to_owned(), rendered.to_owned()),
                ("Bob".to_owned(), String::new()),
            ],
            "the {label} phrase came back as something else"
        );
        assert_eq!(credit.line(), Some(printed.as_str()));
    }
}

/// **The line is not the primary name.** Keying an `artists` row on "X feat. Y" is what made a
/// featured collaboration read as a third artist nobody had ever recorded under.
#[test]
fn the_primary_name_is_the_first_credit_and_never_the_whole_line() {
    let credit = ArtistCredit::from_tags("Alice feat. Bob", &names(&["Alice", "Bob"]));

    assert_eq!(credit.primary_name(), "Alice");
    assert_eq!(credit.line(), Some("Alice feat. Bob"));
}

/// The derived phrases are checked against what they render back to, so a printed tag that does
/// not reproduce is not trusted to describe how the names join.
#[test]
fn a_printed_line_the_names_cannot_rebuild_falls_back_to_the_default_phrases() {
    for printed in [
        // Nothing of the first name at the front.
        "Someone Else",
        // The names in the other order.
        "Bob feat. Alice",
        // Trailing text no name accounts for, so the walk succeeds and the render disagrees.
        "Alice feat. Bob (live)",
    ] {
        let credit = ArtistCredit::from_tags(printed, &names(&["Alice", "Bob"]));

        assert_eq!(credit.line(), Some("Alice & Bob"), "{printed} should not survive as phrases");
        assert_eq!(credit.primary_name(), "Alice");
    }
}

/// A list with no readable printed line joins with a comma between and an ampersand before the
/// last, which is how a credit reads when nothing says otherwise.
#[test]
fn the_default_phrases_put_the_ampersand_before_the_last_name() {
    let lines = [
        (vec!["Alice"], "Alice"),
        (vec!["Alice", "Bob"], "Alice & Bob"),
        (vec!["Alice", "Bob", "Carol"], "Alice, Bob & Carol"),
        (vec!["Alice", "Bob", "Carol", "Dave"], "Alice, Bob, Carol & Dave"),
    ];

    for (list, expected) in lines {
        let credit = ArtistCredit::from_tags("", &names(&list));

        assert_eq!(credit.line(), Some(expected));
    }
}

/// `ARTISTS` is what says how many artists there are, so a list beside a printed line that only
/// names one still credits both.
#[test]
fn the_list_decides_the_names_even_where_the_printed_line_disagrees() {
    let credit = ArtistCredit::from_tags("Alice", &names(&["Alice", "Bob"]));

    assert_eq!(credit.artists().len(), 2);
    assert_eq!(credit.line(), Some("Alice & Bob"));
}
