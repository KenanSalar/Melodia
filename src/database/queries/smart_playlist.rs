//! Smart-playlist evaluation engine — turns a [`SmartCriteria`] rule set into a
//! parameterized query over the `tracks` table.
//!
//! **Safety contract:** only enum-derived `&'static str` fragments (the column
//! name from [`column_for`], the SQL operator token, the ORDER BY clause) are
//! ever pushed as raw SQL. Every user-supplied value goes through
//! [`sqlx::QueryBuilder::push_bind`] — the same "structure interpolated, data
//! bound" guarantee the rest of the query layer gets from `AssertSqlSafe(sql) +
//! .bind()`. No user string can reach the SQL text.

use sqlx::{QueryBuilder, Sqlite};

use crate::database::DbPool;
use crate::entities::smart_criteria::{
    LimitOrder, MatchMode, Rule, RuleField, RuleOp, RuleValue, SmartCriteria, SmartLimit, ValueType,
    ops_for,
};
use crate::entities::track;
use crate::error::AppError;

/// Resolve a smart playlist's current membership as list-view rows, in the
/// criteria's order (and capped by its optional limit).
pub async fn get_smart_playlist_tracks(
    db: &DbPool,
    criteria: &SmartCriteria,
) -> Result<Vec<track::TrackListRow>, AppError> {
    let cols = track::track_list_columns();
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(format!("SELECT {cols} FROM tracks"));
    push_where(&mut qb, criteria);
    qb.push(order_by_fragment(criteria.limit.as_ref()));
    if let Some(limit) = &criteria.limit {
        qb.push(" LIMIT ").push_bind(i64::from(limit.count));
    }
    let rows = qb
        .build_query_as::<track::TrackListRow>()
        .persistent(false)
        .fetch_all(db.read())
        .await?;
    Ok(rows)
}

/// `(track_count, total_duration_ms)` for a smart playlist without projecting
/// any track rows — backs the grid card and detail-header stats, which can't
/// rely on the `playlist_items` triggers (a smart playlist has no junction
/// rows). Respects the limit so a "top 25" playlist reports 25, not the full
/// match count.
pub async fn count_smart_playlist(
    db: &DbPool,
    criteria: &SmartCriteria,
) -> Result<(i64, i64), AppError> {
    let mut qb: QueryBuilder<Sqlite> = if criteria.limit.is_some() {
        QueryBuilder::new(
            "SELECT COUNT(*), COALESCE(SUM(duration_ms), 0) FROM \
             (SELECT duration_ms FROM tracks",
        )
    } else {
        QueryBuilder::new("SELECT COUNT(*), COALESCE(SUM(duration_ms), 0) FROM tracks")
    };
    push_where(&mut qb, criteria);
    if let Some(limit) = &criteria.limit {
        qb.push(order_by_fragment(Some(limit)));
        qb.push(" LIMIT ").push_bind(i64::from(limit.count));
        qb.push(")");
    }
    let row = qb
        .build_query_as::<(i64, i64)>()
        .persistent(false)
        .fetch_one(db.read())
        .await?;
    Ok(row)
}

/// Append the `WHERE (rule) <AND|OR> (rule) …` clause. Incoherent rules
/// (operator not valid for the field's type, or a missing/wrong-typed value)
/// are silently skipped; an all-skipped rule set yields no `WHERE` (matches the
/// whole library).
fn push_where(qb: &mut QueryBuilder<Sqlite>, criteria: &SmartCriteria) {
    let rules: Vec<&Rule> = criteria
        .rules
        .iter()
        .filter(|r| rule_is_renderable(r))
        .collect();
    if rules.is_empty() {
        return;
    }
    qb.push(" WHERE ");
    let sep = match criteria.match_mode {
        MatchMode::All => " AND ",
        MatchMode::Any => " OR ",
    };
    for (i, rule) in rules.iter().enumerate() {
        if i > 0 {
            qb.push(sep);
        }
        qb.push("(");
        push_rule(qb, rule);
        qb.push(")");
    }
}

/// Whether a rule is coherent enough to render: its operator must be valid for
/// the field's value type, and any value-taking operator must carry a value of
/// the matching kind. Guarantees the value extraction in [`push_rule`] succeeds.
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

