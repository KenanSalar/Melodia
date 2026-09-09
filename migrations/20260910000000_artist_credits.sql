-- Multi-artist credits.
--
-- A track's artist stops being one name. `tracks.artist` keeps the credit as printed, which is
-- what every display surface and the FTS index read, and `tracks.artist_id` keeps the primary
-- name, which is what album grouping and `idx_tracks_artist_album` are built on. The two tables
-- below carry the rest: who else is credited, in what order, and what joins them.
--
-- The artist stats move with the meaning. `artists.track_count` / `total_duration_ms` /
-- `album_count` become "credited on" rather than "primary artist of", so their triggers move off
-- `tracks.artist_id` and onto the join tables. `artist_stats`'s `track_count > 0` filter then
-- admits an artist who is only ever a featured credit, which is the point of the whole change.
--
-- Neither join table cascades from its parent. A cascade fires after the parent row is gone, so
-- the stats trigger could no longer read the duration it has to subtract; a BEFORE DELETE trigger
-- on the parent clears the rows while it still exists instead. Losing either half of that pair
-- leaves a track undeletable under `PRAGMA foreign_keys=ON`.
CREATE TABLE
    IF NOT EXISTS track_artists (
        track_id INTEGER NOT NULL REFERENCES tracks (id),
        artist_id INTEGER NOT NULL REFERENCES artists (id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        -- Rendered form, spacing included (" feat. ", ", "). Empty on the last credit.
        join_phrase TEXT NOT NULL DEFAULT '',
        PRIMARY KEY (track_id, position)
    );

CREATE TABLE
    IF NOT EXISTS album_artists (
        album_id INTEGER NOT NULL REFERENCES albums (id),
        artist_id INTEGER NOT NULL REFERENCES artists (id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        join_phrase TEXT NOT NULL DEFAULT '',
        PRIMARY KEY (album_id, position)
    );

-- The primary key leads with the parent id, so it serves "this track's credit" but not "this
-- artist's tracks" — which is every artist-scoped read and both stat recomputes.
CREATE INDEX IF NOT EXISTS idx_track_artists_artist ON track_artists (artist_id);

CREATE INDEX IF NOT EXISTS idx_album_artists_artist ON album_artists (artist_id);

-- The album-artist credit as printed. `albums` has no text column for it the way `tracks` does,
-- its artist having only ever been the FK, so a multi-name album credit had nowhere to render
-- from. NULL means "no credit of its own"; `album_stats` falls back to the primary artist's name.
ALTER TABLE albums
ADD COLUMN artist_credit TEXT;

-- Seed one credit per row from what the FKs already say, so a library that has never been
-- rescanned is correct as of today rather than empty. Discovering the multi-value tags already
-- sitting in people's files needs those files re-read, which is the one-shot sweep's job.
--
-- Ahead of the triggers below on purpose: these inserts would otherwise be counted twice, once
-- here and once by the recompute at the end.
INSERT INTO
    track_artists (track_id, artist_id, position, join_phrase)
SELECT
    id,
    artist_id,
    0,
    ''
FROM
    tracks
WHERE
    artist_id IS NOT NULL;

INSERT INTO
    album_artists (album_id, artist_id, position, join_phrase)
SELECT
    id,
    artist_id,
    0,
    ''
FROM
    albums;

-- `idx_tracks_fav_artist` existed for `get_favorite_artists`'s GROUP BY over `tracks.artist_id`,
-- and that query reaches its artists through `track_artists` now. Verified with EXPLAIN QUERY
-- PLAN: the rewrite drives off the covering `idx_tracks_is_favorite` and seeks the credit table
-- by its own primary key, so the partial index is write cost with no reader left.
DROP INDEX IF EXISTS idx_tracks_fav_artist;

DROP TRIGGER IF EXISTS tracks_stats_insert;

DROP TRIGGER IF EXISTS tracks_stats_delete;

DROP TRIGGER IF EXISTS tracks_stats_update;

-- Superseded by `album_artists_stats_*`: an album's credit is now a set of rows rather than one
-- FK, so reassigning `albums.artist_id` no longer moves an artist's album count on its own.
DROP TRIGGER IF EXISTS albums_artist_update;

-- The three below keep the album and genre arms they always had, minus every artist arm.
-- `queries/stats.rs` holds the same text and drops and recreates these around bulk scan chunks;
-- the two copies have to move together.
CREATE TRIGGER IF NOT EXISTS tracks_stats_insert AFTER INSERT ON tracks BEGIN
UPDATE albums
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.album_id;

UPDATE genres
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.genre_id;

END;

CREATE TRIGGER IF NOT EXISTS tracks_stats_delete AFTER DELETE ON tracks BEGIN
UPDATE albums
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.album_id;

UPDATE genres
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.genre_id;

END;

-- `artist_id` leaves the watch list: it is the primary name now, and moving it re-points album
-- grouping without changing who is credited. A duration change still reaches every credited
-- artist, which is the one artist fact a `tracks` row can move by itself.
CREATE TRIGGER IF NOT EXISTS tracks_stats_update AFTER
UPDATE OF album_id,
genre_id,
duration_ms ON tracks BEGIN
UPDATE artists
SET
    total_duration_ms = MAX(
        total_duration_ms + new.duration_ms - old.duration_ms,
        0
    )
WHERE
    id IN (
        SELECT
            artist_id
        FROM
            track_artists
        WHERE
            track_id = new.id
    );

UPDATE albums
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.album_id;

UPDATE genres
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.genre_id;

UPDATE albums
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.album_id;

UPDATE genres
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.genre_id;

END;

-- The artist half of the stats, now keyed on the credit rather than on `tracks.artist_id`. Both
-- arms read `tracks.duration_ms` live, which is what the BEFORE DELETE pair below buys them.
CREATE TRIGGER IF NOT EXISTS track_artists_stats_insert AFTER INSERT ON track_artists BEGIN
UPDATE artists
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + COALESCE(
        (
            SELECT
                duration_ms
            FROM
                tracks
            WHERE
                id = new.track_id
        ),
        0
    )
WHERE
    id = new.artist_id;

END;

CREATE TRIGGER IF NOT EXISTS track_artists_stats_delete AFTER DELETE ON track_artists BEGIN
UPDATE artists
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(
        total_duration_ms - COALESCE(
            (
                SELECT
                    duration_ms
                FROM
                    tracks
                WHERE
                    id = old.track_id
            ),
            0
        ),
        0
    )
WHERE
    id = old.artist_id;

END;

CREATE TRIGGER IF NOT EXISTS album_artists_stats_insert AFTER INSERT ON album_artists BEGIN
UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            album_artists
        WHERE
            artist_id = new.artist_id
    )
WHERE
    id = new.artist_id;

END;

CREATE TRIGGER IF NOT EXISTS album_artists_stats_delete AFTER DELETE ON album_artists BEGIN
UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            album_artists
        WHERE
            artist_id = old.artist_id
    )
