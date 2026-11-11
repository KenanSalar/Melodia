use rusqlite::Connection;

use super::{Applied, ForeignKeys, MIGRATIONS, Migration, Pending, apply, compare};
use crate::database::{DbPool, sql};
use melodia_core::error::{AppError, MigrationError};

static KNOWN: &[Migration] = &[
    Migration {
        version: 1,
        description: "first",
        sql: "",
        checksum: b"one",
        foreign_keys: ForeignKeys::Enforced,
    },
    Migration {
        version: 2,
        description: "second",
        sql: "",
        checksum: b"two",
        foreign_keys: ForeignKeys::Enforced,
    },
    Migration {
        version: 3,
        description: "third",
        sql: "",
        checksum: b"three",
        foreign_keys: ForeignKeys::Enforced,
    },
];

fn ran(version: i64, checksum: &[u8]) -> Applied {
    Applied { version, success: true, checksum: checksum.to_vec() }
}

fn failed(version: i64, checksum: &[u8]) -> Applied {
    Applied { version, success: false, checksum: checksum.to_vec() }
}

/// What a boot would run, and the version its backup is named after.
fn plan(pending: &Pending) -> (Vec<i64>, i64) {
    let versions = pending.migrations.iter().map(|migration| migration.version).collect();
    (versions, pending.applied_version())
}

#[test]
fn an_empty_history_leaves_every_migration_pending() -> Result<(), MigrationError> {
    let pending = compare(KNOWN, &[])?;
    assert_eq!(plan(&pending), (vec![1, 2, 3], 0));
    Ok(())
}

#[test]
fn a_history_at_this_build_leaves_nothing_pending() -> Result<(), MigrationError> {
    let history = [ran(1, b"one"), ran(2, b"two"), ran(3, b"three")];
    let pending = compare(KNOWN, &history)?;
    assert_eq!(plan(&pending), (vec![], 3));
    Ok(())
}

#[test]
fn a_history_behind_this_build_leaves_only_the_newer_migrations_pending()
-> Result<(), MigrationError> {
    let history = [ran(1, b"one"), ran(2, b"two")];
    let pending = compare(KNOWN, &history)?;
    assert_eq!(plan(&pending), (vec![3], 2));
    Ok(())
}

/// The backup is named after the newest version applied, not after how many rows there are.
#[test]
fn a_version_missing_from_the_middle_of_the_history_is_still_pending() -> Result<(), MigrationError>
{
    let history = [ran(1, b"one"), ran(3, b"three")];
    let pending = compare(KNOWN, &history)?;
    assert_eq!(plan(&pending), (vec![2], 3));
    Ok(())
}

#[test]
fn a_migration_recorded_as_failed_refuses_the_history_as_dirty() {
    let history = [ran(1, b"one"), failed(2, b"two")];
    let refused = compare(KNOWN, &history);
    assert!(matches!(refused, Err(MigrationError::Dirty(2))), "{:?}", refused.err());
}

#[test]
fn a_version_this_build_lacks_refuses_the_history_as_unknown() {
    let history = [ran(1, b"one"), ran(2, b"two"), ran(3, b"three"), ran(4, b"four")];
    let refused = compare(KNOWN, &history);
    assert!(matches!(refused, Err(MigrationError::Unknown(4))), "{:?}", refused.err());
}

#[test]
fn a_migration_whose_file_changed_refuses_the_history_as_edited() {
    let history = [ran(1, b"one"), ran(2, b"changed")];
    let refused = compare(KNOWN, &history);
    assert!(matches!(refused, Err(MigrationError::Edited(2))), "{:?}", refused.err());
}

#[test]
fn a_failed_migration_is_refused_before_an_unknown_one() {
    let history = [ran(1, b"one"), failed(2, b"two"), ran(4, b"four")];
    let refused = compare(KNOWN, &history);
    assert!(matches!(refused, Err(MigrationError::Dirty(2))), "{:?}", refused.err());
}

#[test]
fn an_unknown_migration_is_refused_before_an_edited_one() {
    let history = [ran(1, b"changed"), ran(4, b"four")];
    let refused = compare(KNOWN, &history);
    assert!(matches!(refused, Err(MigrationError::Unknown(4))), "{:?}", refused.err());
}

