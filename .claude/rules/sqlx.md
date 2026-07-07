# SQLx (SQLite) Best Practices

## Connection Pooling

- Use **two-pool architecture** for SQLite:
  - Write pool: `max_connections(1)` — SQLite only supports one concurrent writer
  - Read pool: `max_connections(num_cpus)` with `.read_only(true)` — multiple concurrent readers in WAL mode
    - **Desktop caveat (Melodia):** a single-user desktop app issues tiny, effectively-sequential reads, so scaling per-core just multiplies idle per-connection page-cache/statement-cache memory. Clamp to a small fixed band (`.clamp(2, 4)`) and add an `idle_timeout` so boot-burst connections get reaped. See `init_database` in `src/database/mod.rs`.
- A single shared pool degrades write performance by ~20x due to reader/writer contention

## Pragmas & Configuration

- Enable WAL mode: `PRAGMA journal_mode=WAL`
- Set `PRAGMA synchronous=NORMAL` (safe with WAL, significant performance gain over FULL)
- Set `PRAGMA foreign_keys=ON` (off by default in SQLite)
- Consider `PRAGMA mmap_size=268435456` (256MB) for read-heavy workloads
- Consider `PRAGMA temp_store=MEMORY` for faster temporary tables

## Queries

- Prefer `query_as!` macro for compile-time checked queries when schemas are stable
- Use `query_as()` runtime function for dynamic/generated queries
- Always use parameterized queries — never string concatenation
- Query strings are `impl SqlSafeStr` (0.9): string literals and named `const &'static str` pass as-is; a runtime-built `String` must be wrapped in `sqlx::AssertSqlSafe(sql)` (impls for `String`, `&str`, `Box<str>`, `Arc<str>`, `Cow<'static, str>`). The wrapper is an explicit no-injection assertion — keep data in `.bind()`s and interpolate only SQL *structure* (placeholder lists, column projections)
- Guard against empty `IN` clauses — SQLx errors on empty bind lists; check before executing
- Build dynamic `?` placeholders in a loop for IN clauses and bind values sequentially

## Bulk Inserts

- SQLite default bind variable limit is 999 (can be raised to 32,766)
- Chunk rows: `BIND_LIMIT / num_columns` rows per batch (e.g., 5 columns = 199 rows/chunk)
- Use `QueryBuilder::push_values()` for building multi-row INSERT statements
- Set `.persistent(false)` on dynamic bulk queries — prevents statement cache bloat
- Always guard against zero-length input before building bulk queries

## Transactions

- Never `await` long-running operations inside write transactions — causes lock starvation for other writers and readers
- Keep write transactions as short as possible — batch the work, then commit
- Use `BEGIN IMMEDIATE` for write transactions to fail fast on lock contention rather than blocking
- Read transactions (`BEGIN DEFERRED`) are fine for longer operations in WAL mode

## Schema Management

- Use `IF NOT EXISTS` in all DDL for idempotent schema creation
- Load schema via `sqlx::raw_sql(include_str!("schema.sql"))` for consolidated, version-controlled DDL
- Index columns used in WHERE, JOIN, and ORDER BY clauses
- Use partial indexes for filtered queries (e.g., `WHERE deleted = 0`)
- Avoid over-indexing — every index slows writes
- `Migrate` trait methods (`ensure_migrations_table`, `list_applied_migrations`, `dirty_version`, …) take a `table_name` arg in 0.9 — pass the default `_sqlx_migrations` unless relocated; `Migrator` builder methods (`set_ignore_missing`, `set_locking`) return `&mut Self`
- A per-crate `sqlx.toml` (0.9) can relocate the migrations table (`[migrate] table-name`), set global type overrides, and rename the database-URL env var for multi-database workspaces — changing the table name on a live database loses migration history unless the old table is copied first

## Performance

- Use `EXPLAIN QUERY PLAN` to verify index usage before optimizing
- Avoid `SELECT *` — specify only needed columns
- Use covering indexes for frequently accessed column combinations
- Regular `VACUUM` for reclaiming space after bulk deletes (but not during normal operation)

## Fetch Methods

- `fetch_one` — returns `Error::RowNotFound` if no rows; add `LIMIT 1` for best performance when filtering unique columns
- `fetch_optional` — returns `Option<T>`; prefer over `fetch_one` when absence is expected
- `fetch_all` — collects entire result set into `Vec<T>`; ensure result set has a known upper bound, use `LIMIT`
- `fetch` — returns a `Stream`; use for large result sets to avoid loading everything into memory at once
- `query_scalar` / `query_scalar!` — extract the first column of each row directly to a scalar type

## Error Handling

- `sqlx::Error::RowNotFound` — use `fetch_optional` instead of catching this error
- `sqlx::Error::Database` — contains the raw DB error code; match on `.code()` for constraint violations
- SQLite constraint violation code: `"2067"` (UNIQUE) — check `err.code()` for upsert conflict detection

## Type Mapping (SQLite)

- SQLite stores `bool` as `INTEGER` (0/1) — use `i64` or `bool` in Rust; SQLx handles the conversion
- `Option<T>` maps to nullable columns — a missing or NULL column value deserializes to `None`
- `chrono::NaiveDateTime` requires the `chrono` feature flag in SQLx
- Use `TEXT` for UUIDs (store as hyphenated string) or enable the `uuid` feature for direct mapping

## SQLite-Specific Pragmas

- `PRAGMA cache_size = -64000` (64MB page cache) — significantly improves read performance for large DBs. For a small single-user desktop DB (Melodia), size it modestly (e.g. `-16000`); with `mmap_size` set, cold pages stay cheaply file-backed instead of being copied into per-connection heap cache, so a large cache there is just idle resident overhead multiplied across the pool.
- `PRAGMA wal_autocheckpoint = 1000` — tune how often WAL is checkpointed (default 1000 pages)
- `PRAGMA optimize` — run periodically (e.g. on app close) to update query planner statistics; fast and safe

## 0.9 Upgrade Notes

- MSRV is 1.94; the project moved to the `transact-rs` GitHub org (crate name unchanged)
- Upstream dropped its `Cargo.lock` — install the CLI with plain `cargo install sqlx-cli` (`--locked` no longer resolves)
- Runtime/TLS features are split-only — the combined `runtime-tokio-native-tls`-style features are gone; use `runtime-tokio` plus a `tls-*` feature only when TLS is actually needed
- New opt-in SQLite features: `sqlite-deserialize`, `sqlite-load-extension` (extension loading now requires `unsafe`), `sqlite-unlock-notify`
- `SqliteValue` is `!Sync` and `SqliteValueRef` is `!Send` — don't stash raw values across threads; decoding through `FromRow` into owned structs is unaffected
- `libsqlite3-sys` is version-ranged — the bundled SQLite version can bump within 0.9.x patch releases
- `RawSql::fetch_optional` returns `Result<Option<Row>>` (was `Result<Row>`)
