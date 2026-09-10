-- The rest of the tag vocabulary.
--
-- `20260910000000_artist_credits.sql` made a track's artist a set of rows. Two more fields are
-- multi-valued by the same conventions and were still single here: a role credit (composer,
-- conductor, producer, …) and a genre. Both get the shape that migration established — a join
-- table carrying order, the rendered line kept on `tracks` so every display surface and the FTS
-- index read one column, and the primary FK kept for grouping.
--
-- What is *not* repeated from it: role credits carry no `join_phrase` and move no stats. A role
-- is a set rather than a printed line, so there is nothing to join with; and `artists.track_count`
-- means "credited as an artist", which a composer must not inflate — an artist who is only ever
-- a composer keeps a count of 0 and `artist_stats` filters them out of the grid, while the Credits
-- panel still reaches them. What the table does owe is the artist row's life: `prune_orphans`
-- gains an arm for it, and so does the BEFORE DELETE cleanup below.
--
-- Everything else here is a column that had no reader, a column that had no writer, or a tag the
-- ingest never asked for. `tracks.composer` and `tracks.label` leave: the first becomes
-- `role = 'composer'` rows, the second moves to `albums`, where a record label has always
-- belonged and where it finally gets a surface.

CREATE TABLE
    IF NOT EXISTS track_credits (
        track_id INTEGER NOT NULL REFERENCES tracks (id),
        artist_id INTEGER NOT NULL REFERENCES artists (id) ON DELETE CASCADE,
        role TEXT NOT NULL,
        -- Instrument or voice for `performer` (Vorbis `PERFORMER=Name (instrument)`, ID3v2
        -- `TMCL`), empty for every other role.
        detail TEXT NOT NULL DEFAULT '',
        position INTEGER NOT NULL,
        PRIMARY KEY (track_id, role, position)
    );

