-- The Songs tabs' artist, album and genre orders each read a stored key, so the whole library opens
-- in one of them as an index walk rather than a sort calling Rust's collation on every comparison.
--
-- Stored rather than computed because the lowercase is Rust's: SQLite's lower() folds ASCII alone.
-- A key is its field lowercased, a missing field reading as ''.
--
-- NULL means not filled yet, '' being a real key, and every write fills the three together. The
-- store fills the rows that predate them when it opens, before any list reads them.
ALTER TABLE tracks ADD COLUMN artist_key TEXT;
ALTER TABLE tracks ADD COLUMN album_key TEXT;
ALTER TABLE tracks ADD COLUMN genre_key TEXT;

-- The natural title key after each key, the orders' tie-break, so the walk needs no sort.
CREATE INDEX IF NOT EXISTS idx_tracks_artist_key ON tracks (artist_key, sort_key COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_tracks_album_key ON tracks (album_key, sort_key COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_tracks_genre_key ON tracks (genre_key, sort_key COLLATE NOCASE);
