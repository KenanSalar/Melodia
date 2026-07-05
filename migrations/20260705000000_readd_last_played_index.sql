-- Re-add the partial index on `last_played`, dropped in
-- 20260612000000_drop_unused_track_indexes.sql.
--
-- That migration dropped idx_tracks_last_played on the grounds that
-- `last_played` was write-only — "no query filters, sorts, or groups on it
-- (no 'recently played' surface exists)". The Recently-Played view now IS
-- that surface: `queries::track::get_recently_played` runs
-- `WHERE last_played IS NOT NULL ORDER BY last_played DESC LIMIT ?` on every
-- section enter and every play-count flush while the view is visible. The
-- partial index makes that an index scan instead of a full-table scan + sort.
--
-- Symmetric with idx_tracks_play_count, which the drop migration deliberately
-- kept because the Favorites / Most Played strip reads it. Partial
-- (`WHERE last_played IS NOT NULL`) so it only covers played tracks — small,
-- and the per-write cost on the play-count flush is modest.
CREATE INDEX IF NOT EXISTS idx_tracks_last_played ON tracks (last_played)
WHERE
    last_played IS NOT NULL;
