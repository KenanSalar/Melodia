use std::collections::HashSet;

use crate::database::DbPool;
use crate::database::queries;
use crate::database::queries::fixtures::insert_test_track;
use melodia_core::entities::smart_criteria::{
    LimitOrder, MatchMode, Rule, RuleField, RuleOp, RuleValue, SmartCriteria, SmartLimit,
};
use melodia_core::entities::track::TrackListRow;
use melodia_core::error::AppError;

/// RFC-3339 timestamp `n` days before now — for staging `last_played`.
fn days_ago(n: i64) -> String {
    let delta = chrono::TimeDelta::try_days(n).unwrap_or_else(chrono::TimeDelta::zero);
    (chrono::Utc::now() - delta).to_rfc3339()
}

/// Stage playback stats the scan-insert helper leaves at defaults.
async fn set_stats(
    db: &DbPool,
    id: i64,
    rating: i32,
    play_count: i32,
    is_favorite: bool,
    last_played: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE tracks SET rating = ?, play_count = ?, is_favorite = ?, \
         last_played = ? WHERE id = ?",
    )
    .bind(rating)
    .bind(play_count)
    .bind(is_favorite)
    .bind(last_played)
    .bind(id)
    .execute(db.write())
    .await?;
    Ok(())
}

struct Seed {
    db: DbPool,
    t1: i64, // Rock, rating 5, plays 10, favorite, played 1 day ago
    t2: i64, // Rock, rating 2, plays 1,  not fav,   played 400 days ago
    t3: i64, // Pop,  rating 4, plays 5,  favorite,  never played
    t4: i64, // Jazz, rating 0, plays 0,  not fav,   never played
}

async fn seed() -> Result<Seed, AppError> {
    let db = DbPool::test_pool().await?;
    queries::folder::insert_folder(&db, "/music", true).await?;

    let t1 = insert_test_track(&db, "/music/1.mp3", "One", "Artist A", "Album A", "Rock").await?;
    let t2 = insert_test_track(&db, "/music/2.mp3", "Two", "Artist B", "Album B", "Rock").await?;
    let t3 = insert_test_track(&db, "/music/3.mp3", "Three", "Artist C", "Album C", "Pop").await?;
    let t4 = insert_test_track(&db, "/music/4.mp3", "Four", "Artist D", "Album D", "Jazz").await?;

    set_stats(&db, t1, 5, 10, true, Some(&days_ago(1))).await?;
    set_stats(&db, t2, 2, 1, false, Some(&days_ago(400))).await?;
    set_stats(&db, t3, 4, 5, true, None).await?;
    set_stats(&db, t4, 0, 0, false, None).await?;

    Ok(Seed { db, t1, t2, t3, t4 })
}

fn one(field: RuleField, op: RuleOp, value: Option<RuleValue>) -> SmartCriteria {
    SmartCriteria {
        rules: vec![Rule { field, op, value }],
        ..SmartCriteria::default()
    }
}

fn ids(rows: &[TrackListRow]) -> HashSet<i64> {
    rows.iter().map(|r| r.id).collect()
}

async fn resolve(db: &DbPool, c: &SmartCriteria) -> Result<Vec<TrackListRow>, AppError> {
    queries::smart_playlist::get_smart_playlist_tracks(db, c).await
}

#[tokio::test]
async fn genre_contains() -> Result<(), AppError> {
    let s = seed().await?;
    let c = one(RuleField::Genre, RuleOp::Contains, Some(RuleValue::Text("Rock".to_owned())));
    assert_eq!(ids(&resolve(&s.db, &c).await?), HashSet::from([s.t1, s.t2]));
    Ok(())
}

#[tokio::test]
async fn rating_gte() -> Result<(), AppError> {
    let s = seed().await?;
    let c = one(RuleField::Rating, RuleOp::Gte, Some(RuleValue::Number(4.0)));
    assert_eq!(ids(&resolve(&s.db, &c).await?), HashSet::from([s.t1, s.t3]));
    Ok(())
}

#[tokio::test]
async fn play_count_gt() -> Result<(), AppError> {
    let s = seed().await?;
    let c = one(RuleField::PlayCount, RuleOp::Gt, Some(RuleValue::Number(3.0)));
    assert_eq!(ids(&resolve(&s.db, &c).await?), HashSet::from([s.t1, s.t3]));
    Ok(())
}

#[tokio::test]
async fn favorite_is_true() -> Result<(), AppError> {
    let s = seed().await?;
    let c = one(RuleField::Favorite, RuleOp::IsTrue, None);
    assert_eq!(ids(&resolve(&s.db, &c).await?), HashSet::from([s.t1, s.t3]));
    Ok(())
}

#[tokio::test]
async fn last_played_never() -> Result<(), AppError> {
    let s = seed().await?;
    let c = one(RuleField::LastPlayed, RuleOp::IsNotSet, None);
    assert_eq!(ids(&resolve(&s.db, &c).await?), HashSet::from([s.t3, s.t4]));
    Ok(())
}

#[tokio::test]
async fn last_played_in_last_30_days() -> Result<(), AppError> {
    let s = seed().await?;
    let c = one(RuleField::LastPlayed, RuleOp::InLast, Some(RuleValue::Days(30)));
    // Only t1 (played 1 day ago); t2 is 400 days ago, t3/t4 never.
    assert_eq!(ids(&resolve(&s.db, &c).await?), HashSet::from([s.t1]));
    Ok(())
}