static LANDS_THEN_FAILS: [Migration; 2] = [
    Migration {
        version: 1,
        description: "lands",
        sql: "CREATE TABLE landed (x)",
        checksum: b"1",
        foreign_keys: ForeignKeys::Enforced,
    },
    Migration {
        version: 2,
        description: "fails",
        sql: "CREATE TABLE half_made (x); INSERT INTO no_such_table VALUES (1)",
        checksum: b"2",
        foreign_keys: ForeignKeys::Enforced,
    },
];

fn apply_lands_then_fails(conn: &mut Connection) -> Result<(), AppError> {
    let pending = Pending { applied_version: 0, migrations: LANDS_THEN_FAILS.iter().collect() };
    apply(conn, pending)
}

#[test]
fn a_failing_migration_is_named_by_its_version() -> Result<(), AppError> {
    let mut conn = Connection::open_in_memory()?;

    let outcome = apply_lands_then_fails(&mut conn);

    assert!(
        matches!(outcome, Err(AppError::Migration(MigrationError::Apply { version: 2, .. }))),
        "{:?}",
        outcome.err()
    );
    Ok(())
}

/// The next boot retries from the version the backup captured, so a half-applied migration would
/// meet its own tables and fail again.
#[test]
fn a_failing_migration_rolls_back_with_its_history_row() -> Result<(), AppError> {
    let mut conn = Connection::open_in_memory()?;

    let _failed = apply_lands_then_fails(&mut conn);

    let tables: Vec<String> = sql::fetch_all(
        &conn,
        "SELECT name FROM sqlite_master WHERE name IN ('landed', 'half_made') ORDER BY name",
        (),
    )?;
    let history: Vec<i64> =
        sql::fetch_all(&conn, "SELECT version FROM _sqlx_migrations ORDER BY version", ())?;
    assert_eq!((tables, history), (vec!["landed".to_owned()], vec![1]));
    Ok(())
}

type HistoryRow = (i64, String, bool, Vec<u8>, bool);

/// **An older build reads this history through sqlx**, so each row is one sqlx would have written.
#[tokio::test]
async fn every_applied_migration_is_recorded_as_sqlx_records_it() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let history: Vec<HistoryRow> = db
        .read(|conn| {
            sql::fetch_all(
                conn,
                "SELECT version, description, success, checksum, execution_time >= 0 \
                 FROM _sqlx_migrations ORDER BY version",
                (),
            )
        })
        .await?;

    let embedded: Vec<HistoryRow> = MIGRATIONS
        .iter()
        .map(|m| (m.version, m.description.to_owned(), true, m.checksum.to_vec(), true))
        .collect();
    assert_eq!(history, embedded);
    Ok(())
}

/// sqlx-sqlite's DDL as `sqlite_master` keeps it, so a fresh database's schema reads byte for byte
/// as one sqlx created.
const SQLX_HISTORY_DDL: &str = "CREATE TABLE _sqlx_migrations (\n    \
    version BIGINT PRIMARY KEY,\n    \
    description TEXT NOT NULL,\n    \
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,\n    \
    success BOOLEAN NOT NULL,\n    \
    checksum BLOB NOT NULL,\n    \
    execution_time BIGINT NOT NULL\n\
)";

#[tokio::test]
async fn the_history_table_is_created_with_sqlx_s_own_ddl() -> Result<(), AppError> {
    let db = DbPool::test_pool().await?;

    let ddl: String = db
        .read(|conn| {
            sql::fetch_one(
                conn,
                "SELECT sql FROM sqlite_master WHERE name = '_sqlx_migrations'",
                (),
            )
        })
        .await?;

    assert_eq!(ddl, SQLX_HISTORY_DDL);
    Ok(())
}

const PARENT_AND_CHILD: &str = "\
    CREATE TABLE parent (id INTEGER PRIMARY KEY); \
    CREATE TABLE child (id INTEGER PRIMARY KEY, \
        parent_id INTEGER REFERENCES parent (id) ON DELETE CASCADE); \
    INSERT INTO parent VALUES (1); \
    INSERT INTO child VALUES (1, 1);";

/// A table other tables reference, rebuilt by `SQLite`'s documented recipe.
static REBUILDS_PARENT: Migration = Migration {
    version: 2,
    description: "rebuilds parent",
    sql: "CREATE TABLE parent_new (id INTEGER PRIMARY KEY); \
          INSERT INTO parent_new SELECT id FROM parent; \
          DROP TABLE parent; \
          ALTER TABLE parent_new RENAME TO parent",
    checksum: b"r",
    foreign_keys: ForeignKeys::Suspended,
};

