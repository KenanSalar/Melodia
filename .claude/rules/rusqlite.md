---
paths:
  - crates/melodia-store/src/database/**/*.rs
  - crates/melodia-core/src/entities/**/*.rs
  - crates/melodia-app/src/library/**/*.rs
  - crates/melodia-app/src/tasks/**/*.rs
  - migrations/**/*.sql
---

# rusqlite (SQLite) Best Practices

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

## Pragmas & Configuration

- Enable WAL mode: `PRAGMA journal_mode=WAL`
- Set `PRAGMA synchronous=NORMAL` (safe with WAL, significant performance gain over FULL)
- Set `PRAGMA foreign_keys=ON` (off by default in SQLite)
- Consider `PRAGMA mmap_size=268435456` (256MB) for read-heavy workloads
- Consider `PRAGMA temp_store=MEMORY` for faster temporary tables
- Set them through `pragma_update`: `journal_mode` and `mmap_size` answer with a row, which
  `execute` refuses

## Queries

- Static text runs through `sql::{execute, fetch_one, fetch_optional, fetch_all, fetch_each}`.
  **A statement assembled at runtime is a `database::sql::Sql`**, whose `trusted` and
  `push_trusted` are the explicit no-injection marker: values go in through `bind` / `push_bind`,
  and the text holds only SQL *structure* (placeholder lists, column projections). Text fixed for
  the whole run is interned as a `&'static str` (a `LazyLock<String>`) instead, so it stays cached
- Caching follows the text's type (`database::sql`'s `//!`)
- rusqlite refuses three things the compiler lets through, each only when the statement runs:
  - a second statement in one string (`MultipleStatement`): only `execute_batch` runs several, as
    the migrator does
  - a row from `execute` (`ExecuteReturnedResults`): `RETURNING` reads through `fetch_*`
  - a bind count unlike the statement's (`InvalidParameterCount`)
- Params are a tuple up to 16 values, `rusqlite::params![]` past it
- Always use parameterized queries — never string concatenation
- Guard against empty `IN` lists before building the statement

## Rows

- `rows::FromRow` decodes a row: an entity by column name (`by_name!`, beside a store-private
  struct), a lone column or a tuple by position. A missing scalar type or tuple width is a line in
  `rows/mod.rs`
- By-name `get` takes the first column of a name, matched case-insensitively, so a projection
  names each column once
- By-name lookup scans the statement's columns on every call; move a wide projection to
  by-index only where a profile shows that scan
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
- Always guard against zero-length input before building bulk queries

## Transactions

- Keep write transactions as short as possible — batch the work, then commit
- `db.transaction` commits when its closure returns `Ok`, so an early `Ok` commits whatever ran
  before it: return early only before the first write, or after one that changed nothing
- Melodia's transactions are `DEFERRED`; `DbPool::transaction`'s doc says why
- Read transactions are fine for longer operations in WAL mode

## Schema Management

- Use `IF NOT EXISTS` in all DDL for idempotent schema creation
- Index columns used in WHERE, JOIN, and ORDER BY clauses
- Use partial indexes for filtered queries (e.g., `WHERE deleted = 0`)
- Avoid over-indexing — every index slows writes
- **Melodia migrates through `database::migrate`**, over sqlx's `_sqlx_migrations` table and row
  format; its `//!` says why the format stays

## Performance

- Use `EXPLAIN QUERY PLAN` to verify index usage before optimizing
- Avoid `SELECT *` — specify only needed columns
- Use covering indexes for frequently accessed column combinations
- Regular `VACUUM` for reclaiming space after bulk deletes (but not during normal operation)
- `VACUUM INTO ?1` is the safe way to copy a *live* database — it runs through the open
  connection, so WAL content is folded in and the output needs no `-wal`/`-shm` beside it. The
  path binds like any value. A test of the copy builds a **file-backed** pool, an in-memory one
  having no WAL to fold in: `crates/melodia-store/src/database/tests/backup_tests.rs::open_db` is
  the worked example.

## Fetching and Errors

- `fetch_one` fails with `QueryReturnedNoRows` when no row comes back: use `fetch_optional` where
  absence is expected rather than catching it
- `fetch_all` collects the whole result set: give it a known bound (`LIMIT`, a page, a chunk)
- `fetch_each` hands each row to a fold, for a library-sized result never held whole
- A constraint violation answers `Some(ErrorCode::ConstraintViolation)` from
  `sqlite_error_code()`; a UNIQUE one carries extended code 2067 (`SQLITE_CONSTRAINT_UNIQUE`) on
  `Error::SqliteFailure`

## SQLite-Specific Pragmas

- `PRAGMA cache_size = -64000` (64MB page cache) — significantly improves read performance for
  large DBs. For a small single-user desktop DB (Melodia), size it modestly (e.g. `-16000`); with
  `mmap_size` set, cold pages stay cheaply file-backed instead of being copied into
  per-connection heap cache, so a large cache there is just idle resident overhead multiplied
  across the pool.
- `PRAGMA wal_autocheckpoint = 1000` — tune how often WAL is checkpointed (default 1000 pages)
- `PRAGMA optimize` — run periodically (e.g. on app close) to update query planner statistics;
  fast and safe
