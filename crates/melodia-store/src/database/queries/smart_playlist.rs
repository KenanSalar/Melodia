//! Smart-playlist evaluation: a [`SmartCriteria`] rule set as a parameterized query over `tracks`.
//!
//! **Only enum-derived `&'static str` fragments are pushed as SQL** (the columns and rows from
//! [`field_source`], operator tokens, `ORDER BY`); every user value goes through
//! [`Sql::push_bind`].

use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};

use rusqlite::{Connection, Transaction};

use crate::database::DbPool;
use crate::database::queries::track::columns;
use crate::database::queries::track::file_rows::IN_LIBRARY;
use crate::database::sql::{Sql, escape_like};
use melodia_core::entities::smart_criteria::{
    Draw, LimitOrder, MatchMode, Rule, RuleField, RuleOp, RuleValue, SmartCriteria, SmartLimit,
    ValueType, ops_for,
};
use melodia_core::entities::track;
use melodia_core::error::AppError;

/// A smart playlist's membership as ids, in the criteria's order and under its cap, for the card
/// menu's batch actions. The detail page reads the same membership through its keyset
/// ([`push_positioned_membership`]), except under a random cap, where it keeps its own
/// [`pick_random`] and the two draw separately.
pub async fn get_smart_playlist_track_ids(
    db: &DbPool,
    criteria: &SmartCriteria,
) -> Result<Vec<i64>, AppError> {
    let sql = membership_query("id", criteria);
    db.read(move |conn| sql.fetch_all(conn)).await
}

/// [`get_smart_playlist_track_ids`] as M3U export rows, in the same order and under the same cap.
pub async fn get_smart_playlist_tracks_for_export(
    db: &DbPool,
    criteria: &SmartCriteria,
) -> Result<Vec<track::PlaylistExportRow>, AppError> {
    let sql = membership_query(columns::PLAYLIST_EXPORT.prefixed("tracks"), criteria);
    db.read(move |conn| sql.fetch_all(conn)).await
}

/// A smart playlist's membership as ids, in the order its detail page numbers them under `draw`,
/// read on `tx` so a write keeps exactly what it read.
pub(crate) fn member_ids_tx(
    tx: &Transaction<'_>,
    criteria: &SmartCriteria,
    draw: Draw,
) -> Result<Vec<i64>, AppError> {
    if let Some(count) = random_cap(criteria) {
        return pick_random(tx, criteria, count, draw);
    }
    membership_query("id", criteria).fetch_all(tx)
}

/// `SELECT columns FROM tracks` over the criteria's membership, ordered and capped.
fn membership_query(columns: &'static str, criteria: &SmartCriteria) -> Sql {
    let mut sql = Sql::new("SELECT ");
    sql.push(columns);
    push_membership(&mut sql, criteria);
    sql
}

/// The membership as `(id, position)` rows, `position` numbering the criteria's own order, for a
/// query that sorts or filters the set further and still needs that order to fall back on.
pub(crate) fn push_positioned_membership(sql: &mut Sql, criteria: &SmartCriteria) {
    sql.push("SELECT id, ROW_NUMBER() OVER (")
        .push(order_by_fragment(criteria.limit.as_ref()))
        .push(") AS position");
    push_membership(sql, criteria);
}

/// The membership's ids in no order, for a query that only counts or groups them: numbering an
/// uncapped set sorts every match. A cap keeps its order, that being what picks the ids it keeps.
pub(crate) fn push_member_ids(sql: &mut Sql, criteria: &SmartCriteria) {
    sql.push("SELECT id FROM tracks");
    push_where(sql, criteria, MemberTest::for_reading(criteria));
    if let Some(limit) = &criteria.limit {
        push_order_and_limit(sql, limit);
    }
}

