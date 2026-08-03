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
--
-- `remove_diacritics 2` rather than the default 1, which SQLite keeps only
-- for backwards compatibility: mode 1 folds a character carrying a single
-- combining mark, so it already reaches Björk and Beyoncé, but it leaves the
-- two-mark characters of scripts like Vietnamese alone and an ASCII query for
-- one of those matches nothing.
--
-- The bm25 weights are positional against the column list, which is why they
-- sit under it rather than in `search.rs`. `file_name` normally repeats the
-- title and artist beside it ("01 - Artist - Title.flac"), so at the default
-- uniform weight it counts every match twice and can rank a filename echo
-- above the track whose title *is* the query. It stays indexed at a
-- tiebreaker weight because it is the only column carrying what the tags
-- don't -- the extension, a track-number prefix, a spelling the metadata
-- never had. (An untagged file is not that case: `extract_metadata` falls
-- back to the file stem for the title, so it already matches at full weight.)
-- The setting lives in the `tracks_fts_config` shadow table, which only
-- `DROP TABLE tracks_fts` takes with it -- the 'rebuild' at the bottom of this
-- file leaves it alone. A future migration that swaps the table has to
-- re-issue this INSERT.

DROP TRIGGER IF EXISTS tracks_fts_insert;

DROP TRIGGER IF EXISTS tracks_fts_delete;

DROP TRIGGER IF EXISTS tracks_fts_update;

DROP TABLE IF EXISTS tracks_fts;

-- No IF NOT EXISTS on the CREATE: the DROP above is unconditional, so the
-- guard could only ever fire if that DROP were reordered away -- and then it
-- would skip the CREATE silently and apply the rank INSERT below to the *old*
-- table, leaving the tokenizer unchanged while the migration still passes.
CREATE VIRTUAL TABLE tracks_fts USING fts5 (
    title,
    artist,
    album_artist,
    album,
    genre,
    composer,
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
