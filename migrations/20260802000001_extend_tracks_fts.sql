-- Extend the tracks full-text index to the metadata the track list actually
-- shows. It was created over (title, artist, album, file_name), so a query
-- for a genre or a year matched nothing even though both are columns of
-- every track list in the app. album_artist and composer join them: both are
-- editable in the Edit-Tags dialog, and both are what a user reaches for
-- when the performer and the credited artist differ.
--
-- fts5 has no ALTER, so the table and its three sync triggers are rebuilt.
-- 'rebuild' repopulates from the content table, so nothing is re-read from
-- disk — the upgrade cost is one tokenizer pass over rows SQLite already
-- holds.
--
-- `year` is an INTEGER column and fts5 indexes its text form, so "1998"
-- matches that year and the prefix search the app builds makes "199" match
-- the decade.
--
-- The AFTER UPDATE OF list has to name every indexed column or a tag edit
-- that touches only one of the new ones leaves the index stale;
-- `update_track_metadata` sets all eight in a single statement.

DROP TRIGGER IF EXISTS tracks_fts_insert;

DROP TRIGGER IF EXISTS tracks_fts_delete;

DROP TRIGGER IF EXISTS tracks_fts_update;

DROP TABLE IF EXISTS tracks_fts;

CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5 (
    title,
    artist,
    album_artist,
    album,
    genre,
    composer,
    year,
    file_name,
    content = 'tracks',
    content_rowid = 'id'
);

CREATE TRIGGER IF NOT EXISTS tracks_fts_insert AFTER INSERT ON tracks BEGIN
INSERT INTO
    tracks_fts (
        rowid,
        title,
        artist,
        album_artist,
        album,
        genre,
        composer,
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
        new.composer,
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
        composer,
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
        old.composer,
        old.year,
        old.file_name
    );

END;

CREATE TRIGGER IF NOT EXISTS tracks_fts_update AFTER
UPDATE OF title,
artist,
album_artist,
album,
genre,
composer,
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
        composer,
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
        old.composer,
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
        composer,
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
        new.composer,
        new.year,
        new.file_name
    );

END;

INSERT INTO
    tracks_fts (tracks_fts)
VALUES
    ('rebuild');
