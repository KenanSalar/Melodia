//! The genre list, and the split that reads one back out of a rendered column.

use super::super::genre::GenreList;

fn list(names: &[&str]) -> GenreList {
    GenreList::new(names.iter().map(|name| (*name).to_owned()).collect())
}

fn split(line: &str) -> Vec<&str> {
    GenreList::names_in_line(line).collect()
}

/// **Each name once, case-insensitively.** `genres.name` is `UNIQUE COLLATE NOCASE`, so a repeat
/// resolves to one `genres` row and writes a second `track_genres` row under a key
/// (`track_id`, `position`) that rejects nothing — leaving the triggers *and* the recompute
/// agreeing on twice the truth.
#[test]
fn a_name_spelled_twice_is_kept_once_as_the_tag_first_wrote_it() {
    assert_eq!(list(&["Rock", "rock"]).names(), ["Rock"]);
    assert_eq!(list(&["Rock", "Metal", "ROCK"]).names(), ["Rock", "Metal"]);
}

#[test]
fn a_list_with_nothing_in_it_renders_as_nothing() {
    for empty in [list(&[]), GenreList::from_name(""), GenreList::default()] {
        assert!(empty.is_empty());
        assert_eq!(empty.line(), None);
        assert_eq!(empty.primary(), None);
    }
}

/// The primary is the one `tracks.genre_id` points at, and it is tag order rather than any
/// ordering of its own.
#[test]
fn the_primary_genre_is_the_first_one_tagged() {
    assert_eq!(list(&["Metal", "Rock"]).primary(), Some("Metal"));
    assert_eq!(GenreList::from_name("Rock").primary(), Some("Rock"));
}

#[test]
fn the_rendered_line_splits_back_into_the_names_it_was_built_from() {
    let genres = list(&["Rock", "Metal"]);

    assert_eq!(genres.line(), Some("Rock, Metal"));
    assert_eq!(split(genres.line().unwrap_or_default()), genres.names());
}

/// **The split is display-only and can hand back a name nobody tagged.** A genre whose own name
/// holds the separator is indistinguishable from two once rendered, which is why anything that
/// *persists* a genre reads the rows instead.
#[test]
fn a_name_holding_the_separator_comes_back_as_two() {
    assert_eq!(split("Chanson, Francaise"), ["Chanson", "Francaise"]);
}

/// The separator carries its space, so a comma inside a name stays inside it.
#[test]
fn a_comma_with_no_space_after_it_is_not_a_separator() {
    assert_eq!(split("Rock,Metal"), ["Rock,Metal"]);
}

#[test]
fn a_line_with_nothing_in_it_names_no_genres() {
    assert!(split("").is_empty());
    assert!(split(", ").is_empty());
}
