# Plan: Move the library database from sqlx to rusqlite

Status: **Phase 4d complete** · Owner: Kenan · Created: 2026-11-04

> **Validated 2026-11-04** against `8fe1eb93` (`chore/deps-and-fixes`) with three read-only inventories:
>
> - every sqlx use in `melodia-store`;
> - every use outside it;
> - every doc, rule, CI and script line that names sqlx or its behaviour.
>
> Library internals were checked in their sources: sqlx 0.9.0 (`sqlx-sqlite`'s `connection/worker.rs` and `migrate.rs`, `sqlx-core`'s `migrate/`) and rusqlite 0.41.1 (features, `column_index`, the statement cache), plus libsqlite3-sys 0.30.1 and 0.39.1 (`build.rs` compile flags). Release data is from crates.io and GitHub on 2026-11-03. The benchmark in [Evidence](#evidence-2026-11-04) ran on 2026-11-04.
>
> **Re-validated 2026-11-04** against `76551945` (`main`), which `8fe1eb93` precedes with nothing under `database/`, core or `migrations/` changed between, by three fresh inventories (the store's counts and driver hazards; the pool, runtime and backup; everything outside the store with the docs). Sources added: `sqlx-sqlite`'s `arguments.rs`, rusqlite 0.41.1's `execute`, `execute_batch` and `pragma_update`, libsqlite3-sys 0.37.0's `build.rs`, tokio 1.54.1's blocking scheduler, rusqlite's 0.40 and 0.41 release notes and rusqlite 0.39.0's manifest on crates.io.

Working doc. Delete when every phase has shipped, Tests included.

## Context

**What you see today.**
- **The bundled SQLite is held back, and only a sqlx release can move it.** `Cargo.lock` resolves libsqlite3-sys 0.30.1, SQLite 3.46.0. sqlx 0.9.0 accepts up to libsqlite3-sys `<0.38` (SQLite 3.51.3), while libsqlite3-sys's current 0.39.1, which rusqlite 0.41 takes, bundles 3.54.1. sqlx shipped 0.9.0 on 2026-05-21; its main branch has 48 commits since (GitHub's compare, 2026-11-04), SQLite fixes among them, and no release.
- **Every statement and every row crosses a thread.** sqlx gives each connection its own OS thread. It sends each statement there as a command and copies each result row into an owned `SqliteRow` that comes back over a channel.
- **The store already works around that.** `fetch_packed_ids` folds a whole keyset into one `group_concat` string and parses it back, and its doc says why: "sqlx hands every row across from its connection's worker thread on its own, which over a library costs more than the sort."
- **The cost is measured, and it is CPU more than wait.** On Melodia's real schema, pool shape and runtime, with one SQLite build under both drivers, rusqlite was faster or level on every workload. It ran a mix of store calls in 1.45× less wall time and about half the CPU, with writes 2 to 3× faster and concurrent reads 2.9× ([Evidence](#evidence-2026-11-04)).
- **Lists and search are not where it shows.** The pager's keyset is one packed row, and FTS5 does its own work, so a Songs or Albums open and a search keystroke take the same time under either driver. The pager is what already made them cheap.

Users see nothing broken, and no list or search opens faster after the move. The gain is the CPU every store call spends crossing sqlx's worker thread, faster writes and concurrent reads, and less heap kept.

**Outcome.** The store talks to SQLite through rusqlite: a small async pool of its own over synchronous connections, a migrator of its own over the same history table, and the same SQL.

- **Unchanged:**
  - every query's result, sort order and filter;
  - FTS5 search and `UNICODE_LOWER` ordering;
  - the database file, which opens as-is;
  - the migration history, which stays in `_sqlx_migrations` in sqlx's exact row format, so an older build still refuses a database a newer one migrated, as it does today;
  - the backup before a pending migration;
  - the one-writer, two-to-four-readers layout;
  - idle connections closing after `IDLE_TIMEOUT`, so idle memory and `heap_trim`'s schedule hold;
  - each role's pragmas, as `init_database` and `test_pool` set them;
  - every public store function staying `async` and returning what it returns today. A parameter its closure keeps becomes owned (Cross-cutting), and the two visitors become folds (Phase 3.6);
  - `DbPool::test_pool()`'s signature, which its 282 call sites name.
- **Removed:**
  - sqlx and its per-connection worker threads;
  - the sqlx pool defaults the code never set: the 30-minute `max_lifetime`, the 30 s `acquire_timeout` (a caller waits for its lane instead, every closure being bounded work), the ping before an idle connection is reused, and one connection opened per pool at creation (the writer opens at init for the migrator, readers on first use);
  - `QueryBuilder` and the `TrackBinder` trait's two impls;
  - the executor-generic functions;
  - the `chunked_in_query` / `chunked_in_query_on` split;
  - `&mut **tx` reborrows;
  - the `FromRow` derives in `melodia-core`;
  - sqlx test dev-dependencies in `melodia-app` and `melodia`;
  - `sha2`, the 14 sqlx-applied checksums pinned in its place (Phase 3.5).
- **Added:**
  - `database/pool/`, `database/migrate.rs` with a store `build.rs`, `database/sql.rs` and `database/rows/`;
  - the dependency `rusqlite`, and `blake3` as a store build-dependency;
  - `AppError::DatabaseClosed` (Decision 6).

## Decisions settled here

1. **rusqlite, not another driver.** The survey (chat, 2026-11-03) covered:
   - **Diesel:** a DSL rewrite of hand-written SQL.
   - **SeaORM:** its async side runs on sqlx 0.9.
   - **Turso:** no FTS5 and no custom collations, and it silently treats an unknown collation as the default.
   - **libsql:** its owner's README names the Rust rewrite as the direction.
   - **Toasty:** pre-release churn.

   rusqlite's project maintains libsqlite3-sys, so new SQLite versions land there first. It has shipped five releases since May.
2. **No async wrapper or pool crate.** libsqlite3-sys declares `links = "sqlite3"`, so every crate in the graph must agree on one rusqlite minor version. Of the wrappers:
   - tokio-rusqlite 0.8.0, deadpool-sqlite 0.14.0, r2d2_sqlite 0.35.0, rusqlite_migration 2.6.0 and refinery 0.10.0 are all on 0.40;
   - sea-query-rusqlite 0.8.0 is on 0.38;
   - only async-sqlite 0.7.0 follows 0.41.

   A wrapper brings back the wait for someone else's release. The pool here is small and the store's own (Structure).
3. **Our own migrator, keeping `_sqlx_migrations` in sqlx's format.** Same DDL, the same rows (`execution_time` -1 inside the migration's transaction, the real time after commit) and the same refusals:
   - a dirty version;
   - an applied file whose checksum changed;
   - an applied version this build doesn't know.

   That needs no bridging step on first launch. It keeps "a shipped migration is never edited" enforced, and it leaves downgrade behaviour exactly as it is. rusqlite_migration tracks state with `user_version` and keeps no checksums; refinery keeps its own table. Both would need a one-time bridge, and both lag rusqlite releases.

   - The checksums are computed in `build.rs`, as `sqlx::migrate!` computes them at compile time, so the binary hashes nothing at boot. The 14 migrations sqlx applied keep the SHA-384 their rows record, pinned beside the blake3 of each file as shipped, and every later one records blake3 (Phase 3.5). An older release refuses a version it doesn't know before it compares any checksum, so it never reads a blake3 row.
   - Each file runs through `execute_batch` inside its transaction, on the writer connection, so the pragmas the frozen migrations ran under (`foreign_keys` among them) stay in force.