WHERE
    id = old.artist_id;

END;

-- What stands in for the cascade the FKs deliberately don't have. BEFORE, so the join rows go
-- while their parent is still readable and the stats triggers above can price them.
--
-- These two are never dropped, unlike the stats triggers: without them a parent delete fails the
-- foreign key outright.
CREATE TRIGGER IF NOT EXISTS tracks_credits_cleanup BEFORE DELETE ON tracks BEGIN
DELETE FROM track_artists
WHERE
    track_id = old.id;

END;

CREATE TRIGGER IF NOT EXISTS albums_credits_cleanup BEFORE DELETE ON albums BEGIN
DELETE FROM album_artists
WHERE
    album_id = old.id;

END;

-- The seed above ran with no triggers watching it, and the counts it should have produced are the
-- same ones the old triggers had been keeping for the primary artist alone. Recompute rather than
-- reason about that: it is one pass over a table the user is not waiting on.
UPDATE artists
SET
    track_count = COALESCE(
        (
            SELECT
                COUNT(*)
            FROM
                track_artists
            WHERE
                artist_id = artists.id
        ),
        0
    ),
    total_duration_ms = COALESCE(
        (
            SELECT
                SUM(t.duration_ms)
            FROM
                track_artists ta
                JOIN tracks t ON t.id = ta.track_id
            WHERE
                ta.artist_id = artists.id
        ),
        0
    ),
    album_count = COALESCE(
        (
            SELECT
                COUNT(*)
            FROM
                album_artists
            WHERE
                artist_id = artists.id
        ),
        0
    );

-- `artist_name` becomes the credit where the album has one. Same column, same type, so every
-- `SELECT *` against this view is untouched.
DROP VIEW IF EXISTS album_stats;

CREATE VIEW
    IF NOT EXISTS album_stats AS
SELECT
    al.id,
    al.name,
    al.sort_name,
    al.artist_id,
    COALESCE(al.artist_credit, a.name) AS artist_name,
    al.year,
    al.disc_count,
    al.is_compilation,
    al.musicbrainz_id,
    al.artwork_path,
    al.track_count,
    al.total_duration_ms
FROM
    albums al
    JOIN artists a ON al.artist_id = a.id
WHERE
    al.track_count > 0;
