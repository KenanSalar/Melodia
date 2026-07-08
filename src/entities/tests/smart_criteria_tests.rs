use super::{
    LimitOrder, MatchMode, Rule, RuleField, RuleOp, RuleValue, SMART_CRITERIA_VERSION, SmartCriteria,
    SmartLimit, ValueType, ops_for,
};

fn sample() -> SmartCriteria {
    SmartCriteria {
        version: SMART_CRITERIA_VERSION,
        match_mode: MatchMode::Any,
        rules: vec![
            Rule {
                field: RuleField::Genre,
                op: RuleOp::Contains,
                value: Some(RuleValue::Text("Rock".to_owned())),
            },
            Rule {
                field: RuleField::Rating,
                op: RuleOp::Gte,
                value: Some(RuleValue::Number(4.0)),
            },
            Rule {
                field: RuleField::LastPlayed,
                op: RuleOp::InLast,
                value: Some(RuleValue::Days(30)),
            },
            Rule {
                field: RuleField::Favorite,
                op: RuleOp::IsTrue,
                value: None,
            },
        ],
        limit: Some(SmartLimit {
            count: 50,
            order: LimitOrder::PlayCountDesc,
        }),
    }
}

#[test]
fn round_trip_preserves_all_fields() -> Result<(), serde_json::Error> {
    let c = sample();
    let json = c.to_json()?;
    assert_eq!(SmartCriteria::from_json_opt(Some(&json)), c);
    Ok(())
}

#[test]
fn none_and_empty_default() {
    let default = SmartCriteria::default();
    assert_eq!(SmartCriteria::from_json_opt(None), default);
    assert_eq!(SmartCriteria::from_json_opt(Some("")), default);
    assert_eq!(SmartCriteria::from_json_opt(Some("   ")), default);
}

#[test]
fn missing_fields_fall_back_to_defaults() {
    // Empty object: version / match_mode / rules / limit all defaulted.
    let c = SmartCriteria::from_json_opt(Some("{}"));
    assert_eq!(c.version, SMART_CRITERIA_VERSION);
    assert_eq!(c.match_mode, MatchMode::All);
    assert!(c.rules.is_empty());
    assert!(c.limit.is_none());
}

#[test]
fn unknown_keys_are_ignored() {
    // A field a future version might add must not break an older client.
    let json = r#"{"match_mode":"any","rules":[],"future_field":123}"#;
    let c = SmartCriteria::from_json_opt(Some(json));
    assert_eq!(c.match_mode, MatchMode::Any);
}

#[test]
fn malformed_json_defaults_without_panic() {
    assert_eq!(
        SmartCriteria::from_json_opt(Some("{not json")),
        SmartCriteria::default()
    );
    // Wrong top-level shape (array, not object) also degrades gracefully.
    assert_eq!(
        SmartCriteria::from_json_opt(Some("[1,2,3]")),
        SmartCriteria::default()
    );
}

#[test]
fn value_is_type_tagged_in_json() -> Result<(), serde_json::Error> {
    let c = SmartCriteria {
        rules: vec![Rule {
            field: RuleField::Genre,
            op: RuleOp::Is,
            value: Some(RuleValue::Text("Jazz".to_owned())),
        }],
        ..SmartCriteria::default()
    };
    let json = c.to_json()?;
    assert!(json.contains(r#""kind":"text""#), "value should be tagged: {json}");
    assert!(json.contains(r#""value":"Jazz""#), "value should serialize: {json}");
    Ok(())
}

#[test]
fn field_value_types_and_operator_lists_are_coherent() {
    // Value-type coverage (one per category).
    assert_eq!(RuleField::Title.value_type(), ValueType::Text);
    assert_eq!(RuleField::Year.value_type(), ValueType::Number);
    assert_eq!(RuleField::Favorite.value_type(), ValueType::Bool);
    assert_eq!(RuleField::LastPlayed.value_type(), ValueType::Date);

    // Every value category exposes at least one operator.
    for vt in [ValueType::Text, ValueType::Number, ValueType::Bool, ValueType::Date] {
        assert!(!ops_for(vt).is_empty(), "no operators for {vt:?}");
    }
}
