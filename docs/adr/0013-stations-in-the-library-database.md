# ADR 13: Stations are one table in the library database, not a JSON file

**Status:** Accepted, 2026-08-20

Radio introduced three things that look like separate features: favourites, stations the user typed
in themselves, and recently played. They are the same row at three points in its life. A station is
heard, kept, or both, and the difference is a couple of columns rather than a couple of stores.

Decision: one `radio_stations` table in the existing library database, with `is_favorite`,
`play_count` and `last_played` on it, carrying all three.

Alternatives: a JSON file beside the other persisted state, and three separate stores for the three
features.

Trade: a JSON file is the lighter thing to reach for and Melodia already has several, so it was the
real alternative rather than a straw one. It loses ordering, counting and the artwork join, and each
of those is something the UI needs on the first screen: most-played ordering, a play count, and a
logo resolved through the same artwork store every other image goes through. Rebuilding those over a
parsed file is rewriting the query layer that is already sitting there, and the app already owns a
database for exactly this shape of data.

The cost is that a station is now schema. Adding a column means a migration, and a migration is
irreversible once applied ([ADR 4](0004-sqlite-through-sqlx.md)), where a JSON file could have
grown a field with a default and no ceremony. It also means the artwork sweep has to know about
another column, because a station logo is a stored image like any other and nothing else would
delete it.

The row is written when the user keeps a station rather than when they look at one. Upserting on
every drill-in was the alternative and it makes persistence uniform at the cost of a growing table
of stations somebody merely glanced at, with "seen" and "kept" indistinguishable in the same rows.
One conditional beats that.