/// The first cover an uncapped playlist's rules reach in its natural order, the one its automatic
/// thumbnail shows. Ordered as `idx_tracks_sort_key` is, so a plan can walk it and stop at the
/// first match carrying one, where numbering the set sorts every match.
pub(crate) fn first_cover(
    conn: &Connection,
    criteria: &SmartCriteria,
) -> Result<Option<String>, AppError> {
    let mut sql = Sql::new("SELECT artwork_path");
    push_first_cover(&mut sql, criteria);
    sql.fetch_optional(conn)
}

/// [`first_cover`]'s statement past its projection, which its plan test reads.
fn push_first_cover(sql: &mut Sql, criteria: &SmartCriteria) {
    sql.push(" FROM tracks WHERE artwork_path IS NOT NULL AND artwork_path != '' AND ")
        .push(IN_LIBRARY);
    push_and_rules(sql, criteria, MemberTest::Correlated);
    sql.push(order_by_fragment(None)).push(" LIMIT 1");
}

/// The count of a [`LimitOrder::Random`] cap, the cut [`pick_random`] makes rather than SQL.
pub(crate) fn random_cap(criteria: &SmartCriteria) -> Option<u32> {
    criteria
        .limit
        .as_ref()
        .filter(|limit| limit.order == LimitOrder::Random)
        .map(|limit| limit.count)
}

/// The `count` tracks `draw` picks from everything the rules match, in pick order, for a
/// [`LimitOrder::Random`] cap a page reads more than once and a convert keeps.
///
/// In Rust because `RANDOM()` takes no seed. Ranked by a hash of the draw and the id rather than
/// shuffled, so a track entering or leaving the matches moves only itself.
pub(crate) fn pick_random(
    conn: &Connection,
    criteria: &SmartCriteria,
    count: u32,
    draw: Draw,
) -> Result<Vec<i64>, AppError> {
    let mut sql = Sql::new("SELECT id FROM tracks");
    push_where(&mut sql, criteria, MemberTest::Materialized);
    let matches = sql.fetch_all(conn)?;
    Ok(pick(matches, count, draw))
}

/// The `count` of `matches` ranking lowest under `draw`, lowest first.
fn pick(matches: Vec<i64>, count: u32, draw: Draw) -> Vec<i64> {
    let hasher = BuildHasherDefault::<DefaultHasher>::default();
    let mut ranked: Vec<(u64, i64)> =
        matches.into_iter().map(|id| (hasher.hash_one((draw, id)), id)).collect();
    let count = usize::try_from(count).unwrap_or(usize::MAX);
    if count < ranked.len() {
        ranked.select_nth_unstable(count);
        ranked.truncate(count);
    }
    ranked.sort_unstable();
    ranked.into_iter().map(|(_, id)| id).collect()
}

/// `FROM tracks` plus the criteria's `WHERE`, order and cap.
fn push_membership(sql: &mut Sql, criteria: &SmartCriteria) {
    sql.push(" FROM tracks");
    push_where(sql, criteria, MemberTest::for_reading(criteria));
    if let Some(limit) = &criteria.limit {
        push_order_and_limit(sql, limit);
    } else {
        sql.push(order_by_fragment(None));
    }
}

/// `(track_count, total_duration_ms)` under the cap, for the grid card and the rule editor's live
/// count; a smart playlist has no `playlist_items` rows for the triggers to count.
pub async fn count_smart_playlist(
    db: &DbPool,
    criteria: &SmartCriteria,
) -> Result<(i64, i64), AppError> {
    let mut sql = if criteria.limit.is_some() {
        Sql::new(
            "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0) FROM \
             (SELECT duration_ms FROM tracks",
        )
    } else {
        Sql::new("SELECT COUNT(*), COALESCE(SUM(duration_ms), 0) FROM tracks")
    };
    push_where(&mut sql, criteria, MemberTest::for_reading(criteria));
    if let Some(limit) = &criteria.limit {
        // A deterministic order picks which rows are counted; a random one would sort the whole
        // match set to sum a slice that is random anyway.
        if limit.order != LimitOrder::Random {
            sql.push(order_by_fragment(Some(limit)));
        }
        sql.push(" LIMIT ").push_bind(i64::from(limit.count));
        sql.push(")");
    }
    db.read(move |conn| sql.fetch_one(conn)).await
}

