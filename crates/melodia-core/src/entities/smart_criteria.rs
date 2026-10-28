//! A smart playlist's rule set, the JSON in `playlists.smart_criteria`, evaluated at read time
//! and never materialized. Every struct is `#[serde(default)]`, so a new field needs no
//! [`SMART_CRITERIA_VERSION`] bump; bump it only for a breaking change, branching in
//! [`SmartCriteria::from_json_opt`].

use serde::{Deserialize, Serialize};

use crate::error::describe;

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
    /// The default for `None` or empty, and logs-and-defaults on a malformed blob, which must not
    /// take down a list view.
    pub fn from_json_opt(raw: Option<&str>) -> Self {
        let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(raw) {
            Ok(criteria) => criteria,
            Err(e) => {
                log::warn!("malformed smart_criteria, falling back to defaults: {}", describe(&e));
                Self::default()
            }
        }
    }

    /// Serialize to the JSON stored in `playlists.smart_criteria`.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Whether a play-count flush can move membership or order, `skip_count` included since one
    /// flush carries both. Conservative: a wrong `true` costs a recount, never a stale count.
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
    /// Everyone credited in any role, matched against the `track_credits` rows rather than the
    /// rendered line, so a rule finds a name wherever the tag put it.
    Credits,
    Mood,
    InitialKey,
    Isrc,
    // numeric
    Year,
    Bpm,
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
            | RuleField::Genre
            | RuleField::Credits
            | RuleField::Mood
            | RuleField::InitialKey
            | RuleField::Isrc => ValueType::Text,
            RuleField::Year
            | RuleField::Bpm
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

    /// Whether a value can carry a fractional part. `bpm` is the one `REAL` column, so the editor
    /// keeps every other numeric field to digits.
    pub fn holds_fractions(self) -> bool {
        matches!(self, RuleField::Bpm)
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

/// Adjacently tagged, so the JSON is self-describing and a value can't be coerced across types.
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
        Self { count: 25, order: LimitOrder::DateAddedDesc }
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

/// Which tracks a [`LimitOrder::Random`] cap picks. One draw picks the same tracks out of the
/// same matches, so a page can sort, filter and add up one pick across separate queries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Draw(pub u64);

/// What the evaluator rejects an incoherent `(field, op)` pair by, and what the operator dropdown
/// mirrors by index.
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

const DATE_OPS: &[RuleOp] = &[RuleOp::InLast, RuleOp::NotInLast, RuleOp::IsSet, RuleOp::IsNotSet];

// The rule editor picks by position, each array below mirrored by index in a Slint dropdown
// (`smart_criteria_tests` pins order and length), and the codes below are what `SmartRuleRow`
// carries.

pub const FIELDS: &[RuleField] = &[
    RuleField::Title,
    RuleField::Artist,
    RuleField::AlbumArtist,
    RuleField::Album,
    RuleField::Genre,
    RuleField::Credits,
    RuleField::Mood,
    RuleField::InitialKey,
    RuleField::Isrc,
    RuleField::Year,
    RuleField::Bpm,
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

pub const MATCH_MODES: &[MatchMode] = &[MatchMode::All, MatchMode::Any];

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

/// `0` when absent, which over an array listing every variant it never is.
fn index_of<T: PartialEq>(slice: &[T], value: &T) -> i32 {
    slice.iter().position(|v| v == value).and_then(|i| i32::try_from(i).ok()).unwrap_or(0)
}

fn at_index<T>(slice: &[T], index: i32) -> Option<&T> {
    usize::try_from(index).ok().and_then(|i| slice.get(i))
}

impl MatchMode {
    pub fn as_index(self) -> i32 {
        index_of(MATCH_MODES, &self)
    }

    /// The default on an out-of-range index.
    pub fn from_index(index: i32) -> Self {
        at_index(MATCH_MODES, index).copied().unwrap_or_default()
    }
}

impl LimitOrder {
    pub fn as_index(self) -> i32 {
        index_of(LIMIT_ORDERS, &self)
    }

    /// The default on an out-of-range index.
    pub fn from_index(index: i32) -> Self {
        at_index(LIMIT_ORDERS, index).copied().unwrap_or_default()
    }
}

impl ValueType {
    /// The `field-kind` code, which picks the operator dropdown's array.
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
    /// The `input-kind` code, which picks the value widget.
    pub fn as_index(self) -> i32 {
        match self {
            InputKind::None => 0,
            InputKind::Text => 1,
            InputKind::Number => 2,
        }
    }
}

impl RuleOp {
    /// Presence and boolean operators take none and relative-date ones a day count; the rest take
    /// the field's own kind.
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
    /// `None` where the operator takes no value or a number won't parse, the evaluator then
    /// skipping the rule as incomplete.
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
