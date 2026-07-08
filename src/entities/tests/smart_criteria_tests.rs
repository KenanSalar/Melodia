use super::{
    FIELDS, LIMIT_ORDERS, LimitOrder, MATCH_MODES, MatchMode, Rule, RuleField, RuleOp, RuleValue,
    SMART_CRITERIA_VERSION, SmartCriteria, SmartLimit, ValueType, ops_for,
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

#[test]
fn editor_field_array_is_exhaustive_and_unique() {
    // Wildcard-free match: adding a `RuleField` variant fails to compile here
    // until it's given an arm — the reminder to also list it in `FIELDS` (and
    // the Slint `field-names` array that mirrors `FIELDS` by index).
    for &f in FIELDS {
        match f {
            RuleField::Title
            | RuleField::Artist
            | RuleField::AlbumArtist
            | RuleField::Album
            | RuleField::Genre
            | RuleField::Composer
            | RuleField::Comment
            | RuleField::Label
            | RuleField::Year
            | RuleField::Bpm
            | RuleField::DurationMs
            | RuleField::PlayCount
            | RuleField::SkipCount
            | RuleField::Rating
            | RuleField::Bitrate
            | RuleField::SampleRate
            | RuleField::FileSize
            | RuleField::Favorite
            | RuleField::LastPlayed
            | RuleField::DateAdded => {}
        }
    }
    // No field appears twice — a copy-paste slip would mis-map every index
    // past the duplicate.
    for (i, a) in FIELDS.iter().enumerate() {
        for b in &FIELDS[i + 1..] {
            assert_ne!(a, b, "duplicate field in FIELDS: {a:?}");
        }
    }
}

#[test]
fn editor_dropdown_orders_are_pinned() {
    // These leading positions are mirrored by the Slint dropdown arrays by
    // index (see `smart-playlist-editor-body.slint`). Reordering either side
    // silently mislabels the UI — pin the contract so it fails here instead.
    assert_eq!(FIELDS.first(), Some(&RuleField::Title));
    assert_eq!(MATCH_MODES.first(), Some(&MatchMode::All));
    assert_eq!(MATCH_MODES.get(1), Some(&MatchMode::Any));
    assert_eq!(LIMIT_ORDERS.first(), Some(&LimitOrder::DateAddedDesc));
    assert_eq!(ops_for(ValueType::Text).first(), Some(&RuleOp::Contains));
    assert_eq!(ops_for(ValueType::Number).first(), Some(&RuleOp::Eq));
    assert_eq!(ops_for(ValueType::Bool).first(), Some(&RuleOp::IsTrue));
    assert_eq!(ops_for(ValueType::Bool).get(1), Some(&RuleOp::IsFalse));
    assert_eq!(ops_for(ValueType::Date).first(), Some(&RuleOp::InLast));
}

#[test]
fn match_mode_and_limit_order_index_round_trip() {
    // `as_index` == array position and `from_index` inverts it, for every
    // entry in the editor-order arrays.
    for (i, &m) in MATCH_MODES.iter().enumerate() {
        assert_eq!(usize::try_from(m.as_index()).ok(), Some(i));
        if let Ok(idx) = i32::try_from(i) {
            assert_eq!(MatchMode::from_index(idx), m);
        }
    }
    for (i, &o) in LIMIT_ORDERS.iter().enumerate() {
        assert_eq!(usize::try_from(o.as_index()).ok(), Some(i));
        if let Ok(idx) = i32::try_from(i) {
            assert_eq!(LimitOrder::from_index(idx), o);
        }
    }
    // Out-of-range indices fall back to the default rather than panicking.
    assert_eq!(MatchMode::from_index(-1), MatchMode::default());
    assert_eq!(MatchMode::from_index(99), MatchMode::default());
    assert_eq!(LimitOrder::from_index(-1), LimitOrder::default());
    assert_eq!(LimitOrder::from_index(99), LimitOrder::default());
}
