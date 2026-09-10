//! Role credits: the stored strings, and the line the index reads.

use super::super::credits::{CreditRole, ROLES, RoleCredit, RoleCredits};

fn credit(role: CreditRole, name: &str) -> RoleCredit {
    RoleCredit {
        role,
        name: name.to_owned(),
        detail: String::new(),
    }
}

/// The stored form is stable — renaming one is a data migration, not an edit — so the round trip
/// is what says a build can still read rows it wrote.
#[test]
fn every_role_round_trips_through_its_stored_string() {
    for role in ROLES {
        assert_eq!(CreditRole::from_db_str(role.as_db_str()), Some(role));
    }
}

/// A database written by a newer build can hold a role this one has never heard of, and reading it
/// back as `None` is what keeps that from becoming a panic or a wrong role.
#[test]
fn a_role_this_build_does_not_know_reads_as_nothing() {
    assert_eq!(CreditRole::from_db_str("director"), None);
    assert_eq!(CreditRole::from_db_str(""), None);
    assert_eq!(CreditRole::from_db_str("Composer"), None, "the stored form is lower case");
}

/// Two roles sharing a stored string would merge silently in `track_credits`, where the key is
/// (`track_id`, `role`, `position`) and nothing else compares them.
#[test]
fn no_two_roles_are_stored_under_the_same_string() {
    let mut stored: Vec<&str> = ROLES.iter().map(|role| role.as_db_str()).collect();
    stored.sort_unstable();
    stored.dedup();

    assert_eq!(stored.len(), ROLES.len());
}

/// Only a performer has an instrument or a voice, and a writer must not invent one for anybody
/// else — the detail travels inside the tag value, so an invented one renames the credit.
#[test]
fn only_a_performer_carries_a_detail() {
    let table = [
        (CreditRole::Composer, false),
        (CreditRole::Lyricist, false),
        (CreditRole::Arranger, false),
        (CreditRole::Conductor, false),
        (CreditRole::Performer, true),
        (CreditRole::Remixer, false),
        (CreditRole::Producer, false),
        (CreditRole::Engineer, false),
        (CreditRole::Mixer, false),
        (CreditRole::DjMixer, false),
    ];

    assert_eq!(table.len(), ROLES.len(), "a new role owes this table a row");
    for (role, carries) in table {
        assert_eq!(role.carries_detail(), carries, "{} answered wrong", role.as_db_str());
    }
}

/// **Each name once.** One person is routinely both composer and producer, and fts5's bm25 counts
/// a repeated token twice — the same weighting argument the index already makes about a filename
/// echoing the title.
#[test]
fn the_searchable_line_names_each_person_once() {
    let credits = RoleCredits::new(vec![
        credit(CreditRole::Composer, "Alice"),
        credit(CreditRole::Producer, "alice"),
        credit(CreditRole::Engineer, "Bob"),
    ]);

    assert_eq!(credits.line(), Some("Alice, Bob"));
}

/// First appearance, so the line reads as the tag wrote it rather than as the roles are ordered.
#[test]
fn the_line_reads_in_the_order_the_names_arrived() {
    let credits = RoleCredits::new(vec![
        credit(CreditRole::Producer, "Bob"),
        credit(CreditRole::Composer, "Alice"),
    ]);

    assert_eq!(credits.line(), Some("Bob, Alice"));
}

#[test]
fn a_set_with_nobody_in_it_renders_no_line() {
    let credits = RoleCredits::default();

    assert!(credits.is_empty());
    assert_eq!(credits.line(), None);
    assert!(credits.all().is_empty());
}

#[test]
fn a_role_yields_its_own_names_in_tag_order() {
    let credits = RoleCredits::new(vec![
        credit(CreditRole::Composer, "Alice"),
        credit(CreditRole::Producer, "Bob"),
        credit(CreditRole::Composer, "Carol"),
    ]);

    let composers: Vec<&str> =
        credits.for_role(CreditRole::Composer).map(|c| c.name.as_str()).collect();

    assert_eq!(composers, ["Alice", "Carol"]);
    assert_eq!(credits.for_role(CreditRole::Mixer).count(), 0);
}