/// Appends `WHERE` the library's tracks, then `AND ((rule) AND|OR (rule) …)`, skipping incoherent
/// rules; with none left it matches the whole library.
fn push_where(sql: &mut Sql, criteria: &SmartCriteria, test: MemberTest) {
    sql.push(" WHERE ").push(IN_LIBRARY);
    push_and_rules(sql, criteria, test);
}

/// The rules as ` AND (…)`, after a `WHERE` holding a condition of its own. Grouped whole, or an
/// `Any` set's `OR` would bind that condition to its first rule alone.
fn push_and_rules(sql: &mut Sql, criteria: &SmartCriteria, test: MemberTest) {
    let rules = renderable_rules(criteria);
    if rules.is_empty() {
        return;
    }
    sql.push(" AND (");
    push_rules(sql, &rules, criteria.match_mode, test);
    sql.push(")");
}

fn renderable_rules(criteria: &SmartCriteria) -> Vec<&Rule> {
    criteria.rules.iter().filter(|r| rule_is_renderable(r)).collect()
}

fn push_rules(sql: &mut Sql, rules: &[&Rule], match_mode: MatchMode, test: MemberTest) {
    let sep = match match_mode {
        MatchMode::All => " AND ",
        MatchMode::Any => " OR ",
    };
    for (i, rule) in rules.iter().enumerate() {
        if i > 0 {
            sql.push(sep);
        }
        sql.push("(");
        push_rule(sql, rule, test);
        sql.push(")");
    }
}

/// Whether a rule's operator is valid for its field's type, with a value of the matching kind
/// wherever the operator takes one.
fn rule_is_renderable(rule: &Rule) -> bool {
    if !ops_for(rule.field.value_type()).contains(&rule.op) {
        return false;
    }
    match rule.op {
        RuleOp::IsSet | RuleOp::IsNotSet | RuleOp::IsTrue | RuleOp::IsFalse => true,
        RuleOp::InLast | RuleOp::NotInLast => matches!(rule.value, Some(RuleValue::Days(_))),
        RuleOp::Contains
        | RuleOp::NotContains
        | RuleOp::Is
        | RuleOp::IsNot
        | RuleOp::StartsWith
        | RuleOp::EndsWith => matches!(rule.value, Some(RuleValue::Text(_))),
        RuleOp::Eq | RuleOp::Ne | RuleOp::Gt | RuleOp::Gte | RuleOp::Lt | RuleOp::Lte => {
            matches!(rule.value, Some(RuleValue::Number(_)))
        }
    }
}

/// One rule's predicate, which the caller parenthesizes.
fn push_rule(sql: &mut Sql, rule: &Rule, test: MemberTest) {
    let value = rule.value.as_ref();
    let col = match field_source(rule.field) {
        FieldSource::Members(rows) => return push_member_predicate(sql, rows, rule, test),
        FieldSource::Column(col) => col,
    };
    match rule.field.value_type() {
        ValueType::Text => push_text_predicate(sql, col, rule.op, value),
        ValueType::Number => push_numeric_predicate(sql, col, rule.field, rule.op, value),
        ValueType::Bool => push_bool_predicate(sql, col, rule.op),
        ValueType::Date => push_date_predicate(sql, col, rule.op, value),
    }
}

/// How a set-valued rule asks whether a track belongs.
#[derive(Clone, Copy)]
enum MemberTest {
    /// `IN` over the matching rows, each name tested once, for a query reading every match.
    Materialized,
    /// A correlated `EXISTS`, asked of one track at a time, so a plan walking an order index can
    /// stop at its cap or first hit, where an `IN` would find every match first.
    Correlated,
}

impl MemberTest {
    fn for_reading(criteria: &SmartCriteria) -> Self {
        if criteria.limit.is_some() { Self::Correlated } else { Self::Materialized }
    }
}