#[tokio::test]
async fn last_played_not_in_last_365_days() -> Result<(), AppError> {
    let s = seed().await?;
    let c = one(RuleField::LastPlayed, RuleOp::NotInLast, Some(RuleValue::Days(365)));
    // Old (t2) plus the never-played (t3, t4); t1 is recent → excluded.
    assert_eq!(ids(&resolve(&s.db, &c).await?), HashSet::from([s.t2, s.t3, s.t4]));
    Ok(())
}

#[tokio::test]
async fn match_all_is_intersection() -> Result<(), AppError> {
    let s = seed().await?;
    let c = SmartCriteria {
        match_mode: MatchMode::All,
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
        ],
        ..SmartCriteria::default()
    };
    // Rock AND rating>=4 → only t1 (t2 is Rock but rating 2).
    assert_eq!(ids(&resolve(&s.db, &c).await?), HashSet::from([s.t1]));
    Ok(())
}

#[tokio::test]
async fn match_any_is_union() -> Result<(), AppError> {
    let s = seed().await?;
    let c = SmartCriteria {
        match_mode: MatchMode::Any,
        rules: vec![
            Rule {
                field: RuleField::Genre,
                op: RuleOp::Is,
                value: Some(RuleValue::Text("Pop".to_owned())),
            },
            Rule {
                field: RuleField::Rating,
                op: RuleOp::Gte,
                value: Some(RuleValue::Number(4.0)),
            },
        ],
        ..SmartCriteria::default()
    };
    // Pop (t3) OR rating>=4 (t1, t3) → {t1, t3}.
    assert_eq!(ids(&resolve(&s.db, &c).await?), HashSet::from([s.t1, s.t3]));
    Ok(())
}

#[tokio::test]
async fn limit_caps_and_orders() -> Result<(), AppError> {
    let s = seed().await?;
    let c = SmartCriteria {
        rules: vec![Rule {
            field: RuleField::PlayCount,
            op: RuleOp::Gt,
            value: Some(RuleValue::Number(0.0)),
        }],
        limit: Some(SmartLimit {
            count: 2,
            order: LimitOrder::PlayCountDesc,
        }),
        ..SmartCriteria::default()
    };
    let rows = resolve(&s.db, &c).await?;
    // plays>0 → t1(10), t3(5), t2(1); top-2 by play_count desc → [t1, t3].
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, s.t1);
    assert_eq!(rows[1].id, s.t3);
    Ok(())
}

#[tokio::test]
async fn empty_rules_match_whole_library() -> Result<(), AppError> {
    let s = seed().await?;
    let rows = resolve(&s.db, &SmartCriteria::default()).await?;
    assert_eq!(rows.len(), 4);
    Ok(())
}

#[tokio::test]
async fn count_matches_membership() -> Result<(), AppError> {
    let s = seed().await?;
    let c = one(RuleField::Rating, RuleOp::Gte, Some(RuleValue::Number(4.0)));
    let (count, duration) = queries::smart_playlist::count_smart_playlist(&s.db, &c).await?;
    // t1 + t3, each 180_000 ms (helper default duration).
    assert_eq!(count, 2);
    assert_eq!(duration, 360_000);
    Ok(())
}

#[tokio::test]
async fn count_respects_limit() -> Result<(), AppError> {
    let s = seed().await?;
    let c = SmartCriteria {
        rules: vec![Rule {
            field: RuleField::PlayCount,
            op: RuleOp::Gt,
            value: Some(RuleValue::Number(0.0)),
        }],
        limit: Some(SmartLimit {
            count: 2,
            order: LimitOrder::PlayCountDesc,
        }),
        ..SmartCriteria::default()
    };
    let (count, _) = queries::smart_playlist::count_smart_playlist(&s.db, &c).await?;
    // 3 tracks match plays>0 but the limit caps the reported size at 2.
    assert_eq!(count, 2);
    Ok(())
}

#[tokio::test]
async fn count_random_limit_caps_without_ordering() -> Result<(), AppError> {
    let s = seed().await?;
    // A `Random` limit skips the pointless `ORDER BY RANDOM()` in the count
    // path. COUNT is still min(limit, matches); and since every seeded track
    // shares the helper's 180_000 ms duration, the SUM of any 2 rows is
    // deterministic regardless of which 2 the LIMIT keeps.
    let c = SmartCriteria {
        rules: Vec::new(),
        limit: Some(SmartLimit {
            count: 2,
            order: LimitOrder::Random,
        }),
        ..SmartCriteria::default()
    };
    let (count, duration) = queries::smart_playlist::count_smart_playlist(&s.db, &c).await?;
    assert_eq!(count, 2);
    assert_eq!(duration, 360_000);
    Ok(())
}

#[tokio::test]
async fn duration_rule_value_is_seconds() -> Result<(), AppError> {
    let s = seed().await?;
    // Give each track a distinct duration in ms. The editor presents Duration
    // in whole seconds ("Duration (sec)"), so a `Duration > 150` rule must
    // compare against 150_000 ms — not the raw 150 ms (which would match all).
    for (id, ms) in [
        (s.t1, 60_000_i64),
        (s.t2, 120_000),
        (s.t3, 240_000),
        (s.t4, 360_000),
    ] {
        sqlx::query("UPDATE tracks SET duration_ms = ? WHERE id = ?")
            .bind(ms)
            .bind(id)
            .execute(s.db.write())
            .await?;
    }
    let c = one(RuleField::DurationMs, RuleOp::Gt, Some(RuleValue::Number(150.0)));
    // 150 s == 150_000 ms → only t3 (240 s) and t4 (360 s) exceed it.
    assert_eq!(ids(&resolve(&s.db, &c).await?), HashSet::from([s.t3, s.t4]));
    Ok(())
}
