//! On-disk model for a smart (dynamic) playlist's rule set.
//!
//! Serialized as JSON into `playlists.smart_criteria`. A smart playlist's
//! membership is *derived* by evaluating these rules against the `tracks` table
//! at read time (see [`crate::database::queries::smart_playlist`]) — it is never
//! materialized into `playlist_items`, so membership updates live as the library
//! changes.
//!
//! Forward-compatibility: every struct is `#[serde(default)]` and the enums use
//! `rename_all = "snake_case"`. Missing keys fall back to defaults and unknown
//! keys are ignored, so adding a field is additive without a [`SMART_CRITERIA_VERSION`]
//! bump; bump the version only for a semantic-breaking change and branch on it in
//! [`SmartCriteria::from_json_opt`].

use serde::{Deserialize, Serialize};

/// Schema version stamped into every serialized [`SmartCriteria`].
pub const SMART_CRITERIA_VERSION: u32 = 1;

/// A smart playlist's complete rule set — the deserialized form of the
/// `playlists.smart_criteria` TEXT column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartCriteria {
    pub version: u32,
    pub match_mode: MatchMode,
    pub rules: Vec<Rule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<SmartLimit>,
}

impl Default for SmartCriteria {
    fn default() -> Self {
        Self {
            version: SMART_CRITERIA_VERSION,
            match_mode: MatchMode::All,
            rules: Vec::new(),
            limit: None,
        }
    }
}

impl SmartCriteria {
    /// Parse a `smart_criteria` column value. Returns [`SmartCriteria::default`]
    /// for `None`/empty, and logs-and-defaults on a malformed blob (never panics
    /// — a corrupt criteria string must not take down a list view or playback).
    pub fn from_json_opt(raw: Option<&str>) -> Self {
        let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(raw) {
            Ok(criteria) => criteria,
            Err(e) => {
                log::warn!("malformed smart_criteria, falling back to defaults: {e}");
                Self::default()
            }
        }
    }

    /// Serialize to the JSON stored in `playlists.smart_criteria`.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Whether resolving this criteria's membership or order can be moved by a
    /// play-count flush (a `play_count` / `skip_count` / `last_played` change —
    /// the columns `stats_changed_tx` signals). Lets the Playlists grid skip
    /// re-counting smart playlists a stats bump can't affect. `skip_count` is
    /// included because a single flush can carry play *and* skip changes at the
    /// same bump. Conservative: a `true` here only ever costs a redundant
    /// recount, never a stale count.
    pub fn depends_on_play_stats(&self) -> bool {
        let order_is_stat = matches!(
            self.limit.as_ref().map(|l| l.order),
            Some(
                LimitOrder::PlayCountDesc
                    | LimitOrder::PlayCountAsc
                    | LimitOrder::LastPlayedDesc
                    | LimitOrder::LastPlayedAsc
            )
        );
        order_is_stat
            || self.rules.iter().any(|r| {
                matches!(
                    r.field,
                    RuleField::PlayCount | RuleField::SkipCount | RuleField::LastPlayed
                )
            })
    }
}

/// Whether a track must satisfy every rule (`All` → AND / intersection) or any
/// single rule (`Any` → OR / union).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    #[default]
    All,
    Any,
}

/// One `field op value?` condition. `value` is absent for the presence
/// (`is_set`/`is_not_set`) and boolean (`is_true`/`is_false`) operators.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub field: RuleField,
    pub op: RuleOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<RuleValue>,
}

/// A filterable `tracks` column. Each field has a fixed [`ValueType`] that
/// determines its available operators and value-input widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleField {
    // text
    Title,
    Artist,
    AlbumArtist,
    Album,
    Genre,
    // numeric
    Year,
    DurationMs,
    PlayCount,
    SkipCount,
    Rating,
    Bitrate,
    SampleRate,
    FileSize,
    // boolean
    Favorite,
    // date (RFC-3339 TEXT)
    LastPlayed,
    DateAdded,
}

impl RuleField {
    /// The value category this field filters on — drives operator availability
    /// (see [`ops_for`]) and, in the editor, the value-input widget.
    pub fn value_type(self) -> ValueType {
        match self {
            RuleField::Title
            | RuleField::Artist
            | RuleField::AlbumArtist
            | RuleField::Album
            | RuleField::Genre => ValueType::Text,
            RuleField::Year
            | RuleField::DurationMs
            | RuleField::PlayCount
            | RuleField::SkipCount
            | RuleField::Rating
            | RuleField::Bitrate
            | RuleField::SampleRate
            | RuleField::FileSize => ValueType::Number,
            RuleField::Favorite => ValueType::Bool,
            RuleField::LastPlayed | RuleField::DateAdded => ValueType::Date,
        }
    }
}