/// A text predicate over a set-valued field's *rows*, in the form `test` picks. The negation is
/// spelled once here, so a set matches when *any* member does.
fn push_member_predicate(sql: &mut Sql, rows: MemberRows, rule: &Rule, test: MemberTest) {
    let value = rule.value.as_ref();
    let (negated, op) = match rule.op {
        RuleOp::NotContains => (true, RuleOp::Contains),
        RuleOp::IsNot => (true, RuleOp::Is),
        RuleOp::IsNotSet => (true, RuleOp::IsSet),
        op => (false, op),
    };
    // Ahead of the wrapper, not inside it: a value-shape mismatch owes an always-false term, and
    // one placed under a negation is an always-true one.
    if op != RuleOp::IsSet && !matches!(value, Some(RuleValue::Text(_))) {
        return push_false(sql);
    }

    let name_clause = match test {
        MemberTest::Materialized => {
            // `track_id` is `NOT NULL`, so a `NOT IN` can't turn unknown over a NULL.
            sql.push(if negated { "tracks.id NOT IN (SELECT " } else { "tracks.id IN (SELECT " });
            sql.push(rows.track_id).push(" FROM ").push(rows.from);
            " WHERE ("
        }
        MemberTest::Correlated => {
            sql.push(if negated { "NOT EXISTS (SELECT 1 FROM " } else { "EXISTS (SELECT 1 FROM " });
            sql.push(rows.from).push(" WHERE ").push(rows.track_id).push(" = tracks.id");
            " AND ("
        }
    };
    // `IsSet` is the row existing, so what's already written is the whole predicate.
    if op != RuleOp::IsSet {
        // Parenthesized: two of `push_text_predicate`'s arms spell a bare `OR`, and one reaching
        // here would bind the correlation to its left half and leave the `EXISTS` uncorrelated.
        sql.push(name_clause);
        push_text_predicate(sql, rows.name, op, value);
        sql.push(")");
    }
    sql.push(")");
}

/// An always-false term, for a value shape [`rule_is_renderable`] should have refused: matching
/// nothing beats panicking on a user's library.
fn push_false(sql: &mut Sql) {
    sql.push("0");
}

/// The presence test every value category shares. A text column's empty string is "not set" too.
fn push_presence(sql: &mut Sql, col: &'static str, op: RuleOp, is_text: bool) {
    match op {
        RuleOp::IsSet => {
            sql.push(col).push(" IS NOT NULL");
            if is_text {
                sql.push(" AND ").push(col).push(" != ''");
            }
        }
        RuleOp::IsNotSet => {
            sql.push(col).push(" IS NULL");
            if is_text {
                sql.push(" OR ").push(col).push(" = ''");
            }
        }
        _ => push_false(sql),
    }
}

fn push_text_predicate(sql: &mut Sql, col: &'static str, op: RuleOp, value: Option<&RuleValue>) {
    match op {
        RuleOp::IsSet | RuleOp::IsNotSet => push_presence(sql, col, op, true),
        RuleOp::Contains | RuleOp::NotContains | RuleOp::StartsWith | RuleOp::EndsWith => {
            let Some(RuleValue::Text(s)) = value else {
                return push_false(sql);
            };
            let esc = escape_like(s);
            let pattern = match op {
                RuleOp::StartsWith => format!("{esc}%"),
                RuleOp::EndsWith => format!("%{esc}"),
                _ => format!("%{esc}%"),
            };
            if op == RuleOp::NotContains {
                sql.push(col).push(" IS NULL OR ").push(col).push(" NOT LIKE ");
            } else {
                sql.push(col).push(" LIKE ");
            }
            sql.push_bind(pattern).push(" ESCAPE '\\'");
        }
        RuleOp::Is => {
            let Some(RuleValue::Text(s)) = value else {
                return push_false(sql);
            };
            sql.push(col).push(" = ").push_bind(s.as_str()).push(" COLLATE NOCASE");
        }
        RuleOp::IsNot => {
            let Some(RuleValue::Text(s)) = value else {
                return push_false(sql);
            };
            sql.push(col)
                .push(" IS NULL OR ")
                .push(col)
                .push(" <> ")
                .push_bind(s.as_str())
                .push(" COLLATE NOCASE");
        }
        _ => push_false(sql),
    }
}

