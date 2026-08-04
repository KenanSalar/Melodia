-- Drop idx_tracks_fav_album — a partial index on tracks (album_id) WHERE
-- is_favorite = TRUE, created for `get_favorite_albums`, which was the only
-- query shaped to drive off it. The Favorites page has had no Albums tab
-- since its strips became tabs (albums are reachable via the Albums tab),
-- and the query is now gone with it.
--
-- The initial schema's comment still names `get_favorite_albums` beside this
-- index and cannot be corrected: an applied migration is checksummed, so
-- editing one byte of it fails startup on every existing install.
--
-- idx_tracks_fav_artist stays — `get_favorite_artists` still drives its
-- per-artist GROUP BY off it. idx_tracks_is_favorite stays and still covers
-- the plain `WHERE is_favorite = TRUE` fetches.

DROP INDEX IF EXISTS idx_tracks_fav_album;
