-- The tag vocabulary: multi-artist credits, role credits, genres, and the columns nothing wrote.
--
-- Two halves, and the seam is marked below. The first makes a track's artist a set of rows; the
-- second gives role credits and genres the same shape and fills in the rest of what a tag file
-- can say. One migration rather than two because the second supersedes five objects the first
-- would otherwise create and immediately throw away — the three `tracks_stats_*` triggers, the
-- `album_stats` view and `tracks_credits_cleanup` — and every install would run all ten
-- statements to arrive where one set leaves it.
--
-- Order is load-bearing throughout. A trigger follows the tables it reads, the `tracks_fts`
-- rebuild precedes the `DROP COLUMN`s whose names its old column list still resolves, and every
-- seeding pass runs before the triggers that would otherwise count it twice.
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

-- The three `tracks_stats_*` triggers are dropped here and put back near the end of this file,
-- once `track_genres` exists and the genre arms can read it. Nothing in between inserts into or
-- deletes from `tracks`, and the one `UPDATE tracks` names a column no watch list carries, so the
-- window where none of them exists is a window in which none of them would have fired.

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
-- These are never dropped, unlike the stats triggers: without them a parent delete fails the
-- foreign key outright. `tracks_credits_cleanup` is created further down, once it has all three
-- child tables to sweep; nothing deletes a track before then.
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

-- `album_stats` gains `artist_credit` and the release columns together, so it is rebuilt once at
-- the end of this file rather than here and again after the `albums` columns land.

-- The rest of the tag vocabulary.
--
-- The half above made a track's artist a set of rows. Two more fields are multi-valued by the
-- same conventions and were still single: a role credit (composer, conductor, producer, …) and a
-- genre. Both get the shape established above — a join table carrying order, the rendered line
-- kept on `tracks` so every display surface and the FTS index read one column, and the primary FK
-- kept for grouping.
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

-- The three dropped near the top of this file, put back now that `track_genres` exists. The genre
-- half of the stats reads it the way the artist half reads `track_artists`, and `genre_id` leaves
-- the watch list with it: it is the primary genre now, and moving it re-points nothing a count
-- depends on.
--
-- `queries/stats.rs` holds this same text and drops and recreates these three around bulk scan
-- chunks; the two copies have to move together.
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

-- Here rather than beside `albums_credits_cleanup` above, so it is written once with all three
-- child tables in hand: they share the credit tables' no-cascade arrangement, so a track delete
-- has to clear them while the row is still readable or the foreign key refuses it.
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
