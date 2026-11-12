use rusqlite::Connection;

use super::super::membership::Membership;
use super::{Page, order_by};
use crate::database::DbPool;
use crate::database::sql::Sql;
use melodia_core::entities::track_list::{SortDir, TrackOrder, TrackSortField};
use melodia_core::error::AppError;
use melodia_core::utils::needle::Needle;

/// `EXPLAIN QUERY PLAN` for the unfiltered library's keyset in `order`, spelled as `track_ids`
/// spells it.
fn library_plan(conn: &Connection, order: TrackOrder) -> Result<Vec<String>, AppError> {
    let library = Membership::Library;
    let mut sql = Sql::new("EXPLAIN QUERY PLAN SELECT t.id FROM ");
    library.push_source(&mut sql);
    sql.push(" ORDER BY ").push_vouched(&order_by(
        Page::of(&library, &Needle::default()),
        order,
        library.position(),
    ));
    let steps: Vec<(i64, i64, i64, String)> = sql.fetch_all(conn)?;
    Ok(steps.into_iter().map(|(_, _, _, detail)| detail).collect())
}

/// **An index on an expression serves only that expression spelled the same**, so a respelled
/// order still sorts right and silently goes back to sorting the whole library on every open.
#[tokio::test]
async fn every_walked_songs_order_reads_its_index_without_a_sort() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;
    let walks = [
        (TrackSortField::Artist, "idx_tracks_artist_key"),
        (TrackSortField::Album, "idx_tracks_album_key"),
        (TrackSortField::Genre, "idx_tracks_genre_key"),
        (TrackSortField::Title, "idx_tracks_sort_key"),
        (TrackSortField::Year, "idx_tracks_year_order"),
        (TrackSortField::Length, "idx_tracks_length_order"),
    ];

    for (field, index) in walks {
        for dir in [SortDir::Asc, SortDir::Desc] {
            let order = TrackOrder { field, dir };
            let plan = db.read(move |conn| library_plan(conn, order)).await?;
            assert_eq!(
                plan,
                [
                    format!("SCAN t USING COVERING INDEX {index}"),
                    "LIST SUBQUERY 1".to_owned(),
                    "SCAN f".to_owned(),
                    "SEARCH outside USING COVERING INDEX idx_tracks_folder_id (folder_id=?)"
                        .to_owned(),
                    "CREATE BLOOM FILTER".to_owned(),
                ],
                "Songs by {field:?} {}, the outside rows read off the disabled folders",
                dir.as_str()
            );
        }
    }
    Ok(())
}