fn push_numeric_predicate(
    sql: &mut Sql,
    col: &'static str,
    field: RuleField,
    op: RuleOp,
    value: Option<&RuleValue>,
) {
    match op {
        RuleOp::IsSet | RuleOp::IsNotSet => push_presence(sql, col, op, false),
        RuleOp::Eq | RuleOp::Ne | RuleOp::Gt | RuleOp::Gte | RuleOp::Lt | RuleOp::Lte => {
            let Some(RuleValue::Number(n)) = value else {
                return push_false(sql);
            };
            let sql_op = match op {
                RuleOp::Eq => " = ",
                RuleOp::Ne => " <> ",
                RuleOp::Gt => " > ",
                RuleOp::Gte => " >= ",
                RuleOp::Lt => " < ",
                _ => " <= ",
            };
            // The editor takes Duration in whole seconds, and the column holds milliseconds.
            let bound = if field == RuleField::DurationMs { *n * 1000.0 } else { *n };
            sql.push(col).push(sql_op).push_bind(bound);
        }
        _ => push_false(sql),
    }
}

fn push_bool_predicate(sql: &mut Sql, col: &'static str, op: RuleOp) {
    match op {
        // Spelled as `idx_tracks_is_favorite`'s own `WHERE`: `= 1` doesn't reach a partial index.
        RuleOp::IsTrue => {
            sql.push(col).push(" = TRUE");
        }
        RuleOp::IsFalse => {
            sql.push(col).push(" = FALSE");
        }
        _ => push_false(sql),
    }
}

fn push_date_predicate(sql: &mut Sql, col: &'static str, op: RuleOp, value: Option<&RuleValue>) {
    match op {
        RuleOp::IsSet | RuleOp::IsNotSet => push_presence(sql, col, op, false),
        RuleOp::InLast | RuleOp::NotInLast => {
            let Some(RuleValue::Days(days)) = value else {
                return push_false(sql);
            };
            let cutoff = cutoff_rfc3339(*days);
            if op == RuleOp::InLast {
                sql.push(col).push(" IS NOT NULL AND ").push(col).push(" >= ");
            } else {
                sql.push(col).push(" IS NULL OR ").push(col).push(" < ");
            }
            sql.push_bind(cutoff);
        }
        _ => push_false(sql),
    }
}