static ORPHANS_THE_CHILD: Migration = Migration {
    version: 2,
    description: "orphans child",
    sql: "DELETE FROM parent",
    checksum: b"o",
    foreign_keys: ForeignKeys::Suspended,
};

/// A database holding a parent row and a child referencing it, enforcement on as the pool opens
/// every connection.
fn parent_and_child() -> Result<Connection, AppError> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(PARENT_AND_CHILD)?;
    Ok(conn)
}

fn apply_one(conn: &mut Connection, migration: &'static Migration) -> Result<(), AppError> {
    apply(conn, Pending { applied_version: 1, migrations: vec![migration] })
}

#[test]
fn a_suspended_rebuild_keeps_the_rows_referencing_the_rebuilt_table() -> Result<(), AppError> {
    let mut conn = parent_and_child()?;

    apply_one(&mut conn, &REBUILDS_PARENT)?;

    let children: i64 = sql::fetch_one(&conn, "SELECT COUNT(*) FROM child", ())?;
    assert_eq!(children, 1);
    Ok(())
}

#[test]
fn a_suspended_migration_leaving_a_reference_dangling_is_refused() -> Result<(), AppError> {
    let mut conn = parent_and_child()?;

    let outcome = apply_one(&mut conn, &ORPHANS_THE_CHILD);

    assert!(
        matches!(
            &outcome,
            Err(AppError::Migration(MigrationError::DanglingReference { version: 2, table, parent }))
                if table == "child" && parent == "parent"
        ),
        "{outcome:?}"
    );
    Ok(())
}

#[test]
fn a_refused_migration_rolls_back_whole() -> Result<(), AppError> {
    let mut conn = parent_and_child()?;

    let _refused = apply_one(&mut conn, &ORPHANS_THE_CHILD);

    let parents: i64 = sql::fetch_one(&conn, "SELECT COUNT(*) FROM parent", ())?;
    let recorded: i64 = sql::fetch_one(&conn, "SELECT COUNT(*) FROM _sqlx_migrations", ())?;
    assert_eq!((parents, recorded), (1, 0));
    Ok(())
}

#[test]
fn a_reference_dangling_before_the_migration_does_not_fail_it() -> Result<(), AppError> {
    let mut conn = parent_and_child()?;
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    conn.execute_batch("INSERT INTO child VALUES (2, 99)")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    apply_one(&mut conn, &REBUILDS_PARENT)?;

    let children: i64 = sql::fetch_one(&conn, "SELECT COUNT(*) FROM child", ())?;
    assert_eq!(children, 2);
    Ok(())
}

#[test]
fn foreign_keys_are_enforced_again_after_a_suspended_migration_lands_or_fails()
-> Result<(), AppError> {
    for migration in [&REBUILDS_PARENT, &ORPHANS_THE_CHILD] {
        let mut conn = parent_and_child()?;
        let _outcome = apply_one(&mut conn, migration);
        let enforced: bool = sql::fetch_one(&conn, "PRAGMA foreign_keys", ())?;
        assert!(enforced, "after {}", migration.description);
    }
    Ok(())
}

const CUE_TRACKS: i64 = 20_261_110_000_000;

fn migrate_where(conn: &mut Connection, applies: impl Fn(i64) -> bool) -> Result<(), AppError> {
    let migrations = MIGRATIONS.iter().filter(|migration| applies(migration.version)).collect();
    apply(conn, Pending { applied_version: 0, migrations })
}

/// A library before cue sheets: two tracks with play history, in a playlist, credited, and
/// dismissed as a duplicate pair.
const LIBRARY_BEFORE_CUE_SHEETS: &str = "\
    INSERT INTO folders (id, path, added_at) VALUES (1, '/music', 'then'); \
    INSERT INTO artists (id, name) VALUES (50, 'Artist'); \
    INSERT INTO genres (id, name) VALUES (60, 'Rock'); \
    INSERT INTO tracks (id, file_path, file_name, title, folder_id, date_added, play_count) \
        VALUES (5, '/music/a.flac', 'a.flac', 'A', 1, 'then', 7), \
               (9, '/music/b.flac', 'b.flac', 'B', 1, 'then', 3); \
    INSERT INTO playlists (id, name, created_at, updated_at) VALUES (1, 'Mix', 'then', 'then'); \
    INSERT INTO playlist_items (playlist_id, track_id, position, added_at) \
        VALUES (1, 9, 0, 'then'), (1, 5, 1, 'then'); \
    INSERT INTO track_artists (track_id, artist_id, position) VALUES (5, 50, 0), (9, 50, 0); \
    INSERT INTO track_credits (track_id, artist_id, role, position) VALUES (5, 50, 'composer', 0); \
    INSERT INTO track_genres (track_id, genre_id, position) VALUES (5, 60, 0); \
    INSERT INTO duplicate_dismissals (track_id, set_id) VALUES (5, 1), (9, 1);";

