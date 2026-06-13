-- Drop three tracks indexes that only cost write time:
--
-- * idx_tracks_artist_id — redundant left prefix of the composite
--   idx_tracks_artist_album (artist_id, album_id). Verified via EXPLAIN
--   QUERY PLAN: with the single-column index gone, the artist-detail
--   queries (`WHERE artist_id = ? ORDER BY sort_key`) switch to
--   `SEARCH tracks USING INDEX idx_tracks_artist_album (artist_id=?)`
--   with an identical plan shape.
-- * idx_tracks_last_played — `last_played` is written by the play-count
--   flusher but no query filters, sorts, or groups on it (no
--   "recently played" surface exists).
-- * idx_tracks_rating — `rating` is initialized at insert and never read.
--
-- idx_tracks_play_count stays: it backs the Favorites hero / Most Played
-- strip (`WHERE is_favorite AND play_count > 0 ORDER BY play_count DESC`).
--
-- Every dropped index was maintained on each scan insert, watcher delete,
-- and play-count flush; removing them trims that per-write overhead.

DROP INDEX IF EXISTS idx_tracks_artist_id;
DROP INDEX IF EXISTS idx_tracks_last_played;
DROP INDEX IF EXISTS idx_tracks_rating;
