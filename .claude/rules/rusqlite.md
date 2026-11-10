---
paths:
  - crates/melodia-store/src/database/**/*.rs
  - crates/melodia-core/src/entities/**/*.rs
  - crates/melodia-core/src/error.rs
  - crates/melodia-app/src/library/**/*.rs
  - crates/melodia-app/src/tasks/**/*.rs
  - crates/melodia-app/src/services/artist_images.rs
  - crates/melodia-app/src/services/diagnostics.rs
  - crates/melodia/src/shutdown.rs
  - migrations/**/*.sql
  - Cargo.toml
  - crates/melodia-store/Cargo.toml
  - crates/melodia-core/Cargo.toml
---

# rusqlite (SQLite) Best Practices

## Dependency

- `libsqlite3-sys` declares `links = "sqlite3"`, so the graph holds one copy: a member takes
  rusqlite through the workspace line, and a third-party crate over rusqlite enters only where it
  resolves the same `libsqlite3-sys`
- The workspace line names `bundled` and `collation` and keeps the default `cache`, which
  `prepare_cached` sits behind: a `default-features = false` line has to list it
- The bundled build compiles without `SQLITE_ENABLE_MEMORY_MANAGEMENT`, so each connection keeps
  a page cache of its own, `cache_size` apiece and behind no lock, and `sqlite3_release_memory`
  frees nothing. Defining it again pools every connection's pages behind one global mutex

## Connections

- One writer and a small band of read-only readers, each a lane on the runtime's blocking pool
  (`database::pool`'s `//!`). `init_database` sizes the band; `pool/open.rs` sets each role's
  pragmas and the statement-cache capacity
- A store call is one closure: `db.read`, `db.write` or `db.transaction`, `Send + 'static`.
  Nothing awaits inside one, so no transaction spans an await
- The closure owns what it reads. Take an input owned where the caller is done with it, copy in
  one the caller keeps, and build an `Sql` from borrowed input before the closure
- A dropped store future doesn't stop its closure: the call runs to its end and the result is
  discarded
- The lanes bound database concurrency, not threads: a permit freed inside a closure lets the next
  call spawn before that thread is idle, so the blocking-pool cap (`tokio.md`) is the bound
- A one-off bulk write leaves its connection's page cache full, `cache_size` of heap, until the
  reaper closes it. `Connection::release_memory` after the commit frees it at once, as
  `order_keys::fill_missing` does; it leaves the connection's map, statements and schema open

## Pragmas & Configuration

- `pool/open.rs` sets each role's pragmas, its constants carrying why each value. Set one through
  `pragma_update`: `journal_mode` and `mmap_size` answer with a row, which `execute` refuses
- SQLite ignores a pragma it doesn't know, and `pragma_update` checks only that the name is an
  identifier (`extra_check`, which would check it against `pragma_list`, is off): a misspelled
  pragma changes nothing and reports nothing
- `synchronous=NORMAL` under WAL stays consistent but not durable: a commit just before a power
  loss can roll back
- The bundled build enforces foreign keys by default (`SQLITE_DEFAULT_FOREIGN_KEYS=1`) and stock
  SQLite doesn't, so `open.rs` sets the pragma as well
- String literals take single quotes: every connection turns SQLite's double-quoted-string
  fallback off (`pool/open.rs`), so a double-quoted literal fails when it is prepared, a new
  migration's included

## Queries

- Static text runs through `sql::{execute, fetch_one, fetch_optional, fetch_all, fetch_each}`.
  **A statement assembled at runtime is a `database::sql::Sql`**, whose `vouched` and
  `push_vouched` are the explicit no-injection marker: values go in through `bind` / `push_bind`,
  and the text holds only SQL *structure* (placeholder lists, column projections). Text fixed for
  the whole run is interned as a `&'static str` (a `LazyLock<String>`) instead, so it stays cached
- Caching follows the text's type (`database::sql`'s `//!`)
- rusqlite refuses three things the compiler lets through, each only when the statement runs:
  - a second statement in one string (`MultipleStatement`): only `execute_batch` runs several, as
    the migrator does
  - a row from `execute` (`ExecuteReturnedResults`): `RETURNING` reads through `fetch_*`
  - a bind count unlike the statement's (`InvalidParameterCount`)
- Params are a tuple up to 16 values, `rusqlite::params![]` past it
- A value used twice is one `?N` written twice and bound once, the bind count being the highest
  index. A bare `?` takes the index after the highest so far, so a statement never mixes the two
- A multi-row `RETURNING` answers in no set order: match each row back by a key it returns, as
  `insert_tracks_batch` matches `(file_path, cue_track)`
- Always use parameterized queries — never string concatenation
- Guard against empty `IN` lists before building the statement

## Rows

- `rows::FromRow` decodes a row: an entity by column name (`by_name!`, beside a store-private
  struct), a lone column or a tuple by position. A missing scalar type or tuple width is a line in
  `rows/mod.rs`
- A field added to an entity is a column every projection decoding it has to select. The compiler
  holds the `rows/` impl to the struct, but a projection missing the column fails only when it
  runs (`InvalidColumnName`)
- A by-name lookup takes the first column of a name, matched case-insensitively, so a projection
  names each column once
- An entity reads its columns by index, found once per statement (`rows::FromRow::columns`): a new
  row type takes `by_name!`, never a by-name `get` on each row
- `get_ref` borrows a column's value where `get` copies it out, for text read only to parse it
- Decoding follows the value's storage class: an `i64` refuses a REAL, a `String` an INTEGER, an
  `i32` an integer outside its range. `bool` reads an INTEGER, and `Option<T>` reads NULL as `None`