fn rebuilt_library() -> Result<Connection, AppError> {
    let mut conn = Connection::open_in_memory()?;
    migrate_where(&mut conn, |version| version < CUE_TRACKS)?;
    conn.execute_batch(LIBRARY_BEFORE_CUE_SHEETS)?;
    migrate_where(&mut conn, |version| version == CUE_TRACKS)?;
    Ok(conn)
}

#[test]
fn the_cue_rebuild_keeps_every_row_that_references_a_track() -> Result<(), AppError> {
    let conn = rebuilt_library()?;

    let counts: Vec<(String, i64)> = sql::fetch_all(
        &conn,
        "SELECT 'duplicate_dismissals', COUNT(*) FROM duplicate_dismissals \
         UNION ALL SELECT 'playlist_items', COUNT(*) FROM playlist_items \
         UNION ALL SELECT 'track_artists', COUNT(*) FROM track_artists \
         UNION ALL SELECT 'track_credits', COUNT(*) FROM track_credits \
         UNION ALL SELECT 'track_genres', COUNT(*) FROM track_genres",
        (),
    )?;

    let expected = [
        ("duplicate_dismissals", 2),
        ("playlist_items", 2),
        ("track_artists", 2),
        ("track_credits", 1),
        ("track_genres", 1),
    ];
    assert_eq!(counts, expected.map(|(table, count)| (table.to_owned(), count)));
    Ok(())
}

#[test]
fn the_cue_rebuild_keeps_each_track_s_id_and_history_as_a_whole_file() -> Result<(), AppError> {
    let conn = rebuilt_library()?;

    let tracks: Vec<(i64, String, i64, Option<i64>)> = sql::fetch_all(
        &conn,
        "SELECT id, file_path, play_count, cue_track FROM tracks ORDER BY id",
        (),
    )?;

    assert_eq!(
        tracks,
        [(5, "/music/a.flac".to_owned(), 7, None), (9, "/music/b.flac".to_owned(), 3, None)]
    );
    Ok(())
}

#[test]
fn the_cue_rebuild_keeps_the_playlist_s_order() -> Result<(), AppError> {
    let conn = rebuilt_library()?;

    let items: Vec<i64> =
        sql::fetch_all(&conn, "SELECT track_id FROM playlist_items ORDER BY position", ())?;

    assert_eq!(items, [9, 5]);
    Ok(())
}

#[test]
fn the_rebuilt_table_holds_one_whole_file_row_per_path_and_one_row_per_cue_track()
-> Result<(), AppError> {
    let conn = rebuilt_library()?;
    let insert = |values: &str| {
        conn.execute_batch(&format!(
            "INSERT INTO tracks (file_path, file_name, title, folder_id, date_added, cue_track, cue_start) \
             VALUES {values}"
        ))
    };

    let rows = [
        (
            "a second whole-file row",
            "('/music/a.flac', 'a.flac', 'A', 1, 'now', NULL, NULL)",
            false,
        ),
        ("a first cue track", "('/music/c.flac', 'c.flac', 'C', 1, 'now', 1, 0)", true),
        ("a second cue track", "('/music/c.flac', 'c.flac', 'C', 1, 'now', 2, 75)", true),
        ("the same number again", "('/music/c.flac', 'c.flac', 'C', 1, 'now', 2, 150)", false),
        ("a number past 99", "('/music/d.flac', 'd.flac', 'D', 1, 'now', 100, 0)", false),
        ("a number with no start", "('/music/e.flac', 'e.flac', 'E', 1, 'now', 1, NULL)", false),
    ];
    for (label, values, lands) in rows {
        assert_eq!(insert(values).is_ok(), lands, "{label}");
    }
    Ok(())
}
