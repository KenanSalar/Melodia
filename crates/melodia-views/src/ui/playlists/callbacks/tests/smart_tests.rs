use slint::SharedString;

use melodia_core::entities::smart_criteria::{FIELDS, Rule, RuleField, RuleOp, RuleValue, ops_for};
use melodia_ui::SmartRuleRow;

use super::{field_at, first_op, op_at, rebuilt_row, row_to_rule, rule_row, rule_to_row};

fn rule(field: RuleField, op: RuleOp, value: Option<RuleValue>) -> Rule {
    Rule { field, op, value }
}

/// Editing a saved playlist reads its rules into rows and Save reads them back, so a rule the trip
/// changes is a playlist silently rewritten by opening it. One row per kind of value.
#[test]
fn a_stored_rule_round_trips_through_its_editor_row() {
    let rules = [
        rule(RuleField::Title, RuleOp::Contains, Some(RuleValue::Text("night".to_owned()))),
        rule(RuleField::Year, RuleOp::Gte, Some(RuleValue::Number(1990.0))),
        rule(RuleField::Bpm, RuleOp::Gt, Some(RuleValue::Number(120.5))),
        rule(RuleField::LastPlayed, RuleOp::InLast, Some(RuleValue::Days(30))),
        rule(RuleField::Favorite, RuleOp::IsTrue, None),
        rule(RuleField::Genre, RuleOp::IsNotSet, None),
    ];

    let read_back = rules.clone().map(|rule| rule_to_row(&rule).as_ref().and_then(row_to_rule));

    assert_eq!(read_back, rules.map(Some));
}

/// The dropdowns can't show an operator the field doesn't offer, so the row would read back as a
/// different rule than the one stored.
#[test]
fn a_rule_naming_an_operator_its_field_does_not_offer_is_dropped() {
    let stray = rule(RuleField::Title, RuleOp::Gt, Some(RuleValue::Number(3.0)));

    assert_eq!(rule_to_row(&stray), None);
}

/// A row carries its dropdown positions rather than its field and operator, so every pair the
/// editor offers has to land on positions that read back as that pair, and fractions are let into
/// the value only where the column holds them.
#[test]
fn every_offered_field_and_operator_mounts_at_its_own_indices() {
    let misread: Vec<(RuleField, RuleOp)> = FIELDS
        .iter()
        .flat_map(|&field| ops_for(field.value_type()).iter().map(move |&op| (field, op)))
        .filter(|&(field, op)| {
            let row = rule_row(field, op);
            let read = (field_at(row.field_index), op_at(field.value_type(), row.op_index));
            (read, row.fractional) != ((field, op), field.holds_fractions())
        })
        .collect();

    assert!(misread.is_empty(), "rows that read back as another rule: {misread:?}");
}

/// A field or operator change remounts the value input, and text kept across a change of kind
/// would sit in an input that can't take it.
#[test]
fn an_edit_keeps_the_value_only_while_the_input_kind_holds() {
    let typed = SmartRuleRow {
        value_text: SharedString::from("night"),
        ..rule_row(RuleField::Title, RuleOp::Contains)
    };
    let edits = [
        (RuleField::Artist, first_op(RuleField::Artist.value_type())),
        (RuleField::Year, first_op(RuleField::Year.value_type())),
        (RuleField::Title, RuleOp::IsSet),
    ];

    let kept = edits.map(|(field, op)| rebuilt_row(field, op, &typed).value_text);

    assert_eq!(
        kept,
        [SharedString::from("night"), SharedString::default(), SharedString::default()],
        "(text to text, text to number, text to presence)"
    );
}