4. **Row mapping lives in the store, by column name.**
   - `melodia-core`'s entities become plain data. The store's own `FromRow` trait maps them by name, as sqlx's derive did, so `queries::track::columns`' "the order carries nothing" stays true.
   - Tuples map by index, as sqlx's did.
   - rusqlite's by-name `get` scans the statement's columns on each call. That's cheap at these widths (a page of `TrackListRow`s, a grid of cards). Phase 5 measures the wide projections before anything moves to by-index.
   - `DismissalSet`'s field is private, so its `FromSql` lives in core beside it, as its `sqlx::Type` derive does today. It binds as a `Param` through core's `From<DismissalSet> for i64` (Phase 3).
5. **One builder for dynamic SQL, and caching follows the SQL's type.**
   - A `&'static str` statement goes through `prepare_cached`, and so does an `Sql` still holding only the `&'static str` it was made from (`update_track_metadata`, `insert_track`): `Sql` keeps its text as a `Cow<'static, str>`.
   - An `Sql` with anything pushed onto it goes through `prepare`.
   - Text fixed for the whole run is interned in a `LazyLock<String>` and runs as a `&'static str`, cached as sqlx kept it.
   - That reproduces sqlx's split, where 31 `.persistent(false)` sites were the assembled ones, except for five statements spliced from a few fixed variants that sqlx cached: `search_name::refill`'s two, the radio station insert, `ranked_totals` and the backfill page select. Each runs per name write, save, tab open or launch, so they are assembled and uncached.
   - `Sql::trusted` takes runtime-built structure (placeholder lists, column projections) and replaces `AssertSqlSafe` as the deliberate marker.
   - rusqlite refuses three things sqlx let through, each checked against the tree:
     - **A second statement in one string** (`MultipleStatement`). Only the migrations hold one (Decision 3). `stats.rs`'s trigger DDL runs one statement per call, and rusqlite allows a trailing tail with no statement in it.
     - **A row from `execute`** (`ExecuteReturnedResults`). `journal_mode` and `mmap_size` answer with a row, so `pool/open.rs` sets pragmas through `pragma_update`, which runs `execute_batch`. All 19 `RETURNING` sites already read through `fetch_*`.
     - **A bind count that differs from the statement's** (`InvalidParameterCount`). sqlx binds a missing parameter as NULL and ignores extras (`sqlx-sqlite`'s `arguments.rs:100-107`). An audit of 352 literal-SQL chains found no mismatch, and the one `?1` used twice (`renumber_playlist_positions_tx`) binds once in both drivers.
   - rusqlite counts changed rows as `usize` where sqlx gave `u64`. The 12 `rows_affected` sites convert, `Sql::execute`'s among them, so the public return types stay.
6. **Each store function runs in one closure on the runtime's blocking pool.**
   - A store function stays an `async fn`. Its body awaits a lane: a semaphore permit (async, so a burst never fills the 32-thread blocking pool), then `Handle::spawn_blocking` with the connection.
   - The handle is captured at init, so a call awaited on the Slint thread never runs SQLite there. `main.rs` enters the runtime on that thread, but the pool doesn't lean on it.
   - A write transaction is synchronous inside its closure, so nothing can `.await` inside one.
   - **The permit and the connection travel together.** Both move into the blocking closure and go back to the lane only when it ends. A dropped future then never frees a permit while its connection still works, so the writer lane can't open a second writer.
   - **After `close()`, `read` and `write` fail at once**, as sqlx's `PoolClosed` does. `play_count_flusher_tests.rs:151` stages its failed write that way.
   - **That failure, and a blocking task cancelled at runtime shutdown, return `AppError::DatabaseClosed`.** A `SQLITE_ABORT` made up for it would carry no real cause, where failures are told apart by variant. `melodia-views`' `radio/callbacks/kept.rs:199` takes it beside `Database`, or its `_` arm reports "not audio".
7. **The cutover lands as one PR in four commits, after the phases that shrink it.**
   - A half-converted store can't run: a transaction can't span drivers, and an in-memory test database can't be shared between them. So the swap is validated only once it is whole.
   - It lands as sub-phases 4a to 4d, a commit each, and the workspace builds again only at 4d's. `main` takes PRs as merge commits, so its first-parent history stays buildable, and `git bisect --first-parent` never visits 4a to 4c.
   - Phases 1 to 3 and 3.6 take out of it everything that can land and be validated under sqlx: the cross-crate tests, the migrator, the dynamic SQL and the visitors' fold shape.
   - Running both drivers side by side was rejected. It needs a file-backed test pool, two error variants and two collation registrations, and it brings `SQLITE_BUSY` between two writers into the hand tests. All of that is machinery built to be deleted.
