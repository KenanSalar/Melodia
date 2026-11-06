# ADR 41: SQLite through rusqlite, with the pool and the migrator our own

**Status:** Accepted, 2026-11-06; supersedes [ADR 4](0004-sqlite-through-sqlx.md)

The library database had two problems, and neither came from SQLite itself. The copy of SQLite
built into the app could only be upgraded when sqlx published a release, and even its newest
release held SQLite several versions behind what rusqlite already shipped. The other was a cost
paid on every query: sqlx runs each connection on a thread of its own, so every statement is sent
over to that thread and every result row is copied and sent back. The store was already working
around it, packing the ids of a whole list into one string so that opening the list brought back
one row instead of thousands.

Decision: the store talks to SQLite through rusqlite. The layout stays as it was, one connection
that writes and a few read-only connections beside it, and each store call runs as one ordinary
synchronous function on one of those connections, on the tokio runtime's pool for blocking work
([ADR 5](0005-one-runtime-no-ipc.md)). The small pool that hands the connections out, and the
migrator that applies schema changes at startup, are written here rather than taken from a crate.
The migrator records its history in the same table and format sqlx used, so no database needs
converting on the first launch after the switch, and an older release still refuses to open a
database that a newer one has migrated.

Alternatives: staying on sqlx, Diesel, SeaORM, Turso, libsql, an async wrapper or pool crate over
rusqlite, and rusqlite_migration or refinery for the migration history.

Trade: we measured both drivers on a library of 51,000 tracks before deciding to keep the switch,
and rusqlite was faster or level on every workload we timed. A full scan and the quick rescan at
startup finished sooner on less CPU, writes and reads running alongside each other gained the most,
startup made noticeably fewer allocations, and the app ran on fewer threads. Memory is the same once
the app is idle and at its peak during startup, and slightly lower until the connections opened at
startup close for being idle. The dependency tree shrank from 824 packages to 793: sqlx and
everything it pulled in are gone, and the desktop builds gained only rusqlite and two small helper
crates. And SQLite upgrades now arrive with rusqlite's releases, which come from the same project
that maintains the low-level SQLite bindings, so a new SQLite lands there first.

The other candidates each lost on something specific. Diesel would have meant rewriting every
query, all of them hand-written SQL today, in its own query language. SeaORM's async side is built
on sqlx, so it keeps both problems above. Turso supports neither FTS5, which the search is built on,
nor the custom collation the library sorts with, and libsql's owners name Turso, their rewrite, as
the way forward. A wrapper, a pool crate or a ready-made migrator would bring the first problem
back, because every crate in the build has to agree on a single rusqlite version, so SQLite could
only move once each of them had caught up. The two migrators also keep their history in a format of
their own, which would have meant converting every user's database once.

What it costs: the pool and the migrator are now code we maintain. A store call can't be cancelled
once it has started, so if the code that asked for it stops waiting, the query still runs to the
end and its result is thrown away. The pool limits how many store calls run at once but not how many
threads the runtime starts for blocking work, so the runtime's own cap on those threads is what
keeps the count down ([ADR 30](0030-memory-is-a-product-requirement.md)). The history table keeps
sqlx's name and row format although sqlx is no longer used anywhere. rusqlite refuses three
mistakes sqlx let pass, two statements in one string, a statement that returns a row when the code
expected none, and the wrong number of values for a statement, and it catches each one only when
the statement runs, so only a test that reaches it finds it before a user does. And it isn't
settled whether a full scan now peaks higher in memory: rusqlite's samples came out higher than
sqlx's, but the runs overlapped and varied too much to show a difference or rule one out.

What [ADR 4](0004-sqlite-through-sqlx.md) argued for SQLite itself still holds: WAL, one writer
behind a busy timeout, and FTS5 for the search. So does its argument for the required backup before
every migration.
