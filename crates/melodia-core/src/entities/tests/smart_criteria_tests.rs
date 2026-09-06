use super::{
    FIELDS, InputKind, LIMIT_ORDERS, LimitOrder, MATCH_MODES, MatchMode, Rule, RuleField, RuleOp,
    RuleValue, SMART_CRITERIA_VERSION, SmartCriteria, SmartLimit, ValueType, ops_for,
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
    assert_eq!(SmartCriteria::from_json_opt(Some("{not json")), SmartCriteria::default());
    // Wrong top-level shape (array, not object) also degrades gracefully.
    assert_eq!(SmartCriteria::from_json_opt(Some("[1,2,3]")), SmartCriteria::default());
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
    for vt in [
        ValueType::Text,
        ValueType::Number,
        ValueType::Bool,
        ValueType::Date,
    ] {
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
            | RuleField::Year
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
fn slint_dropdown_arrays_match_rust_editor_order_lengths() {
    // `editor_dropdown_orders_are_pinned` fixes the leading *order* of each
    // mirrored array; this fixes the *length*. Shortening one side without the
    // other compiles and mislabels every dropdown row past the drift point, so
    // pin it here instead of relying on the `.slint`'s count comments.
    const EDITOR: &str = include_str!(
        "../../../../melodia-ui/ui/components/dialog/smart-playlist-editor-body.slint"
    );

    // `@tr(` occurrences inside `property <[string]> <name>: [ … ];`. `None`
    // when the array is renamed or removed, which fails the `assert_eq!` loudly.
    fn tr_count(name: &str) -> Option<usize> {
        let (_, tail) = EDITOR.split_once(&format!("property <[string]> {name}: ["))?;
        let (body, _) = tail.split_once("];")?;
        Some(body.matches("@tr(").count())
    }

    assert_eq!(tr_count("field-names"), Some(FIELDS.len()));
    assert_eq!(tr_count("text-ops"), Some(ops_for(ValueType::Text).len()));
    assert_eq!(tr_count("number-ops"), Some(ops_for(ValueType::Number).len()));
    assert_eq!(tr_count("bool-ops"), Some(ops_for(ValueType::Bool).len()));
    assert_eq!(tr_count("date-ops"), Some(ops_for(ValueType::Date).len()));
    assert_eq!(tr_count("match-modes"), Some(MATCH_MODES.len()));
    assert_eq!(tr_count("limit-orders"), Some(LIMIT_ORDERS.len()));
}

#[test]
fn depends_on_play_stats_classification() {
    // Helper: criteria with a single rule on `field` (value shape irrelevant to
    // the stat-dependence check, which only inspects the field) and no limit.
    let with_field = |field: RuleField| SmartCriteria {
        rules: vec![Rule {
            field,
            op: RuleOp::IsSet,
            value: None,
        }],
        limit: None,
        ..SmartCriteria::default()
    };
    // Helper: no rules, just a limit order.
    let with_order = |order: LimitOrder| SmartCriteria {
        rules: Vec::new(),
        limit: Some(SmartLimit { count: 25, order }),
        ..SmartCriteria::default()
    };

    // Stat-dependent via rule field.
    assert!(with_field(RuleField::PlayCount).depends_on_play_stats());
    assert!(with_field(RuleField::SkipCount).depends_on_play_stats());
    assert!(with_field(RuleField::LastPlayed).depends_on_play_stats());

    // Stat-independent rule fields — a play-count flush can't move these.
    assert!(!with_field(RuleField::Genre).depends_on_play_stats());
    assert!(!with_field(RuleField::Rating).depends_on_play_stats());
    assert!(!with_field(RuleField::DateAdded).depends_on_play_stats());

    // Stat-dependent via limit order.
    assert!(with_order(LimitOrder::PlayCountDesc).depends_on_play_stats());
    assert!(with_order(LimitOrder::PlayCountAsc).depends_on_play_stats());
    assert!(with_order(LimitOrder::LastPlayedDesc).depends_on_play_stats());
    assert!(with_order(LimitOrder::LastPlayedAsc).depends_on_play_stats());

    // Stat-independent limit orders.
    assert!(!with_order(LimitOrder::DateAddedDesc).depends_on_play_stats());
    assert!(!with_order(LimitOrder::RatingDesc).depends_on_play_stats());
    assert!(!with_order(LimitOrder::Random).depends_on_play_stats());

    // Empty criteria (whole library) is stat-independent.
    assert!(!SmartCriteria::default().depends_on_play_stats());

    // The mixed `sample()` (LastPlayed rule + PlayCountDesc order) is dependent.
    assert!(sample().depends_on_play_stats());
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

/// The `RuleValue` variant an operator produces, reduced to something a table can state.
/// Deriving it from `input_kind` would restate the code under test and pass against every
/// way that code can be wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    NoValue,
    Text,
    Number,
    Days,
}

/// Every `(value type, operator)` pair the rule editor can present, against the shape
/// `melodia-store`'s renderability gate demands for it.
const OFFERED_SHAPES: &[(ValueType, RuleOp, Shape)] = &[
    (ValueType::Text, RuleOp::Contains, Shape::Text),
    (ValueType::Text, RuleOp::NotContains, Shape::Text),
    (ValueType::Text, RuleOp::Is, Shape::Text),
    (ValueType::Text, RuleOp::IsNot, Shape::Text),
    (ValueType::Text, RuleOp::StartsWith, Shape::Text),
    (ValueType::Text, RuleOp::EndsWith, Shape::Text),
    (ValueType::Text, RuleOp::IsSet, Shape::NoValue),
    (ValueType::Text, RuleOp::IsNotSet, Shape::NoValue),
    (ValueType::Number, RuleOp::Eq, Shape::Number),
    (ValueType::Number, RuleOp::Ne, Shape::Number),
    (ValueType::Number, RuleOp::Gt, Shape::Number),
    (ValueType::Number, RuleOp::Gte, Shape::Number),
    (ValueType::Number, RuleOp::Lt, Shape::Number),
    (ValueType::Number, RuleOp::Lte, Shape::Number),
    (ValueType::Number, RuleOp::IsSet, Shape::NoValue),
    (ValueType::Number, RuleOp::IsNotSet, Shape::NoValue),
    (ValueType::Bool, RuleOp::IsTrue, Shape::NoValue),
    (ValueType::Bool, RuleOp::IsFalse, Shape::NoValue),
    (ValueType::Date, RuleOp::InLast, Shape::Days),
    (ValueType::Date, RuleOp::NotInLast, Shape::Days),
    (ValueType::Date, RuleOp::IsSet, Shape::NoValue),
    (ValueType::Date, RuleOp::IsNotSet, Shape::NoValue),
];

fn shape_of(value: Option<&RuleValue>) -> Shape {
    match value {
        None => Shape::NoValue,
        Some(RuleValue::Text(_)) => Shape::Text,
        Some(RuleValue::Number(_)) => Shape::Number,
        Some(RuleValue::Days(_)) => Shape::Days,
    }
}

/// An operator with no row here is an operator the sweep below stops covering, which is
/// the way this pair of tests would go quiet rather than fail.
#[test]
fn the_shape_table_covers_exactly_the_operators_the_editor_offers() {
    for vt in [
        ValueType::Text,
        ValueType::Number,
        ValueType::Bool,
        ValueType::Date,
    ] {
        let declared: Vec<RuleOp> =
            OFFERED_SHAPES.iter().filter(|(v, ..)| *v == vt).map(|&(_, op, _)| op).collect();
        assert_eq!(
            declared.as_slice(),
            ops_for(vt),
            "a new operator owes OFFERED_SHAPES a row saying what value it takes"
        );
    }
}

/// The dropdown, the value box and the evaluator's renderability gate agree on a rule's
/// value shape only by construction: `ops_for` picks the operator, `input_kind` sizes the
/// input, and `rule_is_renderable` one crate over demands a specific `RuleValue` variant.
/// A disagreement raises nothing and drops the rule, and `push_where` turns a rule set
/// that is entirely dropped into no `WHERE` at all, so a playlist asking for one artist
/// comes back holding the whole library.
///
/// `input_kind`'s text arm is a catch-all over the value type, so the pair that breaks
/// this is a text operator offered for a field that is not text.
#[test]
fn every_offered_operator_builds_the_value_shape_it_declares() {
    for &(vt, op, shape) in OFFERED_SHAPES {
        let built = RuleValue::from_input(vt, op, "5");
        assert_eq!(
            shape_of(built.as_ref()),
            shape,
            "{vt:?} + {op:?} is skipped by the evaluator, not rendered"
        );
    }
}

/// The *intended* skip, which answers identically to the accidental one above and is why
/// the sweep is written against a value that parses: an empty or unparseable box means
/// the user has not finished the rule, and an incomplete rule is dropped rather than
/// guessed at.
#[test]
fn a_numeric_operator_given_something_that_is_not_a_number_builds_no_value() {
    assert_eq!(RuleValue::from_input(ValueType::Number, RuleOp::Gte, "four"), None);
    assert_eq!(RuleValue::from_input(ValueType::Number, RuleOp::Eq, ""), None);
    assert_eq!(RuleValue::from_input(ValueType::Date, RuleOp::InLast, "thirty"), None);
    // Days are whole, so a fractional one is not a smaller day.
    assert_eq!(RuleValue::from_input(ValueType::Date, RuleOp::InLast, "1.5"), None);
}

/// Surrounding space comes free with a paste, and an untrimmed value reaches SQL as a
/// `LIKE` nothing matches.
#[test]
fn a_pasted_value_is_trimmed_before_it_becomes_a_rule() {
    assert_eq!(
        RuleValue::from_input(ValueType::Text, RuleOp::Is, "  Jazz  "),
        Some(RuleValue::Text("Jazz".to_owned()))
    );
    assert_eq!(
        RuleValue::from_input(ValueType::Number, RuleOp::Gte, "  4  "),
        Some(RuleValue::Number(4.0))
    );
    assert_eq!(
        RuleValue::from_input(ValueType::Date, RuleOp::InLast, "  30  "),
        Some(RuleValue::Days(30))
    );
}

/// `SmartRuleRow.input-kind` picks which value widget the editor mounts and the `.slint`
/// branches on the bare numbers, the days suffix reading `field-kind` beside it. A code
/// that moves here mounts the wrong input, or none, with nothing failing.
#[test]
fn the_editor_widget_codes_are_the_ones_the_slint_branches_on() {
    assert_eq!(InputKind::None.as_index(), 0);
    assert_eq!(InputKind::Text.as_index(), 1);
    assert_eq!(InputKind::Number.as_index(), 2);
    assert_eq!(ValueType::Date.as_index(), 3);
}
