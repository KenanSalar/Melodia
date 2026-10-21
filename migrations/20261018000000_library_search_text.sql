-- The library's searchable names, case- and accent-folded and packed, so the track lists and the
-- card grids can filter in SQL with a plain `instr`.
--
-- Stored rather than computed because the fold is Rust's, and SQLite can only call Rust here
-- through a collation, never a scalar function. What a row packs is `SearchFields::pack`'s for a
-- track and `queries::search_name`'s for the four card tables. Every writer of those fields writes
-- the column, and `tasks::search_text_backfill` fills the rows that predate it.
--
-- '' means not filled yet. A filled value never is, the separators being there whatever the fields
-- hold, so a change to the packing ships as a migration blanking the column and the backfill
-- refills it on the next launch.
ALTER TABLE tracks ADD COLUMN search_text TEXT NOT NULL DEFAULT '';
ALTER TABLE albums ADD COLUMN search_name TEXT NOT NULL DEFAULT '';
ALTER TABLE artists ADD COLUMN search_name TEXT NOT NULL DEFAULT '';
ALTER TABLE genres ADD COLUMN search_name TEXT NOT NULL DEFAULT '';
ALTER TABLE playlists ADD COLUMN search_name TEXT NOT NULL DEFAULT '';

-- The rows the backfill still owes, so its check on every launch reads five indexes that are empty
-- once the library is filled rather than five tables.
CREATE INDEX IF NOT EXISTS idx_tracks_search_text_unfilled ON tracks (id) WHERE search_text = '';
CREATE INDEX IF NOT EXISTS idx_albums_search_name_unfilled ON albums (id) WHERE search_name = '';
CREATE INDEX IF NOT EXISTS idx_artists_search_name_unfilled ON artists (id) WHERE search_name = '';
CREATE INDEX IF NOT EXISTS idx_genres_search_name_unfilled ON genres (id) WHERE search_name = '';
CREATE INDEX IF NOT EXISTS idx_playlists_search_name_unfilled
    ON playlists (id) WHERE search_name = '';
