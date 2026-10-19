-- Each card grid's searchable names, case- and accent-folded and packed, so the grids can filter in
-- SQL with a plain `instr`, as `tracks.search_text` lets the track lists.
--
-- What a row packs is `queries::search_name`'s: an album's name and the artist line its card shows,
-- an artist's name and sort name, a genre's or a playlist's name, each field followed by a newline.
-- Every writer of those names writes it, and `tasks::search_text_backfill` fills the rows that
-- predate it.
--
-- '' means not filled yet. A filled value never is, every field ending in its separator, so a
-- change to the packing ships as a migration blanking the column and the backfill refills it on the
-- next launch.
ALTER TABLE albums ADD COLUMN search_name TEXT NOT NULL DEFAULT '';
ALTER TABLE artists ADD COLUMN search_name TEXT NOT NULL DEFAULT '';
ALTER TABLE genres ADD COLUMN search_name TEXT NOT NULL DEFAULT '';
ALTER TABLE playlists ADD COLUMN search_name TEXT NOT NULL DEFAULT '';

-- The rows the backfill still owes, so its check on every launch reads four indexes that are empty
-- once the library is filled rather than four tables.
CREATE INDEX IF NOT EXISTS idx_albums_search_name_unfilled ON albums (id) WHERE search_name = '';
CREATE INDEX IF NOT EXISTS idx_artists_search_name_unfilled ON artists (id) WHERE search_name = '';
CREATE INDEX IF NOT EXISTS idx_genres_search_name_unfilled ON genres (id) WHERE search_name = '';
CREATE INDEX IF NOT EXISTS idx_playlists_search_name_unfilled
    ON playlists (id) WHERE search_name = '';