/// Where a [`RuleField`]'s values live, one answer per field so nothing compares a genre against
/// the rendered line in one place and its rows in another.
enum FieldSource {
    /// A column on `tracks` holding the whole value.
    Column(&'static str),
    /// Rows in a join table, reached as a [`MemberTest`] says.
    Members(MemberRows),
}

/// The three fragments either [`MemberTest`] is built from, split so a fourth set of rows can't
/// leave out the tie to its track, which would match the whole library rather than fail.
#[derive(Clone, Copy)]
struct MemberRows {
    /// The `FROM` text: the join table and whatever it reaches for a name.
    from: &'static str,
    /// The column tying one of its rows to a track.
    track_id: &'static str,
    /// The name column a predicate compares.
    name: &'static str,
}

/// The column or rows each [`RuleField`] filters on.
///
/// **Genres and role credits are rows**: against the rendered `tracks.genre` / `.credits` line, `is`
/// can't match a track carrying two, `starts_with` reads only the first, and `contains` matches
/// across the seam. **`Artist` is a column**, the credit as printed being the string on screen a
/// rule is written against.
fn field_source(field: RuleField) -> FieldSource {
    match field {
        RuleField::Genre => FieldSource::Members(MemberRows {
            from: "track_genres tg JOIN genres g ON g.id = tg.genre_id",
            track_id: "tg.track_id",
            name: "g.name",
        }),
        RuleField::Credits => FieldSource::Members(MemberRows {
            from: "track_credits tc JOIN artists a ON a.id = tc.artist_id",
            track_id: "tc.track_id",
            name: "a.name",
        }),
        RuleField::Title => FieldSource::Column("title"),
        RuleField::Artist => FieldSource::Column("artist"),
        RuleField::AlbumArtist => FieldSource::Column("album_artist"),
        RuleField::Album => FieldSource::Column("album"),
        RuleField::Mood => FieldSource::Column("mood"),
        RuleField::InitialKey => FieldSource::Column("initial_key"),
        RuleField::Isrc => FieldSource::Column("isrc"),
        RuleField::Year => FieldSource::Column("year"),
        RuleField::Bpm => FieldSource::Column("bpm"),
        RuleField::DurationMs => FieldSource::Column("duration_ms"),
        RuleField::PlayCount => FieldSource::Column("play_count"),
        RuleField::SkipCount => FieldSource::Column("skip_count"),
        RuleField::Rating => FieldSource::Column("rating"),
        RuleField::Bitrate => FieldSource::Column("bitrate"),
        RuleField::SampleRate => FieldSource::Column("sample_rate"),
        RuleField::FileSize => FieldSource::Column("file_size"),
        RuleField::Favorite => FieldSource::Column("is_favorite"),
        RuleField::LastPlayed => FieldSource::Column("last_played"),
        RuleField::DateAdded => FieldSource::Column("date_added"),
    }
}

/// Appends `ORDER BY … LIMIT ?` for a capped playlist. The count subquery spells its own so it can
/// leave a random order out.
fn push_order_and_limit(sql: &mut Sql, limit: &SmartLimit) {
    sql.push(order_by_fragment(Some(limit)));
    sql.push(" LIMIT ").push_bind(i64::from(limit.count));
}

/// The `ORDER BY` clause, leading space included. RFC 3339 `last_played` and `date_added` sort
/// chronologically as text.
///
/// **Every arm ends in the id**, or a cap inside a run of ties (a scan stamps one `date_added` on
/// its batch, every unplayed track ties on play count) keeps whichever the plan reaches first, and
/// the plan moves with a rule's index or `PRAGMA optimize`'s statistics. Each direction is the one
/// the arm's unfiltered plan already breaks ties in, so flipping one, or borrowing
/// `MOST_PLAYED_ORDER`'s keys for `PlayCountDesc`, re-picks every playlist capped in that order.
fn order_by_fragment(limit: Option<&SmartLimit>) -> &'static str {
    match limit.map(|l| l.order) {
        Some(LimitOrder::DateAddedDesc) => " ORDER BY date_added DESC, id DESC",
        Some(LimitOrder::DateAddedAsc) => " ORDER BY date_added ASC, id ASC",
        Some(LimitOrder::PlayCountDesc) => " ORDER BY play_count DESC, id ASC",
        Some(LimitOrder::PlayCountAsc) => " ORDER BY play_count ASC, id ASC",
        Some(LimitOrder::LastPlayedDesc) => " ORDER BY last_played DESC, id ASC",
        Some(LimitOrder::LastPlayedAsc) => " ORDER BY last_played ASC, id ASC",
        Some(LimitOrder::RatingDesc) => " ORDER BY rating DESC, id DESC",
        Some(LimitOrder::Random) => " ORDER BY RANDOM()",
        None => " ORDER BY sort_key COLLATE NOCASE ASC, id ASC",
    }
}

/// The most days a relative-date rule reaches back, so `TimeDelta::try_days` can't overflow.
const MAX_RELATIVE_DAYS: i64 = 3_600_000;

/// The RFC 3339 cutoff `now - days` for the relative-date operators.
fn cutoff_rfc3339(days: i64) -> String {
    let days = days.clamp(0, MAX_RELATIVE_DAYS);
    let delta = chrono::TimeDelta::try_days(days).unwrap_or_else(chrono::TimeDelta::zero);
    (chrono::Utc::now() - delta).to_rfc3339()
}

#[cfg(test)]
#[path = "tests/smart_playlist_tests.rs"]
mod tests;