- `fallible_uint` is off, so a `u64` or `usize` neither binds nor decodes: convert at the edge, as
  `sql::execute` does with rusqlite's changed-row count

## Bulk Inserts

- SQLite's default bind variable limit is 32,766 as of 3.32. The 999 that most documentation still
  quotes is the *old* default.
    - **Melodia chunks at its own `database::MAX_BINDS_PER_STATEMENT`, which keeps that lower
      figure deliberately.** Named for the role rather than for the engine, because it is a budget
      we pick; the argument for the value, and for what moving it would take, lives on the constant.
- Chunk rows at the budget *less* every bind that is not part of a row (the `SET` value an
  `UPDATE … WHERE id IN (…)` carries, the parent id an `IN`-list hangs off), and divide what
  remains by the query's binds-per-row. Subtract first: dividing first spends the reserved slots
  once per row
- A multi-row INSERT spells its tuples with `database::values_placeholders` and binds them through
  `Sql`, so it runs uncached with no flag of its own
- Inside one transaction, a cached single-row statement run per row skips the parse a multi-row
  one pays on every chunk. Which shape is faster turns on the row's width, so a new bulk path
  measures both
- Always guard against zero-length input before building bulk queries

## Transactions

- Keep write transactions as short as possible — batch the work, then commit
- `db.transaction` commits when its closure returns `Ok`, so an early `Ok` commits whatever ran
  before it: return early only before the first write, or after one that changed nothing
- Melodia's transactions are `DEFERRED`; `DbPool::transaction`'s doc says why
- Read transactions are fine for longer operations in WAL mode, at the WAL's cost (below)

## Schema Management

- Runtime DDL is idempotent, `IF NOT EXISTS` / `IF EXISTS`, as `stats.rs`' triggers and
  `migrate.rs`' history table are
- Index columns used in WHERE, JOIN, and ORDER BY clauses
- A partial index serves only a query repeating one of its WHERE terms, `IS NOT NULL` aside, which
  any comparison on the column implies: `rating >= ?` never used a `WHERE rating > 0` index
  (`migrations/20260708000000_readd_rating_index.sql`)
- Avoid over-indexing — every index slows writes
- **Melodia migrates through `database::migrate`**, over sqlx's `_sqlx_migrations` table and row
  format; its `//!` says why the format stays
- A migration rebuilding a table others reference opens with the line `-- foreign-keys-off`
  (`migrate::ForeignKeys`), and holds no `PRAGMA`, `BEGIN` or `COMMIT` of its own

## Performance

- Read `EXPLAIN QUERY PLAN` before and after a change: `SCAN` with no index is a full pass, and
  `USE TEMP B-TREE FOR ORDER BY` a sort on every run
- An index answers an `ORDER BY` only on the bare column in the index's own collation: another
  `COLLATE`, or an expression such as `COALESCE(col, '')`, sorts in a temp B-tree unless an index
  carries that expression spelled the same, as `idx_tracks_year_order` does. A respelling sorts
  right and walks nothing, silently
- A list whose rows a filter or a scope narrows names its order's indexed term behind `+` or
  `COALESCE`: named bare, `SQLite` walks the index and looks up every row the filter then drops
  (`keyset::IndexUse`)
- `SELECT *` reads a whole entity by name from a stats view or a small table. A list names its
  columns (`queries::track::columns`), `SELECT * FROM tracks` serving test fixtures only
  (`track/full_rows.rs`)
- Use covering indexes for frequently accessed column combinations
- A set read across tables is one `DISTINCT` over their `UNION ALL`, never a `UNION`
  (`queries::artwork`'s `REFERENCED_PATHS` argues it)
- A plain `VACUUM` runs only as `DbPool::compact`, once at the end of every boot that applied a
  migration, its doc arguing why: a new migration owes none of its own, and nothing else runs one,
  a delete's pages going to the freelist for later writes
- `VACUUM INTO ?1` is the safe way to copy a *live* database — it runs through the open
  connection, so WAL content is folded in and the output needs no `-wal`/`-shm` beside it. The
  path binds like any value. A test of the copy builds a **file-backed** pool, an in-memory one
  having no WAL to fold in: `crates/melodia-store/src/database/tests/backup_tests.rs::open_db` is
  the worked example.

## WAL and Maintenance

- The WAL checkpoints itself at SQLite's default 1000 pages, and a checkpoint resets it without
  truncating it unless `journal_size_limit` is set. The last connection to close checkpoints and
  deletes it (`DbPool::close`'s order)
- A read holds its snapshot for its whole closure, and no checkpoint passes a live snapshot: a
  write landing during a library-sized `fetch_each` grows the WAL until the fold returns
- `PRAGMA optimize` keeps `sqlite_stat1` current and is usually a no-op. `DbPool::close` runs it
  on the writer at quit, the readers being read-only, with the mask `STATS_OPTIMIZE` argues. A
  mask replaces the default whole, so a bit is added on top of the default `0xfffe`, never alone

## Fetching and Errors

- `fetch_one` fails with `QueryReturnedNoRows` when no row comes back: use `fetch_optional` where
  absence is expected rather than catching it
- `fetch_all` collects the whole result set: give it a known bound (`LIMIT`, a page, a chunk)
- `fetch_each` hands each row to a fold, for a library-sized result never held whole
- A constraint violation answers `Some(ErrorCode::ConstraintViolation)` from
  `sqlite_error_code()`, and `sqlite_extended_error_code()` tells a UNIQUE one apart with
  `Some(ffi::SQLITE_CONSTRAINT_UNIQUE)`
- A match telling database failures apart takes `AppError::DatabaseClosed` beside `Database`, or
  a closed pool falls to its catch-all (`radio/callbacks/kept.rs`)