/// Push a single rule's predicate. The caller wraps it in `(...)`. Assumes the
/// rule passed [`rule_is_renderable`]; the `else` fallbacks push a benign
/// always-false term rather than panicking if that invariant is ever broken.
fn push_rule(qb: &mut QueryBuilder<Sqlite>, rule: &Rule) {
    let col = column_for(rule.field);
    let vt = rule.field.value_type();
    match rule.op {
        RuleOp::IsTrue => {
            qb.push(col).push(" = 1");
        }
        RuleOp::IsFalse => {
            qb.push(col).push(" = 0");
        }
        RuleOp::IsSet => {
            qb.push(col).push(" IS NOT NULL");
            if vt == ValueType::Text {
                qb.push(" AND ").push(col).push(" != ''");
            }
        }
        RuleOp::IsNotSet => {
            qb.push(col).push(" IS NULL");
            if vt == ValueType::Text {
                qb.push(" OR ").push(col).push(" = ''");
            }
        }
        RuleOp::Contains | RuleOp::NotContains | RuleOp::StartsWith | RuleOp::EndsWith => {
            let Some(RuleValue::Text(s)) = &rule.value else {
                qb.push("0");
                return;
            };
            let esc = escape_like(s);
            let pattern = match rule.op {
                RuleOp::StartsWith => format!("{esc}%"),
                RuleOp::EndsWith => format!("%{esc}"),
                _ => format!("%{esc}%"),
            };
            if rule.op == RuleOp::NotContains {
                qb.push(col).push(" IS NULL OR ").push(col).push(" NOT LIKE ");
            } else {
                qb.push(col).push(" LIKE ");
            }
            qb.push_bind(pattern).push(" ESCAPE '\\'");
        }
        RuleOp::Is => {
            let Some(RuleValue::Text(s)) = &rule.value else {
                qb.push("0");
                return;
            };
            qb.push(col).push(" = ").push_bind(s.clone()).push(" COLLATE NOCASE");
        }
        RuleOp::IsNot => {
            let Some(RuleValue::Text(s)) = &rule.value else {
                qb.push("0");
                return;
            };
            qb.push(col)
                .push(" IS NULL OR ")
                .push(col)
                .push(" <> ")
                .push_bind(s.clone())
                .push(" COLLATE NOCASE");
        }
        RuleOp::Eq | RuleOp::Ne | RuleOp::Gt | RuleOp::Gte | RuleOp::Lt | RuleOp::Lte => {
            let Some(RuleValue::Number(n)) = &rule.value else {
                qb.push("0");
                return;
            };
            let sql_op = match rule.op {
                RuleOp::Eq => " = ",
                RuleOp::Ne => " <> ",
                RuleOp::Gt => " > ",
                RuleOp::Gte => " >= ",
                RuleOp::Lt => " < ",
                _ => " <= ",
            };
            // The editor presents Duration in whole seconds ("Duration (sec)"),
            // but the column stores milliseconds — scale the bound value so the
            // comparison is against the same unit the user typed.
            let bound = if rule.field == RuleField::DurationMs {
                *n * 1000.0
            } else {
                *n
            };
            qb.push(col).push(sql_op).push_bind(bound);
        }
        RuleOp::InLast | RuleOp::NotInLast => {
            let Some(RuleValue::Days(days)) = &rule.value else {
                qb.push("0");
                return;
            };
            let cutoff = cutoff_rfc3339(*days);
            if rule.op == RuleOp::InLast {
                qb.push(col).push(" IS NOT NULL AND ").push(col).push(" >= ");
            } else {
                qb.push(col).push(" IS NULL OR ").push(col).push(" < ");
            }
            qb.push_bind(cutoff);
        }
    }
}

/// The `tracks` column each [`RuleField`] filters on. Exhaustive `match` over an
/// enum, exactly like `track_list_order_by` — no user string reaches SQL.
fn column_for(field: RuleField) -> &'static str {
    match field {
        RuleField::Title => "title",
        RuleField::Artist => "artist",
        RuleField::AlbumArtist => "album_artist",
        RuleField::Album => "album",
        RuleField::Genre => "genre",
        RuleField::Composer => "composer",
        RuleField::Comment => "comment",
        RuleField::Label => "label",
        RuleField::Year => "year",
        RuleField::Bpm => "bpm",
        RuleField::DurationMs => "duration_ms",
        RuleField::PlayCount => "play_count",
        RuleField::SkipCount => "skip_count",
        RuleField::Rating => "rating",
        RuleField::Bitrate => "bitrate",
        RuleField::SampleRate => "sample_rate",
        RuleField::FileSize => "file_size",
        RuleField::Favorite => "is_favorite",
        RuleField::LastPlayed => "last_played",
        RuleField::DateAdded => "date_added",
    }
}

/// The `ORDER BY` clause (leading space included) for a resolved smart playlist.
/// A `&'static str` from an enum match — the selection order can't inject SQL.
/// Lexical `TEXT` order over RFC-3339 `last_played`/`date_added` == chronological
/// order (same invariant `get_recently_played` documents).
fn order_by_fragment(limit: Option<&SmartLimit>) -> &'static str {
    match limit.map(|l| l.order) {
        Some(LimitOrder::DateAddedDesc) => " ORDER BY date_added DESC",
        Some(LimitOrder::DateAddedAsc) => " ORDER BY date_added ASC",
        Some(LimitOrder::PlayCountDesc) => " ORDER BY play_count DESC",
        Some(LimitOrder::PlayCountAsc) => " ORDER BY play_count ASC",
        Some(LimitOrder::LastPlayedDesc) => " ORDER BY last_played DESC",
        Some(LimitOrder::LastPlayedAsc) => " ORDER BY last_played ASC",
        Some(LimitOrder::RatingDesc) => " ORDER BY rating DESC",
        Some(LimitOrder::Random) => " ORDER BY RANDOM()",
        None => " ORDER BY sort_key COLLATE NOCASE ASC",
    }
}

/// Escape a user substring for a `LIKE ? ESCAPE '\'` pattern — the wildcard
/// metacharacters `% _ \` are prefixed with a backslash.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// The RFC-3339 cutoff `now - days` for the relative-date operators. `days` is
/// clamped to a sane range so a hostile/absurd value can't overflow
/// `TimeDelta`; the `unwrap_or` fallback is dead after the clamp.
fn cutoff_rfc3339(days: i64) -> String {
    let days = days.clamp(0, 3_600_000);
    let delta = chrono::TimeDelta::try_days(days).unwrap_or_else(chrono::TimeDelta::zero);
    (chrono::Utc::now() - delta).to_rfc3339()
}

#[cfg(test)]
#[path = "tests/smart_playlist_tests.rs"]
mod tests;