/// Coarse value category of a [`RuleField`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Text,
    Number,
    Bool,
    Date,
}

/// The comparison applied by a [`Rule`]. Which operators are valid depends on
/// the field's [`ValueType`] — see [`ops_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOp {
    // text
    Contains,
    NotContains,
    Is,
    IsNot,
    StartsWith,
    EndsWith,
    // numeric
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    // date-relative (value = whole days)
    InLast,
    NotInLast,
    // boolean
    IsTrue,
    IsFalse,
    // presence (NULL / empty)
    IsSet,
    IsNotSet,
}

/// The typed value carried by a [`Rule`]. Adjacently tagged so the JSON is
/// self-describing and a value can't be silently coerced across types
/// (`{"kind":"text","value":"Rock"}`, `{"kind":"number","value":4.0}`,
/// `{"kind":"days","value":365}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RuleValue {
    Text(String),
    Number(f64),
    Days(i64),
}

/// Optional cap on a smart playlist's size, with the order used to pick which
/// tracks survive the cut ("50 tracks, most recently added").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartLimit {
    pub count: u32,
    pub order: LimitOrder,
}

impl Default for SmartLimit {
    fn default() -> Self {
        Self {
            count: 25,
            order: LimitOrder::DateAddedDesc,
        }
    }
}

/// Selection order applied when a [`SmartLimit`] caps the result set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitOrder {
    #[default]
    DateAddedDesc,
    DateAddedAsc,
    PlayCountDesc,
    PlayCountAsc,
    LastPlayedDesc,
    LastPlayedAsc,
    RatingDesc,
    Random,
}

/// The operators valid for a given value category. This is the single source of
/// truth the evaluator uses to reject an incoherent `(field, op)` pair, and (in
/// the editor) that the operator dropdown mirrors by index.
pub fn ops_for(value_type: ValueType) -> &'static [RuleOp] {
    match value_type {
        ValueType::Text => TEXT_OPS,
        ValueType::Number => NUMBER_OPS,
        ValueType::Bool => BOOL_OPS,
        ValueType::Date => DATE_OPS,
    }
}

const TEXT_OPS: &[RuleOp] = &[
    RuleOp::Contains,
    RuleOp::NotContains,
    RuleOp::Is,
    RuleOp::IsNot,
    RuleOp::StartsWith,
    RuleOp::EndsWith,
    RuleOp::IsSet,
    RuleOp::IsNotSet,
];

const NUMBER_OPS: &[RuleOp] = &[
    RuleOp::Eq,
    RuleOp::Ne,
    RuleOp::Gt,
    RuleOp::Gte,
    RuleOp::Lt,
    RuleOp::Lte,
    RuleOp::IsSet,
    RuleOp::IsNotSet,
];

const BOOL_OPS: &[RuleOp] = &[RuleOp::IsTrue, RuleOp::IsFalse];

const DATE_OPS: &[RuleOp] = &[
    RuleOp::InLast,
    RuleOp::NotInLast,
    RuleOp::IsSet,
    RuleOp::IsNotSet,
];

// ---------------------------------------------------------------------------
// Editor (UI) bridge
//
// The rule-builder dialog is index-driven: dropdowns pick a field / operator by
// position. [`FIELDS`] is the single source of truth for the field ordering —
// the Slint field-name dropdown array mirrors it by index — and the helpers
// below map a field's value type and an operator's input requirement to the
// small integer codes the Slint `SmartRuleRow` carries.
// ---------------------------------------------------------------------------

/// Editor field order. The Slint field-name dropdown mirrors this by index, and
/// the editor maps a dropdown index back to a [`RuleField`] through it.
pub const FIELDS: &[RuleField] = &[
    RuleField::Title,
    RuleField::Artist,
    RuleField::AlbumArtist,
    RuleField::Album,
    RuleField::Genre,
    RuleField::Year,
    RuleField::DurationMs,
    RuleField::PlayCount,
    RuleField::SkipCount,
    RuleField::Rating,
    RuleField::Bitrate,
    RuleField::SampleRate,
    RuleField::FileSize,
    RuleField::Favorite,
    RuleField::LastPlayed,
    RuleField::DateAdded,
];

/// Match-mode editor order. The Slint `match-modes` dropdown mirrors this by
/// index; [`MatchMode::as_index`] / [`MatchMode::from_index`] derive both
/// directions from it, so the ordering lives in exactly one place.
pub const MATCH_MODES: &[MatchMode] = &[MatchMode::All, MatchMode::Any];

