//! What a multi-track selection agrees on, per role.

use super::{common_roles, detail_for};
use melodia_core::entities::credits::{CreditRole, ROLES, RoleCredit, RoleCredits};
use melodia_core::entities::tags::RoleCreditEdit;

fn credit(role: CreditRole, name: &str, detail: &str) -> RoleCredit {
    RoleCredit { role, name: name.to_owned(), detail: detail.to_owned() }
}

fn spelled(credits: &RoleCredits) -> Vec<(&'static str, &str)> {
    credits.all().iter().map(|c| (c.role.as_db_str(), c.name.as_str())).collect()
}

/// The roles the fold could not answer for, which is what the ‹multiple values› hint marks and
/// what the writer is scoped out of.
fn disagreed(folded: &RoleCreditEdit) -> Vec<&'static str> {
    ROLES
        .into_iter()
        .zip(folded.answered())
        .filter(|(_, answered)| !answered)
        .map(|(role, _)| role.as_db_str())
        .collect()
}

/// **Per role, not for the set as a whole.** A batch sharing a composer and differing on the
/// producer should still show the composer, the way every other field the selection agrees on is
/// shown.
#[test]
fn a_selection_shows_the_roles_it_agrees_on_and_flags_the_rest() {
    let sets = [
        RoleCredits::new(vec![
            credit(CreditRole::Composer, "Alice", ""),
            credit(CreditRole::Producer, "Bob", ""),
        ]),
        RoleCredits::new(vec![
            credit(CreditRole::Composer, "Alice", ""),
            credit(CreditRole::Producer, "Carol", ""),
        ]),
    ];

    let folded = common_roles(&sets);

    assert_eq!(spelled(folded.credits()), vec![("composer", "Alice")]);
    assert_eq!(disagreed(&folded), ["producer"]);
}

/// A role one track leaves empty is a disagreement rather than an agreement on nothing — saving
/// the fold back would otherwise strip the credit off the track that had one.
#[test]
fn a_role_only_one_track_carries_is_a_disagreement() {
    let sets =
        [RoleCredits::new(vec![credit(CreditRole::Composer, "Alice", "")]), RoleCredits::default()];

    let folded = common_roles(&sets);

    assert!(folded.credits().is_empty());
    assert_eq!(disagreed(&folded), ["composer"]);
}

/// Order counts: two tracks crediting the same pair the other way round are not the same credit,
/// and `track_credits` keys on position.
#[test]
fn the_same_names_in_a_different_order_do_not_agree() {
    let sets = [
        RoleCredits::new(vec![
            credit(CreditRole::Composer, "Alice", ""),
            credit(CreditRole::Composer, "Bob", ""),
        ]),
        RoleCredits::new(vec![
            credit(CreditRole::Composer, "Bob", ""),
            credit(CreditRole::Composer, "Alice", ""),
        ]),
    ];

    let folded = common_roles(&sets);

    assert!(folded.credits().is_empty());
    assert_eq!(disagreed(&folded), ["composer"]);
}

#[test]
fn a_single_track_agrees_with_itself_about_everything() {
    let only = RoleCredits::new(vec![
        credit(CreditRole::Composer, "Alice", ""),
        credit(CreditRole::Performer, "Bob", "cello"),
    ]);

    let folded = common_roles(std::slice::from_ref(&only));

    assert_eq!(spelled(folded.credits()), spelled(&only));
    assert!(disagreed(&folded).is_empty());
}

#[test]
fn an_empty_selection_agrees_on_nothing_and_disagrees_about_nothing() {
    let folded = common_roles(&[]);

    assert!(folded.credits().is_empty());
    assert!(disagreed(&folded).is_empty());
}

/// **The editor has no field for an instrument**, so the baseline is where it comes from on the
/// way back — without this the read-back differs from the baseline for any track carrying one, the
/// diff answers `Set` on a form nobody touched, and the credit is written back stripped.
#[test]
fn a_performers_instrument_is_carried_back_from_the_baseline_by_name() {
    let original = RoleCredits::new(vec![
        credit(CreditRole::Performer, "Alice Monroe", "cello"),
        credit(CreditRole::Composer, "Alice Monroe", ""),
    ]);

    assert_eq!(detail_for(&original, CreditRole::Performer, "alice monroe"), "cello");
    // The same name in a role that carries no detail brings none back.
    assert_eq!(detail_for(&original, CreditRole::Composer, "Alice Monroe"), "");
    assert_eq!(detail_for(&original, CreditRole::Performer, "Someone Else"), "");
}
