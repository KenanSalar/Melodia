# ADR 4: SQLite through sqlx, one writer and a pool of readers

**Status:** Accepted, 2026-05-25

A music library is one person's data on one machine, and it has to answer a search on every
keystroke across tens of thousands of tracks while a scan is still writing rows behind it. That
combination is what rules most of the obvious storage choices out: the read side wants indexes
and a query language, and the read and write sides have to run at the same time without the UI
noticing.

Decision: SQLite through sqlx. WAL journalling, one write connection so writes serialize, a pool
of read connections beside it, FTS5 for search, and `sqlx::migrate!` applying versioned
migrations at startup against a backup taken first.

Alternatives: rusqlite, diesel, an embedded key-value store, and plain JSON files.

Trade: SQLite is the only one of these where the concurrency story is already solved and
documented. WAL lets the readers keep reading through a scan's writes, and a single writer plus a
five-second busy timeout turns lock contention into a short wait rather than an error anyone has
to handle. FTS5 comes with it, which is the search feature not written. JSON files lose ordering,
counting and every join the moment the library outgrows a toy, and a key-value store gives up the
query language that the smart playlists and the library views are built out of.

Choosing sqlx over rusqlite buys the async pool and the migrator, and it costs a noticeably larger
dependency tree for a driver over an embedded database. It is worth being precise about what is
actually being paid for, because it is easy to assume it is the compile-time query checking: it is
not. Every query here goes through runtime `query_as`, and the `macros` feature is enabled only
because `sqlx::migrate!` is gated behind it. So the purchase is the pool, the migrator and an
async driver that never blocks a runtime worker, and nothing else.

The heaviest consequence is that a migration is irreversible once it has been applied. That is
what makes the database backup before migration fatal-if-it-fails rather than best-effort, and it
is why a development build gets its own data root: a migration still on a branch would otherwise
leave an installed Melodia unable to open its own library until that branch shipped.

This ADR was written in September 2026. No argument for this choice existed anywhere in the
repository; it is reconstructed from the schema, the pool configuration in
`crates/melodia-store/src/database/mod.rs` and the maintainer's account.