CREATE TABLE
    IF NOT EXISTS track_genres (
        track_id INTEGER NOT NULL REFERENCES tracks (id),
        genre_id INTEGER NOT NULL REFERENCES genres (id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        PRIMARY KEY (track_id, position)
    );

-- Both primary keys lead with `track_id`, so they serve "this track's credits" and neither serves
-- the entity-scoped reads or the stat recomputes.
CREATE INDEX IF NOT EXISTS idx_track_credits_artist ON track_credits (artist_id);

CREATE INDEX IF NOT EXISTS idx_track_genres_genre ON track_genres (genre_id);

-- The rendered role credits, for the FTS index and the one-line display. It is a column rather
-- than a join because `tracks_fts` is an external-content table: every one of its column names
-- has to resolve against `tracks`.
ALTER TABLE tracks
ADD COLUMN credits TEXT;

-- Whole dates. `year` and `original_year` stay as the derived integers every existing index,
-- ORDER BY and smart-playlist rule is built on; these carry the month and day the reader used to
-- throw away, so an album can be ordered by the day it shipped rather than the year.
ALTER TABLE tracks
ADD COLUMN release_date TEXT;

ALTER TABLE tracks
ADD COLUMN original_date TEXT;

ALTER TABLE tracks
ADD COLUMN track_total INTEGER;

ALTER TABLE tracks
ADD COLUMN disc_total INTEGER;

ALTER TABLE tracks
ADD COLUMN disc_subtitle TEXT;

ALTER TABLE tracks
ADD COLUMN subtitle TEXT;

ALTER TABLE tracks
ADD COLUMN isrc TEXT;

ALTER TABLE tracks
ADD COLUMN initial_key TEXT;

ALTER TABLE tracks
ADD COLUMN mood TEXT;

ALTER TABLE tracks
ADD COLUMN work TEXT;

ALTER TABLE tracks
ADD COLUMN movement TEXT;

ALTER TABLE tracks
ADD COLUMN movement_number INTEGER;

ALTER TABLE tracks
ADD COLUMN movement_total INTEGER;

ALTER TABLE tracks
ADD COLUMN grouping TEXT;

ALTER TABLE tracks
ADD COLUMN language TEXT;

ALTER TABLE tracks
ADD COLUMN copyright TEXT;

ALTER TABLE tracks
ADD COLUMN musicbrainz_release_track_id TEXT;

-- Release properties that happen to be stored per-file. They reach `albums` through
-- `upsert_album`'s `COALESCE(excluded.x, albums.x)`, the same way `year` does, so the first track
-- of a release to carry one wins and a later track missing it doesn't blank it.
ALTER TABLE albums
ADD COLUMN label TEXT;

ALTER TABLE albums
ADD COLUMN catalog_number TEXT;

ALTER TABLE albums
ADD COLUMN barcode TEXT;

ALTER TABLE albums
ADD COLUMN media TEXT;

ALTER TABLE albums
ADD COLUMN release_type TEXT;

ALTER TABLE albums
ADD COLUMN release_country TEXT;

ALTER TABLE albums
ADD COLUMN musicbrainz_release_group_id TEXT;

-- Carry the two leaving columns across before they go. A library that has never been rescanned is
-- then correct as of today for what it already knew; the multi-value tags sitting unread in the
-- files need those files re-read, which is the one-shot sweep's job.
UPDATE tracks
SET
    credits = TRIM(composer)
WHERE
    composer IS NOT NULL
    AND TRIM(composer) <> '';

-- A composer becomes an `artists` row like any other credit, which is what makes them clickable
-- and searchable. `INSERT OR IGNORE` against the NOCASE unique index folds the duplicates.
INSERT
OR IGNORE INTO artists (name)
SELECT DISTINCT
    TRIM(composer)
FROM
    tracks
WHERE
    composer IS NOT NULL
    AND TRIM(composer) <> '';

INSERT INTO
    track_credits (track_id, artist_id, role, detail, position)
SELECT
    t.id,
    a.id,
    'composer',
    '',
    0
FROM
    tracks t
    JOIN artists a ON a.name = TRIM(t.composer)
WHERE
    t.composer IS NOT NULL
    AND TRIM(t.composer) <> '';

-- One label per release, taken from whichever of its tracks carried one. `albums.label` is the
-- only reader there will be, so a disagreement between tracks has no surface to show it on.
UPDATE albums
SET
    label = (
        SELECT
            TRIM(t.label)
        FROM
            tracks t
        WHERE
            t.album_id = albums.id
            AND t.label IS NOT NULL
            AND TRIM(t.label) <> ''
        LIMIT
            1
    );

-- Seed one genre per track from the FK, ahead of the triggers below so these rows are counted
-- once rather than here and again by the recompute at the end.
INSERT INTO
    track_genres (track_id, genre_id, position)
SELECT
    id,
    genre_id,
    0
FROM
    tracks
WHERE
    genre_id IS NOT NULL;

-- fts5 has no ALTER, so swapping `composer` for `credits` rebuilds the table and all three sync
-- triggers. They also have to go before the column does: an external-content table and its
-- triggers both name it, and `ALTER TABLE … DROP COLUMN` refuses while anything in the schema
-- does.
--
-- Same eight columns and the same bm25 weights, which are positional against the list — `credits`
-- takes `composer`'s slot and its tiebreaker weight, now covering every role rather than one.
-- The weights live in the `tracks_fts_config` shadow table, which only `DROP TABLE` takes with
-- it, so the INSERT below is re-issued rather than inherited.
DROP TRIGGER IF EXISTS tracks_fts_insert;

DROP TRIGGER IF EXISTS tracks_fts_delete;

DROP TRIGGER IF EXISTS tracks_fts_update;

DROP TABLE IF EXISTS tracks_fts;

ALTER TABLE tracks
DROP COLUMN composer;

ALTER TABLE tracks
DROP COLUMN label;

-- No IF NOT EXISTS, for `20260802000001`'s reason: the DROP above is unconditional, so the guard
-- could only fire if that DROP were reordered away — and then it would skip the CREATE silently
-- and apply the rank INSERT to the *old* table.
CREATE VIRTUAL TABLE tracks_fts USING fts5 (
    title,
    artist,
    album_artist,
    album,
    genre,
    credits,
    year,
    file_name,
    content = 'tracks',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

INSERT INTO
    tracks_fts (tracks_fts, rank)
VALUES
    ('rank', 'bm25(8.0, 6.0, 4.0, 4.0, 2.0, 2.0, 2.0, 0.5)');

CREATE TRIGGER IF NOT EXISTS tracks_fts_insert AFTER INSERT ON tracks BEGIN
INSERT INTO
    tracks_fts (
        rowid,
        title,
        artist,
        album_artist,
        album,
        genre,
        credits,
        year,
        file_name
    )
VALUES
    (
        new.id,
        new.title,
        new.artist,
        new.album_artist,
        new.album,
        new.genre,
        new.credits,
        new.year,
        new.file_name
    );

END;

CREATE TRIGGER IF NOT EXISTS tracks_fts_delete AFTER DELETE ON tracks BEGIN
INSERT INTO
    tracks_fts (
        tracks_fts,
        rowid,
        title,
        artist,
        album_artist,
        album,
        genre,
        credits,
        year,
        file_name
    )
VALUES
    (
        'delete',
        old.id,
        old.title,
        old.artist,
        old.album_artist,
        old.album,
        old.genre,
        old.credits,
        old.year,
        old.file_name
    );

END;

-- The UPDATE OF list has to name every indexed column or an edit touching only one of them leaves
-- the index stale; `update_track_metadata` sets all eight in a single statement.
CREATE TRIGGER IF NOT EXISTS tracks_fts_update AFTER
UPDATE OF title,
artist,
album_artist,
album,
genre,
credits,
year,
file_name ON tracks BEGIN
INSERT INTO
    tracks_fts (
        tracks_fts,
        rowid,
        title,
        artist,
        album_artist,
        album,
        genre,
        credits,
        year,
        file_name
    )
VALUES
    (
        'delete',
        old.id,
        old.title,
        old.artist,
        old.album_artist,
        old.album,
        old.genre,
        old.credits,
        old.year,
        old.file_name
    );

INSERT INTO
    tracks_fts (
        rowid,
        title,
        artist,
        album_artist,
        album,
        genre,
        credits,
        year,
        file_name
    )
VALUES
    (
        new.id,
        new.title,
        new.artist,
        new.album_artist,
        new.album,
        new.genre,
        new.credits,
        new.year,
        new.file_name
    );

END;

INSERT INTO
    tracks_fts (tracks_fts)
VALUES
    ('rebuild');

-- The genre half of the stats moves onto `track_genres`, the way the artist half moved onto
-- `track_artists`. `genre_id` leaves the watch list with it: it is the primary genre now, and
-- moving it re-points nothing a count depends on.
--
-- `queries/stats.rs` holds this same text and drops and recreates these three around bulk scan
-- chunks; the two copies have to move together.
DROP TRIGGER IF EXISTS tracks_stats_insert;

DROP TRIGGER IF EXISTS tracks_stats_delete;

DROP TRIGGER IF EXISTS tracks_stats_update;

CREATE TRIGGER IF NOT EXISTS tracks_stats_insert AFTER INSERT ON tracks BEGIN
UPDATE albums
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.album_id;

END;

CREATE TRIGGER IF NOT EXISTS tracks_stats_delete AFTER DELETE ON tracks BEGIN
UPDATE albums
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.album_id;

END;

CREATE TRIGGER IF NOT EXISTS tracks_stats_update AFTER
UPDATE OF album_id,
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

UPDATE genres
SET
    total_duration_ms = MAX(
        total_duration_ms + new.duration_ms - old.duration_ms,
        0
    )
WHERE
    id IN (
        SELECT
            genre_id
        FROM
            track_genres
        WHERE
            track_id = new.id
    );

UPDATE albums
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.album_id;

UPDATE albums
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.album_id;

END;

-- Both arms read `tracks.duration_ms` live, which is what the BEFORE DELETE cleanup buys them.
CREATE TRIGGER IF NOT EXISTS track_genres_stats_insert AFTER INSERT ON track_genres BEGIN
UPDATE genres
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
    id = new.genre_id;

END;

CREATE TRIGGER IF NOT EXISTS track_genres_stats_delete AFTER DELETE ON track_genres BEGIN
UPDATE genres
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
    id = old.genre_id;

END;

-- Recreated rather than left alone: the two new tables have the same no-cascade arrangement as
-- the credit tables, so a track delete has to clear them while the row is still readable or the
-- foreign key refuses it.
DROP TRIGGER IF EXISTS tracks_credits_cleanup;

CREATE TRIGGER IF NOT EXISTS tracks_credits_cleanup BEFORE DELETE ON tracks BEGIN
DELETE FROM track_artists
WHERE
    track_id = old.id;

DELETE FROM track_credits
WHERE
    track_id = old.id;

DELETE FROM track_genres
WHERE
    track_id = old.id;

END;

-- The genre seed ran with no triggers watching it, and the counts it should have produced are the
-- same ones the old triggers kept off `tracks.genre_id`. Recompute rather than reason about that.
UPDATE genres
SET
    track_count = COALESCE(
        (
            SELECT
                COUNT(*)
            FROM
                track_genres
            WHERE
                genre_id = genres.id
        ),
        0
    ),
    total_duration_ms = COALESCE(
        (
            SELECT
                SUM(t.duration_ms)
            FROM
                track_genres tg
                JOIN tracks t ON t.id = tg.track_id
            WHERE
                tg.genre_id = genres.id
        ),
        0
    );

-- The release-level columns join the view so the album detail page can reach them without a
-- second query, the way `is_compilation` and `disc_count` already do.
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
    al.musicbrainz_release_group_id,
    al.label,
    al.catalog_number,
    al.barcode,
    al.media,
    al.release_type,
    al.release_country,
    al.artwork_path,
    al.track_count,
    al.total_duration_ms
FROM
    albums al
    JOIN artists a ON al.artist_id = a.id
WHERE
    al.track_count > 0;