8. **The cutover keeps the engine Phase 0 lands, and Phase 5 moves it.**
   - The cutover pins rusqlite 0.39.x, whose libsqlite3-sys 0.37 is the 3.51.3 Phase 0 resolves. Phase 5's before/after measurement then isolates the driver.
   - The bump to the latest rusqlite (0.41.x, SQLite 3.54) follows as its own step. What it moves:
     - libsqlite3-sys 0.39 drops `-DSQLITE_ENABLE_MEMORY_MANAGEMENT` from the bundled build (rusqlite #1900; `build.rs` diffed between 0.37.0 and 0.39.1). The flag only gives `sqlite3_release_memory` and the soft heap limit something to free, and the tree calls neither.
     - In 0.41, `Statement::column_names` and `columns` return iterators, and `pragma_update` checks the name against `pragma_list`. Its schema is already `Option<&str>` in 0.39.0 (`pragma.rs:224`).
     - The soundness fixes between (vtab, `array`'s `Rc` values, savepoint names, `trace_v2` and `busy_handler`) touch nothing this store calls, so 0.39 is safe for the one step.

## Structure (who owns what afterwards)

Everything is in `crates/melodia-store/src/database/` unless noted.

| File | Owns | Single reason to change |
|---|---|---|
| `pool/mod.rs` | `DbPool` (cheap `Clone` over an `Arc`), `read(work)` / `write(work)` / `transaction(work)` (`pub(crate)`, the last committing on `Ok`), `close()`, `test_pool()`, `IDLE_CONNECTIONS_CLOSED_WITHIN` | How the store hands out connections |
| `pool/lane.rs` | One lane: semaphore, idle stack with last-used times, run-on-blocking-pool (permit and connection moving into the closure together), the reaper (ticking every `IDLE_TIMEOUT` from the lane's creation, as sqlx's does and `heap_trim` assumes, and closing on the blocking pool, since the last close checkpoints the WAL) | Concurrency and idle-close semantics |
| `pool/open.rs` | Opening a connection per role (writer, reader, in-memory test): flags, pragmas as `init_database` sets them (writer: WAL, `foreign_keys`, `synchronous=NORMAL`, `cache_size`, `temp_store`, `mmap_size`; readers the same without `synchronous`; test: `foreign_keys`, `synchronous`, `temp_store`), `busy_timeout`, collation, statement-cache capacity | Connection configuration |
| `migrate.rs` | Comparing checksums, validation against `_sqlx_migrations`, applying pending migrations, the pending check the backup asks | Migration semantics |
| `crates/melodia-store/build.rs` | Embedding the workspace root's `migrations/*.sql` (`../../migrations`, the path `sqlx::migrate!` reads) in version order with their checksums (the 14 sqlx applied pinned, blake3 after), `rerun-if-changed` on that directory, the sqlx-style description, refusing a shipped file changed, renamed or missing, anything there but `<version>_<description>.sql` files (dotfiles skipped), a `-- no-transaction` header, `.up`/`.down` names, a version below 1 and a duplicate version | The migrations directory's shape |
| `sql.rs` | `Sql` (text plus `Param`s), `push` / `push_trusted` / `push_bind` / `bind`, and the execution vocabulary (`fetch_all`, `fetch_optional`, `fetch_one`, `execute`): free functions over a `&'static str` and rusqlite params, plus `fetch_each` for the two folds, and `Sql`'s methods for built text | How SQL is assembled and run |
| `rows/mod.rs` | The store's `FromRow` trait, its lone-column and tuple impls, and `by_name!` | Row decoding |
| `rows/{track,library,radio,duplicates}.rs` | Entity impls, grouped as `melodia-core/src/entities/` groups them | An entity's columns |
| `mod.rs` | Bind budget, placeholder text (`placeholders`, `values_placeholders`, `values_list`, `case_by_id`) and chunk helpers (`chunked_in_query`, `chunked_in_execute_on`, `select_in_id_order`, `set_where_id_in`, `backfill_folded`), `init_database` | Bulk-statement shape |
| `collation.rs`, `backup.rs` | Unchanged roles, on rusqlite | |
| `melodia-core/src/error.rs` | `Database(#[from] rusqlite::Error)`, `Migration(#[from] MigrationError)` (four variants: `Dirty`, `Edited`, `Unknown`, `Apply { version, source }`), `DatabaseClosed` | |

**Duplication collapsed on the way through:**
- `chunked_in_query` and `chunked_in_query_on` become one function on `&Connection`, because a `Transaction` derefs to one. `chunked_in_execute_on` keeps `&Transaction`, its contract being a write that lands whole.
- The 8 executor-generic functions and the 9 that take a `&mut SqliteConnection` become plain `&Connection` functions.
- `TrackBinder`'s two impls (over sqlx's `Query` and `Separated`) become one: `bind_track_columns` pushes into `Sql`.
- The 73 functions taking a transaction stop reborrowing (`&mut **tx`) and stop being `async`.

**Line budget.**
- `mod.rs` (480) shrinks as the pool and the migrator move out.
- The files nearest the 800 cap are `queries/playlist.rs` (663), `queries/ingest.rs` (580), `queries/smart_playlist.rs` (553), `queries/radio.rs` (459) and `queries/scan/mutations.rs` (449). Each is counted at the end of the phase that grows it.

## Every phase

- **Gates:** `cargo fmt --all`, `cargo clippy --all-targets --locked --workspace -- -D warnings`, `cargo test --locked --workspace`.
- **Doc files:** each phase updates the doc files it makes stale, held to a fact at its anchor; no phase is "docs".
- **Existing tests:** a test the phase breaks is brought to the new truth in that phase. New tests wait for the Tests phase.
- **Hand tests** run in the dev data folder (`~/.local/share/Melodia-dev`), backed up first.

## Phase 0: The newest SQLite sqlx accepts ✅

- `cargo update -p libsqlite3-sys`: 0.30.1 to 0.37.0, SQLite 3.46.0 to 3.51.3. The dry run confirms this is the whole change.
- Delete the stale `.gitignore` block (lines 28-31), which says `.sqlx/` is committed. No `.sqlx/` exists, and no query macro reads one.
- **Hand test:** boot the dev library, search, rescan a folder, quit. After quit, note whether `melodia.db-wal` is left behind.
  - Today's `close()` shuts the writer before the readers (`mod.rs:288-297`), so the writer is not the last connection, and a read-only last connection can't checkpoint.
  - **Result (2026-11-04):** `melodia.db-wal` survives quit today, so Phase 4's folded WAL is new behaviour.
- **Docs:** none. `sqlx.md`'s "`libsqlite3-sys` is version-ranged" stays true.

## Phase 1: Store-owned test seams; the pools go crate-private ✅

The 56 call sites outside the store that name sqlx (58 statements, `cross_tier.rs`'s `id_of` running three) are all test seeding and readback:
- 15 test files under `crates/melodia-app/src/`;
- `crates/melodia/tests/cross_tier.rs`;
- `crates/melodia/tests/headless.rs`.

No production code outside the store calls `db.read()` or `db.write()`.

- Add a driver-neutral seam in `queries::fixtures` (`#[doc(hidden)] pub`, as the rest of `fixtures` is):
  - `execute` for literal SQL, on the writer;
  - `fetch_one`, `fetch_optional` and `fetch_all` on a reader, decoding through the store-owned `Readback` trait. It has concrete impls for the column types the tests read (`i64`, `Option<i64>`, `String`, `Option<String>`, `bool`) and generic impls for tuples of 2 to 5;
  - test values written inline as literals, text through `quoted`, which doubles `'`. Five of the six text values are tempdir paths, which on Windows sit under the user profile.
- Port the statements to it.
  - `play_count_flusher_tests.rs:151` closes `db.write()` directly. It moves to `DbPool::close()`, as `artist_images_tests` already does. The test stages a failed write, and on a `test_pool` closing either pool closes both.
- `DbPool::read()` / `write()` become `pub(crate)`.
- Remove the `sqlx` dev-dependency from `melodia-app` and `melodia`, with their comments. `melodia`'s names `pragma_table_info`, which nothing reads.
- **Hand test:** none; the change is test-only, and the suite is its proof.
- **Docs:** none.

## Phase 2: Our own migrator, history unchanged ✅

- `build.rs` embeds the directory, and `migrate.rs` replaces `sqlx::migrate!` and the `Migrate` trait calls in `init_database` and `test_pool` (Decision 3). Its SQL still runs through sqlx until Phase 4.
- Add `sha2` 0.11.1 as a store build-dependency. `sha2` 0.10.9 stays until Phase 4: sqlx 0.9.0's manifest turns on `sqlx-core`'s `migrate` whatever features the workspace names. Dropping `macros` takes `sqlx-mysql`, `sqlx-postgres` and their crypto dependencies out of `Cargo.lock`.
- `AppError::Migration` holds `MigrationError`. `Apply`'s source is `sqlx::Error` until the cutover.
- Drop sqlx's `migrate` and `macros` features, and delete the comment block above the workspace dependency, whose only subject was `macros`.
- **Hand test:**
  1. **The dev library boots with nothing pending and nothing mismatched.** That proves the checksums equal sqlx's on a real history.
  2. **A fresh data folder applies all 14.** Its `_sqlx_migrations` rows match a sqlx-applied database's in version, description, success and checksum, compared with `/usr/bin/sqlite3`; `installed_on` and `execution_time` differ by nature.
  3. **A database whose history holds a version this build lacks refuses to boot,** as today.
  - **Result (2026-11-05):** all three pass.
    1. History and `backups/` are unchanged across a boot and a tray quit.
    2. The fresh history and schema (`sqlite_master`) equal the sqlx-applied library's, every `execution_time` is positive, and no backup is taken.
    3. Boot exits 1 with `Migration(Unknown(…))` before any backup.
- **Docs:**
  - `CLAUDE.md`'s "sqlx checksums the file" and `code-style.md`'s "sqlx checksums the whole file" now name the migrator.
  - `sqlx.md`'s `Migrate`-trait and `sqlx.toml` bullets go, with a pointer to `migrate.rs`.
  - The `//!` headers of `melodia-app`'s `tasks/artwork_renormalize.rs:5`, `rating_import.rs:5` and `tag_backfill.rs:8` say "Not an `SQLx` migration", which becomes "Not a migration".

## Phase 3: One builder for dynamic SQL ✅

- Add `database/sql.rs` (Decision 5). Under sqlx, its execution binds each `Param` and runs assembled text `.persistent(false)`.
- Convert to it:
  - the 15 `QueryBuilder` constructions;
  - the helpers that take `&mut QueryBuilder<Sqlite>`: `push_needle`, `push_folded_match`, `sql_direction`, `detail_order`, `Membership::push_*`, `push_member_ids` and `push_positioned_membership`;
  - `fetch_packed_ids`;
  - `set_where_id_in`, which returns `Sql`;
  - the chunk helpers' SQL;
  - `backfill_folded`'s page update;
  - the three `push_values` sites;
  - all 39 `AssertSqlSafe` sites, the fixed-text ones as `LazyLock` statics (Decision 5).
- These counts are production sites. `fixtures`' four seam calls, which Phase 1 added after the count, keep `AssertSqlSafe`: they decode raw rows through `Readback`, which Phase 4 folds into `rows/` (steps 3 and 11). `smart_playlist_tests`' one `QueryBuilder` moves here with `push_rule`; the 11 `AssertSqlSafe` calls in store tests move with the tests in Phase 4.
- The backup's `VACUUM INTO` binds its path (`VACUUM INTO ?1`, run under `/usr/bin/sqlite3` 3.51.2 on 2026-11-04) instead of interpolating it, which drops the quote-doubling and the lossy `display()`. `backup_tests` proves it, file-backed.
- Collapse `TrackBinder` into `bind_track_columns` over `Sql`. `insert_track` and the batch share one row tuple, the single insert's defaults becoming the batch's literals, and read their ids through `RETURNING`.
- **Hand test:**
  - every smart-playlist rule kind, a capped and a random one;
  - every Songs and detail sort and filter;
  - the seven grids' keysets;
  - search;
  - a first scan and a rescan (the bulk-insert path);
  - Keep Out and its undo;
  - Not Duplicates and its restore;
  - adding, removing and reordering playlist items;
  - a bulk favourite and rating;
  - saving a radio station;
  - Most Played and Recently Played;
  - a Browse folder;
  - adding a folder that holds a library folder, which absorbs it once its scan completes.
  - **Result (2026-11-05):** all pass, run unattended on Kenan's go.
    1. A harness made 7,804 store calls on a snapshot of the dev library, under `1c9263b4` and under this phase: every track-list scope, order and filter, the grids, every smart-rule field and operator with limits, the lookups and search, then a fixed write sequence (favourites, ratings, plays, playlist edits and convert, Not Duplicates, a duplicate removal, stations, artwork, backfills, an import, a watcher batch, folder absorb). Every result matched, and so did both databases afterwards, row for row.
    2. On the dev library, a launch scanned a new 26-file folder through the bulk insert, and watcher create, rename, retag and delete, a command-line import and repeated plays each left the rows they should. Nothing logged an error or warning, and the tray quit was clean.
    3. No clicks were driven, the view code being unchanged: the UI reaches these paths through the same store calls.
- **Docs:** `sqlx.md`'s `AssertSqlSafe`, `QueryBuilder::push_values` and `.persistent(false)` bullets name the builder, and its `VACUUM INTO` "Takes no bind parameter" goes.

## Phase 3.5: blake3 for the migration checksums ✅

The store's `build.rs` is the last user of `sha2` 0.11, kept for the SHA-384 sqlx records. The 14 rows sqlx wrote keep that SHA-384, so an older build still reads them; every later migration records blake3.

- `build.rs` pins the 14 migrations sqlx applied, keyed by file name: the SHA-384 each history row records and the blake3 of the file as shipped, both in the lowercase hex `sha384sum` and `b3sum` print.
  - A pinned file must hash to its pinned blake3, and its checksum is then its pinned SHA-384. A pinned name with no file fails too, so an edited, renamed or deleted shipped migration stops the build, not each boot of a database that applied it.
  - Any other file records the blake3 of its bytes. It keeps today's boot-time `Edited` check; the table never grows.
  - Keyed by name, the table matches `sha384sum migrations/*.sql` line for line and writes no 14-digit version, which `unreadable_literal` would refuse in `build.rs`.
  - The pinned table's doc carries why the split is safe.
- `migrate.rs`: `Migration::checksum` becomes `&'static [u8]` and `RECORD` binds it as is. `compare` is unchanged, the column being a BLOB compared byte for byte.
- `blake3.workspace = true` replaces `sha2` as the store's build-dependency, as `melodia-net` declares it. The workspace `sha2` line and its comment go.
- **Downgrades.** sqlx 0.8.6 (v0.4.2 to v0.7.0) and 0.9.0 (v0.8.0 to v0.22.0) refuse an applied version they don't know (`validate_applied_migrations`, `VersionMissing`) before comparing any checksum, and compare only versions they know. No release sets `ignore_missing`, so an older build refuses a blake3 row at its version, as it refuses any newer version today.
- **Lock.** `sha2` 0.11.1, `digest` 0.11.3, `block-buffer` 0.12.1, `crypto-common` 0.2.2 and `hybrid-array` 0.4.13 leave. blake3 is already `melodia-net`'s build-dependency with the same features, so nothing new builds. `sha2` 0.10.9, `digest` 0.10.7, `generic-array` and `typenum` leave with sqlx in Phase 4.
- Lands before a 15th migration is written, so no database records a SHA-384 for a version the table doesn't pin.
- **Hand test:**
  1. `$OUT_DIR/migrations.rs` before and after holds the same 14 checksums, byte for byte.
  2. The dev library boots with nothing pending and nothing mismatched; history and `backups/` are unchanged across a boot and a tray quit.
  3. A fresh data folder's 14 rows match a sqlx-applied database's in version, description, success and checksum, compared with `/usr/bin/sqlite3`.
  4. A byte appended to a shipped migration, and a shipped one renamed, each fail the build (restored after).
  5. A scratch 15th migration, never committed, records a 32-byte checksum, and a second boot finds nothing pending.
  - **Result (2026-11-05):** all five pass, run unattended on Kenan's go. Each boot quit through the tray and exited 0, and the dev folder was restored from its backup afterwards, byte for byte.
    1. The pinned SHA-384s equal the dev library's 14 `_sqlx_migrations` rows (read with `/usr/bin/sqlite3 -readonly`), and the generated file's 14 checksums are unchanged byte for byte.
    2. History and `backups/` are unchanged across a boot and a tray quit.
    3. A fresh folder's 14 rows equal the dev library's in version, description, success and checksum. Every `execution_time` is positive, and no backup was taken.
    4. A byte appended to `20261102000001_excluded_files.sql` fails with "changed after it shipped", and renaming it fails with "shipped and is missing". Both were restored, and `git status` and `sha384sum` came back clean.
    5. A scratch `20261105000000` migration:
       - generated a 32-byte checksum equal to its blake3;
       - on the dev library, took `melodia-v20261102000001.db` first and then recorded that checksum with a positive time;
       - left history and `backups/` unchanged on a second boot.
  - The lock lost exactly the five packages above.
- **Docs:**
  - `build.rs`'s `//!` and `Migration::checksum`'s doc stop naming SHA-384 for every file.
  - `migrate.rs`'s `//!` keeps sqlx's row format rather than "row for row".
  - `CLAUDE.md`'s migration bullet and `code-style.md`'s frozen-comments sentence name the build failure beside the boot one.

## Phase 3.6: The visitors become folds ✅

The cutover runs each read in a `'static` closure on the blocking pool, which a visitor borrowing its caller's state can't enter. The fold shape lands here, under sqlx, so the cutover leaves `melodia-app` only step 10's owned inputs.

- `visit_folder_tracks` and `visit_duplicate_keys` become `fold_folder_tracks` and `fold_duplicate_keys`. Each takes an owned accumulator and a step called with `&mut` it and each row, both `Send + 'static`, and hands the accumulator back, still one row at a time. Under sqlx the body keeps streaming through `fetch`.
- Their callers move their state in and take it back: `library/scan/mod.rs:305` its `WalkIndex`, `library/duplicates/mod.rs:20` its keys.
- `scan_tests`' four `visit_folder_tracks` tests, whose closures borrow a local, move to the fold.
- **Hand test:**
  - a rescan with one file retagged between updates that row only;
  - the duplicate finder lists the groups it lists today.
  - **Result (2026-11-05):** both pass, run unattended on Kenan's go. Each boot quit through the tray and exited 0, and the dev folder was restored from its backup afterwards, byte for byte.
    1. A scratch folder of four generated tones joined the dev library, a boot scanned it, and one file was retagged before a second boot.
       - That boot re-parsed only the retagged file, skipping the folder's other 3.
       - The 50,500- and 512-track folders skipped every file on both boots.
       - The `tracks` tables after the two boots differ in that one row: its title, folded columns, size, mtime and hash, with its id and `date_added` kept.
       - The one warning, zbus failing to parse `NameLost` at the second quit, appears in logs from before this phase.
    2. `library::duplicates::find` on a snapshot of the dev library, under `a7d94dc5` and under this phase, printed byte-identical output: 14 groups and 28 copies, over 51,012 tracks.
- **Docs:** none beyond the two functions' own.

## Phase 4: The cutover (one PR, four commits)

The store and core move to rusqlite together, over sub-phases 4a to 4d, each a commit Kenan makes at its end. The ADRs wait for Phase 5's measurement, which decides whether the move stays.

- **Builds.** 4d's commit is the first whose every target builds, 4c's building all but the tests. 4a to 4c's messages say the workspace builds again at 4d, so a plain `git bisect` knows to skip them.
- **Checks.** A sub-phase ends when the compiler reports no error in the files it converted. The gates run once after 4d, and what they or the hand test find lands as its own commits after it.
- **The PR** opens after 4d. Only `pr-validation.yml` runs on a branch, and only on `pull_request`, so pushing 4a to 4c runs nothing.

**Progress.** Each session ends by ticking what landed and rewriting "Where it stands"; the next one starts there.

- [x] **4a Foundation:** steps 1 to 7. Done when no error is left outside `queries/`.
- [x] **4b Scan:** step 8's scan cluster, its fold (step 9) among it, with step 10 for each function it converts. Its first converted module is the template the other clusters copy, named under "Where it stands" once it lands.
- [x] **4c The other clusters:** step 8's clusters 2 to 6, the duplicates fold, and step 10 for theirs. Done when no error is left outside the test targets, `melodia-app`'s callers included, which the compiler reaches only once the store builds.
- [x] **4d Tests and comments:** steps 11 and 12, and the docs. Done when the three gates pass.
- [ ] **Hand test**, below.

**Where it stands:** 4a to 4c have landed. 4d is done and waits on Kenan's commit, and the hand test follows on his go.

- **Gates (2026-11-05):** `cargo fmt --all --check`, `cargo clippy --all-targets --locked --workspace -- -D warnings` and `cargo test --locked --workspace` pass, 4,429 tests and none failed. That was the first run of 4a to 4c's SQL, and it exposed no production defect, so nothing from the gates lands after 4d.
- **4d's tests:**
  - The store's 22 test files run through the production shapes: `db.read`, `db.write` and `db.transaction` over `sql::*` or `Sql`. One deviation: `track_tests`' literal-only staging goes through `fixtures::execute`, which runs several statements in one call, rather than a local copy of it.
  - The pool's own four tests moved to `pool/tests/mod_tests.rs`, `FTS_OPTIMIZE` being private there.
  - `rows/mod.rs` gained `i32` and tuple widths 6 and 7, which only tests read so far.
  - `entity_keyset_tests`' `favorites_library` stages its six tracks in a `Vec`. `insert_tagged_track` now clones the metadata into its closure, and six held on the stack took the future past `large_futures`.
  - The seven test files outside the store take the owned signatures.
- **`VACUUM INTO` from the in-memory test pool writes a real, openable copy under rusqlite** (a scratch test, not kept). sqlx's no-op is gone, so `backup_tests::open_db` and `rusqlite.md` give the WAL a backup folds in as the reason the backup tests stay file-backed.
- **Lock:** sqlx's tree left, `sha2` 0.10.9, `digest` 0.10.7, `generic-array` and `typenum` among it. rusqlite, `fallible-iterator` and `fallible-streaming-iterator` entered, plus `sqlite-wasm-rs` and `rsqlite-vfs`, which rusqlite takes only on wasm32 and which never build here. libsqlite3-sys stays at 0.37.0.
- **The template:** `queries/excluded_files.rs`. `.claude/rules/rusqlite.md` now holds what the clusters copied, and this list keeps the sites for the PR:
  - **Pool calls:** `db.read(move |conn| …)`, `db.write(move |conn| …)` and `db.transaction(move |tx| …)`, each closure `Send + 'static` and returning `Result<T, AppError>`.
  - **A transaction's body** longer than a few lines is a sync `fn` on `&Transaction<'_>` that the closure calls (`watch_batch::apply_changes`, `retag::refresh_rows`), so its comments keep their lines.
  - **What a closure needs off a borrowed input** is read before it, into locals or a small struct (`retag::EditFacts`), rather than a clone of the whole input.
  - **Inputs (step 10, as 4c applied it):**
    - A value read only to build an `Sql` stays borrowed, the statement built before the closure: the needles, `&SmartCriteria`, the chunk loops' slices and `absorb_folders`.
    - A value the closure needs is taken owned where the direct caller holds one it is done with. So is a library-sized input whose chunks would otherwise all be built before the closure, holding a second copy of it: `batch_update_hashes` builds each chunk inside its transaction.
    - Where the direct caller only borrows the value or keeps it, the store borrows and copies in (`ids.to_vec()`): the tag editor's five reads, `entity_ids`, `entity_labels`, `missing_ids`, the radio logo calls.
    - A `melodia-app` pass-through takes what its store function takes (`library::keyset`'s `*_by_ids`), so a views caller done with its ids moves them in.
  - **Folds:** `sql::fetch_each(conn, SQL, params, |row| …)` inside one read closure, the accumulator moved in and handed back (`fold_folder_tracks`). `fold_duplicate_keys` takes the same.
  - **Static text:** `sql::{execute, fetch_one, fetch_optional, fetch_all}(conn, CONST, (a, b))`, cached. The params are a rusqlite tuple or `()`, and a `DismissalSet` binds as `i64::from(set)`.
  - **Built text:** `Sql` as before, its four methods taking the connection.
  - **Rows:**
    - an entity decodes through `rows/`;
    - a store-private row takes `by_name!(Row { field, nested: Inner })` beside its struct;
    - a lone column reads as its scalar (`fetch_one::<i64>`, every sqlx 1-tuple among them), and a tuple from width 2;
    - a missing scalar type or tuple width is a line in `rows/mod.rs`, as `Option<i32>` (a playlist's `MAX(position)`) and `DismissalSet` (`dismiss`'s next set) were;
    - a row wider than a tuple reads by name: `album::get_release_tags_for_tracks` aliases its columns into a private `TrackRelease { id, tags: ReleaseTagRow }`.
  - **Helpers:** `chunked_in_query(conn, …)` (the `_on` form is gone), `chunked_in_execute_on(&tx, …)`, and `select_in_id_order(conn, …)` inside the caller's closure. `backfill_folded(db, target)` takes the `FoldedColumn` by value.
  - **A membership** resolves on the connection that reads it: `Membership::resolve`, `pick_random`, `fetch_packed_ids` and `smart_playlist::first_cover` are sync on `&Connection`, so a scoped keyset, scope fact or cover is one read closure.
  - **More than 16 binds** go through `rusqlite::params![]`, past the tuple `Params` limit (`radio_removal::reinsert_tx`'s 24).
- **Watch for in the hand test**, the suite having reached none of these failing. A statement it never runs fails only at runtime, through one of Decision 5's three refusals or:
  - rusqlite's by-name `get` takes the first column of a name, matched case-insensitively, where sqlx's took the last, so a by-name projection naming a column twice now reads the other one.
  - rusqlite decodes by the value's storage class: an `i64` refuses a REAL, a `String` an INTEGER, and an `i32` an integer out of its range.
  - A transaction body returning `Ok` early now commits, where sqlx's dropped transaction rolled back. Each such return in the tree (`playlist.rs`'s four, `playlist_convert`, `playlist_restore`'s empty guard, `remove_duplicates`) comes before any write or after one that changed nothing, so the outcome is the same; a new one has to hold to that.
- **`fixtures.rs` moved from step 11 into 4c:** it compiles into the store's lib for `melodia-app`'s tests, so the lib couldn't build without it.
  - Its seam reads through `rows::FromRow`, and `Readback` is gone.
  - `execute` runs `execute_batch`, which steps every statement in the string as sqlx's `query().execute()` did.
- **Done early:**
  - step 5's `keyset.rs` line: `unpack_ids` builds `FromSqlConversionFailure`;
  - step 12's `IDLE_TIMEOUT` doc (rewritten as it moved into `pool/lane.rs`), its manifest comments (core's rewritten, the store's `futures-util` line gone) and `track_links.rs`' doc.
- **The packing's stated reason went with sqlx.** `track_ids`' doc and `ranked_ids`' named sqlx's per-row hop, and both lines are gone; Phase 5's step 3 decides whether the packing stays.
- **Step 12 beyond its list:** `logos.rs`' "prepare non-persistent" and `backup_tests`' `create_if_missing` named sqlx too. `heap_trim.rs` needed no change: the reaper still ticks from the lane's creation and closes within `IDLE_CONNECTIONS_CLOSED_WITHIN`.

**Steps,** in working order:

1. **Dependencies.**
   - Workspace `rusqlite = { version = "0.39.x", features = ["bundled", "collation"] }` (Decision 8). `prepare_cached` sits behind rusqlite's default `cache` feature, so a `default-features = false` line lists it.
   - `fallible_uint` stays off: nothing binds or decodes a `u64` or `usize`.
   - sqlx comes out of the workspace, `melodia-core` and `melodia-store`. `melodia-core` takes `rusqlite` in its place, for `AppError::Database` and `DismissalSet`.
   - The store's `futures-util` line goes if `TryStreamExt` was its only use.
2. **`pool/`.**
   - **Writer lane:** one connection.
   - **Read lane:** 2 to 4 read-only connections, sized as `init_database` sizes them now.
   - **Reaper:** a runtime task that closes a connection idle past `IDLE_TIMEOUT`, keeping `IDLE_CONNECTIONS_CLOSED_WITHIN`'s bound.
   - **Contracts:** Decision 6's three.
   - **`close()`:** `PRAGMA optimize` and `FTS_OPTIMIZE` as today, then waits for in-flight work and closes every connection explicitly, since `main()` ends in `process::exit`. The readers close first and the writer last, so the writer's close folds the WAL.
   - **`test_pool()`:** one in-memory connection serving both lanes, with no reaper, since closing it would drop the database. It runs no timer at all, and tokio holds a paused current-thread clock still while a `spawn_blocking` task runs (`runtime/blocking/schedule.rs`), so a `start_paused` test no longer races the database.
   - **Statement-cache capacity:** a named constant starting at sqlx's 100, so memory and speed compare like for like. Phase 5 tunes it.
   - **A failed blocking task:** a panic is resumed, so a test reports it; a cancellation at runtime shutdown returns `AppError::DatabaseClosed`.
3. **`rows/`.** The trait, tuple impls and the 24 core entity impls (`StoredLogoAnswer` among them), plus the store's own five rows: `CopyFacts`, `DuplicateKeyRow`, `search_name::Inputs`, `search_text::Unfilled`, `migrate::Applied`. `#[sqlx(flatten)]` becomes a nested `from_row`. Phase 1's `Readback` folds in here rather than keeping a second set of tuple impls: its tuples become these, and its column impls sit beside them for the seam's scalar reads.
4. **`sql.rs`** executes on `&Connection`: static SQL cached, built SQL uncached.
5. **Core.**
   - The `FromRow` derives and the three `#[sqlx(...)]` attributes go.
   - `DismissalSet` gets `FromSql`.
   - `AppError::Database` holds `rusqlite::Error`, and `describe`'s doc (`error.rs:163`, "sqlx print theirs in `Display`") names rusqlite.
   - `error_tests` constructs `QueryReturnedNoRows` (`:163`, `:176-178`), and `:51-65` stop copying sqlx's message shape.
   - `queries/track/keyset.rs:69`'s hand-built `sqlx::Error::Decode` becomes `FromSqlConversionFailure`, with the tracks cluster (step 8).
6. **The migrator's and the backup's SQL** run on rusqlite. `backup_tests` reopens the copy read-only through `Connection::open_with_flags`, with the store's tests (step 11).
7. **`database/mod.rs`'s helpers** run on `&Connection` (Structure's collapse). A loop that writes per chunk or page outside a transaction takes one write closure per iteration, so a waiting writer still waits for one chunk rather than the table: `backfill_folded`, and the chunk loops at `track/write.rs:56,88,117` and `artist.rs:51`.
8. **Query modules.**
   - Each `pub async fn` stays `async` with its result type and runs its body in one read or write closure, the chunk loops of step 7 and `search_all` below excepted.
   - The functions taking a transaction become synchronous.
   - `watch_batch`'s held `db.read().acquire()` becomes one read closure.
   - `search_all`'s four concurrent reads (`search.rs:131`, `try_join!`) stay four read closures.
   - **The one transaction that awaits more than the database.** `commit_scan_chunk` and `commit_import` stat files inside their transaction on purpose, through `resolve_move_candidates` (`ingest.rs:327-328` argues it). Of the 29 transactions, it is the only one that does. `batch_still_on_disk` becomes synchronous inside the write closure, its rayon pass running under `ScanPool::install` on the blocking thread it is already on, with the `ScanPool` cloned in. Its small-batch stats leave the runtime thread they run on today.
   - The order is by cluster, each cluster's transactions sharing helpers:
     1. scan: `ingest`, `scan/*`, `stats`, `artwork`, `excluded_files`, `search_name`;
     2. playlists: `playlist`, `playlist_convert`, `playlist_delete`, `playlist_restore`, `smart_playlist`;
     3. tracks: `track/*`;
     4. entities: `album`, `artist`, `genre`, `folder`, `entity_keyset`, `entity_labels`, `search`;
     5. radio: `radio`, `radio_removal`;
     6. duplicates: `duplicate_dismissals`, `track/duplicates`, `track/duplicate_merge`.
9. **The two folds** (Phase 3.6) run their row loop inside one read closure, the accumulator moving in and back out. Their callers are unchanged.
10. **Inputs into closures** (Cross-cutting) are applied function by function.
11. **The 22 of the store's 31 test files that reach the database** move to rusqlite, and the seven test files outside the store that "Where it stands" lists take the owned signatures. `fixtures.rs` and Phase 1's seam moved to 4c. Tests that drop a transaction uncommitted keep working, since rusqlite's `Transaction` rolls back on drop.
12. **Comments that name sqlx:**
    - `melodia-engine/.../actions.rs:80-81`;
    - the "deliberately not `start_paused`" reasoning in `play_count_flusher_tests.rs:210-213` (with `:229`) and `rating_writeback_tests.rs:179-181`, whose premise (sqlx's idle and lifetime timers on the test pool) no longer holds (step 2);
    - `heap_trim.rs:1-3` and `:21-22` and `shutdown.rs:95-100`, re-read against the new reaper and `close()`;
    - `scripts/measure-footprint.py:129-132`, which names the reaper.

**Hand test:** the whole app on the dev library:
- a boot with nothing pending;
- a fresh data folder applies all 14, its `sqlite_master` and `_sqlx_migrations` rows (version, description, success, checksum) equal to a sqlx-applied database's, compared with `/usr/bin/sqlite3`. Phase 2 proved it under sqlx's `raw_sql`; `execute_batch` steps each statement once, which every one of the 14 finishes in;
- a boot from an older backup, so the backup-then-migrate path runs;
- a first scan, a rescan and watcher moves and renames;
- search, and every list and grid sort and filter;
- manual and smart playlists, convert, restore and delete;
- radio favourites and logos;
- the duplicate finder and Keep Out;
- tag editing;
- quit, after which the WAL folds and no `-wal` stays behind;
- quit while a search or scan is running, which closes within `flush_tasks_and_db`'s budget: the log says "All background tasks completed" rather than the 3 s warning, and no `-wal` stays behind. A dropped read and a scan's call in flight both run to completion (Cross-cutting), and `close()` waits for them before the writer folds the WAL.

**Docs:**
- `.claude/rules/sqlx.md` becomes `rusqlite.md` with the same `paths:` globs (`rules_globs.rs` holds them). The SQLite-generic sections stay; the sqlx-specific ones go. Its in-memory `VACUUM INTO` note is re-checked under rusqlite before it is kept.
- The rusqlite mechanics the code can't hold go in the new rule: cached versus uncached by SQL type, by-name `get` cost, owned inputs, lanes bounding concurrency under the blocking cap.
- `CLAUDE.md`'s rules table, `library-data.md:19` and `:38-42` (the collapsed helper names), and `README.md:443` and `:509` change to match.

## Phase 5: Measure, keep it or not, tune, take the latest rusqlite

Each measurement step waits for Kenan's go. Step 1's numbers decide whether the move stays, and only a move that stays is recorded in the ADRs (step 2).

1. **Measure the driver.** Fresh release builds of the commit before 4a and of Phase 4's last, each in its own detached worktree, `cmp`'d first. On the dev library measure:
   - a full scan's wall time;
   - Songs open;
   - typing a search;
   - idle Anonymous and a heaptrack diff;
   - the 150 s settle, to see connections close;
   - peak threads, the blocking pool having spawned beyond the five lanes under bursts in the benchmark (Cross-cutting).
2. **Keep it or not, then the ADRs.**
   - **The move stays if step 1 shows it faster and lighter** than the commit before 4a: the scan, Songs open and search no slower, idle Anonymous and the heaptrack diff no larger.
   - **If it doesn't, whether it stays is Kenan's call then**, and the ADRs are left as they are.
   - **If it stays, the ADRs record it**, under `docs/adr/README.md`'s rules: a decision that reverses gets a new ADR, and an accepted one is never edited to reverse itself.
     - **A new ADR, the next free number (41 today), records the move:** SQLite through rusqlite, the pool and the migrator the store's own. Its status carries the date step 1 confirmed it, and it is written in the README's shape:
       - **Context:** what forced it, in the symptom's terms. The bundled SQLite waited on a sqlx release, and every statement and row crossed a worker thread, which the store already worked around.
       - **Decision:**
         - rusqlite;
         - one writer and a band of readers as lanes on the runtime's blocking pool (ADR 5);
         - a migrator of its own over sqlx's history table and row format, so an older build still refuses a database a newer one migrated.
       - **Alternatives:** staying on sqlx, Diesel, SeaORM, Turso, libsql, an async wrapper or pool crate over rusqlite, and rusqlite_migration or refinery for the history. Decision 1 and 2 carry why each lost.
       - **Trade:** what it costs as well as what it buys, step 1's result being why it stayed:
         - a pool and a migrator to maintain;
         - a dropped call running to its end;
         - a blocking pool whose thread count the lanes don't bound (ADR 30);
         - a history table keeping a name and a format for a driver no longer in the tree;
         - the statements rusqlite refuses where sqlx let them through.
       - **What ADR 4 argued that still holds:** a migration is irreversible once applied, the backup before it is fatal if it fails, and a development build has its own data root. ADR 13 and ADR 27 link ADR 4 for exactly these, so the new one states them.
       - **Its evidence:** it names the test holding the history format, if Phase 4's step 11 leaves one.
       - **What it never names:** this plan doc, which is deleted, or a dependency version, which goes stale on a bump.
     - **ADR 4** takes `Superseded by` the new ADR, as a link, for its status, and is otherwise left exactly as written.
     - **The index** (`docs/adr/README.md`): ADR 4's status, and the new ADR's row under Foundation.
     - **ADR 35** takes a dated `**Amendment:**`. A pool on the engine's side would now pin it to rusqlite rather than sqlx, and its decision stands unchanged. The code comment beside it, `actions.rs:80-81`, is Phase 4's step 12.
     - **Left as written:**
       - ADR 1's "sqlx rather than rusqlite" is its example of a choice with no file to sit beside, and still is one.
       - ADR 13 and ADR 27 link ADR 4 for facts the new ADR carries forward, and the link reaches it through the superseded status.
3. **Tune from what that shows:**
   - the statement-cache capacity, memory against re-prepares;
   - dropping `fetch_packed_ids`' `group_concat` packing and `unpack_ids`, whose stated reason is sqlx's per-row hop. Under rusqlite a row per id measured no slower than the packed row ([Evidence](#evidence-2026-11-04)); the dev library confirms it before the packing goes;
   - by-index mapping for a wide projection, only if a profile shows column lookup.
4. **Bump rusqlite** to the latest release (0.41.x at the time of writing, SQLite 3.54), adapting to the API changes Decision 8 lists, and re-measure the scan and the list open.
- **Docs:**
  - `rusqlite.md` takes what tuning settled.
  - The README footprint tables are retaken by their protocol only if a column moves beyond run-to-run noise.

## Phase 6: Tests

Tests are added where the landed change earns them, on Kenan's go.

## Cross-cutting

- **Inputs into closures.** A closure on the blocking pool is `'static`. A store function whose input it needs takes that input owned where callers are done with it (scan batches, id lists built for the call). A borrowed input the caller keeps using is cloned in.
- **Cancellation.** A dropped store future no longer stops its query. sqlx stopped at its next row send; a rusqlite closure runs to the end. Where code drops a query future to abandon it (a superseded search), the read finishes and is discarded, its permit and connection staying with the closure until then (Decision 6). `InterruptHandle` is the lever if Phase 5 shows it matters.
- **Threads.** No new named threads: the work runs on tokio's blocking pool, and the reaper is a runtime task. The lanes bound database concurrency at one writer plus the read connections, inside the blocking cap of 32. `packaging.rs`'s `RUNTIME_NAMED` list is unaffected. The pool's thread count is not bounded by the lanes, though: a permit freed inside a closure lets the next call spawn before that thread is idle again, so bursts took the benchmark's process to 12 to 27 threads, against 9 under sqlx's fixed one per connection. Each idle one retires after tokio's keep-alive, and the heap still came out smaller (Phase 5 measures it on the app).
- **Errors** still go through `error::describe` when logged. Unchanged rules: `Nothing inside a write transaction stats a file`, `UNICODE_LOWER` query-side only, and `database::placeholders` for every IN list.

## Risks / notes

- **Phase 4 is large:** about 43 store files, core's 12 entity files and 22 store test files, plus the `melodia-app` callers whose inputs become owned. It is mechanical once Phases 1 to 3.6 have landed. A half-converted store can't run, so 4a to 4d build only at 4d (Decision 7).
- **Driver strictness.** A statement the suite doesn't reach can fail only at runtime, through any of Decision 5's three refusals. Phase 4's hand test is the net.
- **Copying inputs into closures** adds allocations where a large borrowed input is cloned. Cross-cutting's rule keeps the big ones owned.
- **The statement cache** holds sqlx's capacity to start. rusqlite's default of 16 would change re-prepare costs and memory at once.
- **The reaper's timing** carries `IDLE_CONNECTIONS_CLOSED_WITHIN`. `heap_trim`'s second trim and the README's 150 s settle both depend on it.
- **The database path.** Today it reaches sqlx as `format!("sqlite:{}", db_path.display())` on every platform, which sqlx splits at the first `?` and percent-decodes, `display()` being lossy besides. rusqlite takes the `Path`: its bytes on Unix, `to_str` or `InvalidPath` elsewhere. Its default flags include `SQLITE_OPEN_URI` and the bundled build sets `SQLITE_USE_URI`, but a name without `file:` stays a plain filename, so a data root holding `%` or `?` now opens the file it names. The Windows and macOS hand checks cover the boot.

## Checked and left alone

- **The 14 migration files.** They are frozen. None carries a sqlx directive, and every top-level statement is safe inside the migrator's transaction. `20260514000000_initial_schema.sql:3` ("PRAGMAs are configured via SqliteConnectOptions, not here") goes stale and stays, its comments frozen with its checksum.
- **FTS5 SQL and triggers, `UNICODE_LOWER`'s comparator, `queries::track::columns`' lists** (by-name mapping keeps their doc true), **`strip_windows_verbatim_paths`** (a data fix, not a driver workaround) and **the bundled build's flags.** libsqlite3-sys's `bundled` sets the same `SQLITE_ENABLE_FTS5`, `JSON1`, `STAT4`, `THREADSAFE=1`, `DEFAULT_FOREIGN_KEYS=1` and `USE_URI` in 0.30.1, 0.37.0 and 0.39.1, and sqlx and rusqlite share that crate. 0.39 drops `SQLITE_ENABLE_MEMORY_MANAGEMENT` (Decision 8).
- **CI:** `pr-validation.yml:155` names libsqlite3-sys among the C builds, still true. No workflow installs sqlx-cli or reads `DATABASE_URL`.

## Evidence (2026-11-04)

**Setup.**
- sqlx 0.9.0 and rusqlite 0.39.0 in one release build with Melodia's profile, both on libsqlite3-sys 0.37.0, so one SQLite 3.51.3 serves both and only the driver differs.
- A fresh 50,000-track database per process, built by the 14 migrations, so FTS5 and the stats triggers fire. 2,000 artists, 5,000 albums, 40 genres.
- `init_database`'s pools and pragmas, a statement cache of 100, a runtime with two workers and the blocking pool at 32, and `allocator.rs`'s glibc caps through `GLIBC_TUNABLES`. The rusqlite side is Decision 6's lanes.
- Medians of five interleaved rounds on tmpfs, the order alternating, each driver and suite in its own process. Every run returned the same checksums.
- Anonymous memory is sampled every 5 ms. `VmHWM` is left out: it counts the database's tmpfs-backed map, the same under both.

**A mix of store calls**, each awaited as a store function is:

| Workload | sqlx | rusqlite | sqlx / rusqlite |
|---|---|---|---|
| Insert 50k tracks, 100 transactions | 1,659 ms | 1,374 ms | 1.21× |
| 20k point lookups, one call each | 240 ms | 145 ms | 1.65× |
| Full sorted track list ×10 (50k rows) | 2,032 ms | 1,090 ms | 1.87× |
| Keyset-paged walk ×2 (200-row pages) | 476 ms | 244 ms | 1.95× |
| Album grid in `UNICODE_LOWER` order ×50 | 873 ms | 507 ms | 1.72× |
| Album `GROUP BY` ×100 | 1,241 ms | 883 ms | 1.41× |
| FTS5 prefix search, a call per keystroke | 1,087 ms | 1,054 ms | 1.03× |
| 10k play-count updates, one transaction | 54 ms | 18 ms | 3.10× |
| 1k rating writes, a call each | 19 ms | 9 ms | 2.03× |
| 8 tasks × 2.5k lookups at once | 120 ms | 41 ms | 2.90× |
| **Wall, all workloads** | 7,795 ms | 5,382 ms | 1.45× |
| **CPU, all workloads** | 10,400 ms | 5,490 ms | 1.89× |

| After that mix | sqlx | rusqlite |
|---|---|---|
| Peak anonymous memory | 46.8 MiB | 41.3 MiB |
| Anonymous, pools open | 32.7 MiB (23.0 to 32.7 across runs) | 18.7 MiB |
| Anonymous, pools closed | 25.8 MiB | 18.7 MiB |
| Peak threads | 9 | 14 (12 to 21 across runs) |

**The lists as the app loads them.** A full fetch of every row, against the pager's packed keyset plus the first 128-row window (`paged_rows`' fetch):

| Workload | sqlx | rusqlite | sqlx / rusqlite |
|---|---|---|---|
| Songs open ×10, all 50k rows | 3,007 ms | 1,752 ms | 1.72× |
| Songs open ×10, keyset + first window | 764 ms | 754 ms | 1.01× |
| Albums open ×50, all 5k cards | 841 ms | 477 ms | 1.76× |
| Albums open ×50, keyset + first window | 189 ms | 177 ms | 1.07× |
| Songs scrolled end to end, 391 windows | 264 ms | 129 ms | 2.05× |
| Songs keyset ×10, a row per id | 981 ms | 735 ms | 1.34× |

| Anonymous memory the lists added at peak | sqlx | rusqlite |
|---|---|---|
| Full fetch | 24.8 MiB | 23.4 MiB |
| Pager | 4.8 MiB | 4.0 MiB |

- The pager opens lists 2.3 to 4.4× faster than a full fetch and adds about 20 MiB less, under either driver. With it, the open is the engine's work, and the driver stops mattering.
- A keyset fetched a row per id costs sqlx about 30 % over the packed row and costs rusqlite nothing (Phase 5's packing step).
- Not measured here: disk I/O, a real scan's tag reads and hashing around the inserts, and idle memory once the reaper has closed the connections. Phase 5 measures those on the app.