/// Limit-order editor order. The Slint `limit-orders` dropdown mirrors this by
/// index; [`LimitOrder::as_index`] / [`LimitOrder::from_index`] derive both
/// directions from it.
pub const LIMIT_ORDERS: &[LimitOrder] = &[
    LimitOrder::DateAddedDesc,
    LimitOrder::DateAddedAsc,
    LimitOrder::PlayCountDesc,
    LimitOrder::PlayCountAsc,
    LimitOrder::LastPlayedDesc,
    LimitOrder::LastPlayedAsc,
    LimitOrder::RatingDesc,
    LimitOrder::Random,
];

/// Position of `value` in `slice` as an `i32` dropdown index, or `0` when
/// absent. Used by the `as_index` helpers over the editor-order arrays, where
/// the value is always present (every enum variant is listed).
fn index_of<T: PartialEq>(slice: &[T], value: &T) -> i32 {
    slice.iter().position(|v| v == value).and_then(|i| i32::try_from(i).ok()).unwrap_or(0)
}

/// The element at an `i32` dropdown index, if in range.
fn at_index<T>(slice: &[T], index: i32) -> Option<&T> {
    usize::try_from(index).ok().and_then(|i| slice.get(i))
}

impl MatchMode {
    /// Dropdown index for this mode (mirrors [`MATCH_MODES`] / the Slint
    /// `match-modes` array by position).
    pub fn as_index(self) -> i32 {
        index_of(MATCH_MODES, &self)
    }

    /// Mode at a dropdown index, falling back to the default on out-of-range.
    pub fn from_index(index: i32) -> Self {
        at_index(MATCH_MODES, index).copied().unwrap_or_default()
    }
}

impl LimitOrder {
    /// Dropdown index for this order (mirrors [`LIMIT_ORDERS`] / the Slint
    /// `limit-orders` array by position).
    pub fn as_index(self) -> i32 {
        index_of(LIMIT_ORDERS, &self)
    }

    /// Order at a dropdown index, falling back to the default on out-of-range.
    pub fn from_index(index: i32) -> Self {
        at_index(LIMIT_ORDERS, index).copied().unwrap_or_default()
    }
}

impl ValueType {
    /// `field-kind` code the editor's `SmartRuleRow` carries (selects the
    /// operator dropdown array).
    pub fn as_index(self) -> i32 {
        match self {
            ValueType::Text => 0,
            ValueType::Number => 1,
            ValueType::Bool => 2,
            ValueType::Date => 3,
        }
    }
}

/// What value input, if any, an operator needs in the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    None,
    Text,
    Number,
}

impl InputKind {
    /// `input-kind` code the editor's `SmartRuleRow` carries (selects the value
    /// widget: none / text field / number field).
    pub fn as_index(self) -> i32 {
        match self {
            InputKind::None => 0,
            InputKind::Text => 1,
            InputKind::Number => 2,
        }
    }
}

impl RuleOp {
    /// The value input this operator needs for a field of `value_type`.
    /// Presence and boolean operators take no value; relative-date operators
    /// take a whole-day count (a number); otherwise text fields take text and
    /// numeric fields take a number.
    pub fn input_kind(self, value_type: ValueType) -> InputKind {
        match self {
            RuleOp::IsSet | RuleOp::IsNotSet | RuleOp::IsTrue | RuleOp::IsFalse => InputKind::None,
            RuleOp::InLast | RuleOp::NotInLast => InputKind::Number,
            _ => match value_type {
                ValueType::Text => InputKind::Text,
                _ => InputKind::Number,
            },
        }
    }
}

impl RuleValue {
    /// Build a rule value from the editor's raw text for `(value_type, op)`.
    /// Returns `None` when the operator takes no value, or when a numeric input
    /// fails to parse (the evaluator then treats the rule as incomplete and
    /// skips it).
    pub fn from_input(value_type: ValueType, op: RuleOp, raw: &str) -> Option<Self> {
        match op.input_kind(value_type) {
            InputKind::None => None,
            InputKind::Text => Some(RuleValue::Text(raw.trim().to_owned())),
            InputKind::Number => {
                if matches!(op, RuleOp::InLast | RuleOp::NotInLast) {
                    Some(RuleValue::Days(raw.trim().parse::<i64>().ok()?))
                } else {
                    Some(RuleValue::Number(raw.trim().parse::<f64>().ok()?))
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/smart_criteria_tests.rs"]
mod tests;
