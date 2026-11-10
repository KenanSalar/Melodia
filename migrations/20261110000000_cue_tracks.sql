-- foreign-keys-off
-- A cue sheet splits one file into a row per track, so a track is keyed on its file and the
-- sheet's track number. `tracks.file_path`'s inline UNIQUE is backed by an index nothing can drop,
-- hence the rebuild.
--
-- The line above has the migrator run this with foreign keys off. With them on, the DROP TABLE
-- below empties `tracks` first, cascading into every playlist entry and dismissal, and fails on
-- the credit rows, which don't cascade. The migrator checks every reference before it records the
-- migration.
--
-- Ids are copied, so `tracks_fts`, external content over `id`, stays valid as it is.
CREATE TABLE cue_sheets (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    folder_id INTEGER NOT NULL REFERENCES folders (id) ON DELETE CASCADE,
    -- The stamp a scan compares, as a track's own is.
    file_size INTEGER,
    date_modified TEXT
);

-- Every trigger naming `tracks`: the rename below re-parses the whole schema, and one on another
-- table still naming the dropped `tracks` fails it.
DROP TRIGGER playlist_items_stats_delete;

DROP TRIGGER playlist_items_stats_insert;

DROP TRIGGER track_artists_stats_delete;

DROP TRIGGER track_artists_stats_insert;

DROP TRIGGER track_genres_stats_delete;

DROP TRIGGER track_genres_stats_insert;

DROP TRIGGER tracks_credits_cleanup;

DROP TRIGGER tracks_delete_playlist_stats;

DROP TRIGGER tracks_duration_update_playlists;

DROP TRIGGER tracks_fts_delete;

DROP TRIGGER tracks_fts_insert;

DROP TRIGGER tracks_fts_update;

DROP TRIGGER tracks_stats_delete;

DROP TRIGGER tracks_stats_insert;

DROP TRIGGER tracks_stats_update;

CREATE TABLE tracks_new (
    id INTEGER PRIMARY KEY,
    file_path TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_hash TEXT,
    -- The sheet's TRACK number, 0 for audio hidden before track 1, and where the track starts and
    -- ends in cue frames (1/75 s), a NULL end playing to the end of the file. All NULL on a row
    -- playing its whole file, and the sheet NULL for one embedded in the file.
    cue_track INTEGER,
    cue_start INTEGER,
    cue_end INTEGER,
    cue_sheet_id INTEGER REFERENCES cue_sheets (id) ON DELETE CASCADE,
    -- Core metadata
    title TEXT NOT NULL,
    artist TEXT,
    album_artist TEXT,
    album TEXT,
    genre TEXT,
    credits TEXT,
    track_number INTEGER,
    track_total INTEGER,
    disc_number INTEGER,
    disc_total INTEGER,
    disc_subtitle TEXT,
    subtitle TEXT,
    release_date TEXT,
    year INTEGER,
    original_date TEXT,
    original_year INTEGER,
    comment TEXT,
    -- Extended metadata
    bpm REAL,
    initial_key TEXT,
    mood TEXT,
    grouping TEXT,
    work TEXT,
    movement TEXT,
    movement_number INTEGER,
    movement_total INTEGER,
    language TEXT,
    copyright TEXT,
    isrc TEXT,
    musicbrainz_track_id TEXT,
    musicbrainz_release_id TEXT,
    musicbrainz_release_track_id TEXT,
    -- ReplayGain
    replaygain_track_gain REAL,
    replaygain_track_peak REAL,
    replaygain_album_gain REAL,
    replaygain_album_peak REAL,
    -- Technical info
    duration_ms INTEGER NOT NULL DEFAULT 0,
    file_size INTEGER,
    codec TEXT,
    bitrate INTEGER,
    channels INTEGER,
    sample_rate INTEGER,
    bit_depth INTEGER,
    -- Artwork
    artwork_path TEXT,
    -- Playback state
    play_count INTEGER NOT NULL DEFAULT 0,
    skip_count INTEGER NOT NULL DEFAULT 0,
    rating INTEGER NOT NULL DEFAULT 0,
    is_favorite BOOLEAN NOT NULL DEFAULT FALSE,
    last_played TEXT,
    last_position INTEGER NOT NULL DEFAULT 0,
    -- Relations
    album_id INTEGER REFERENCES albums (id) ON DELETE SET NULL,
    artist_id INTEGER REFERENCES artists (id) ON DELETE SET NULL,
    genre_id INTEGER REFERENCES genres (id) ON DELETE SET NULL,
    folder_id INTEGER NOT NULL REFERENCES folders (id) ON DELETE CASCADE,
    -- Timestamps
    date_added TEXT NOT NULL,
    date_modified TEXT,
    -- Derived columns
    sort_key TEXT,
    search_text TEXT NOT NULL DEFAULT '',
    artist_key TEXT,
    album_key TEXT,
    genre_key TEXT,
    UNIQUE (file_path, cue_track),
    CHECK ((cue_track IS NULL) = (cue_start IS NULL)),
    CHECK (cue_track BETWEEN 0 AND 99)
);

INSERT INTO
    tracks_new (
        id,
        file_path,
        file_name,
        file_hash,
        title,
        artist,
        album_artist,
        album,
        genre,
        credits,
        track_number,
        track_total,
        disc_number,
        disc_total,
        disc_subtitle,
        subtitle,
        release_date,
        year,
        original_date,
        original_year,
        comment,
        bpm,
        initial_key,
        mood,
        grouping,
        work,
        movement,
        movement_number,
        movement_total,
        language,
        copyright,
        isrc,
        musicbrainz_track_id,
        musicbrainz_release_id,
        musicbrainz_release_track_id,
        replaygain_track_gain,
        replaygain_track_peak,
        replaygain_album_gain,
        replaygain_album_peak,
        duration_ms,
        file_size,
        codec,
        bitrate,
        channels,
        sample_rate,
        bit_depth,
        artwork_path,
        play_count,
        skip_count,
        rating,
        is_favorite,
        last_played,
        last_position,
        album_id,
        artist_id,
        genre_id,
        folder_id,
        date_added,
        date_modified,
        sort_key,
        search_text,
        artist_key,
        album_key,
        genre_key
    )
SELECT
    id,
    file_path,
    file_name,
    file_hash,
    title,
    artist,
    album_artist,
    album,
    genre,
    credits,
    track_number,
    track_total,
    disc_number,
    disc_total,
    disc_subtitle,
    subtitle,
    release_date,
    year,
    original_date,
    original_year,
    comment,
    bpm,
    initial_key,
    mood,
    grouping,
    work,
    movement,
    movement_number,
    movement_total,
    language,
    copyright,
    isrc,
    musicbrainz_track_id,
    musicbrainz_release_id,
    musicbrainz_release_track_id,
    replaygain_track_gain,
    replaygain_track_peak,
    replaygain_album_gain,
    replaygain_album_peak,
    duration_ms,
    file_size,
    codec,
    bitrate,
    channels,
    sample_rate,
    bit_depth,
    artwork_path,
    play_count,
    skip_count,
    rating,
    is_favorite,
    last_played,
    last_position,
    album_id,
    artist_id,
    genre_id,
    folder_id,
    date_added,
    date_modified,
    sort_key,
    search_text,
    artist_key,
    album_key,
    genre_key
FROM
    tracks;

DROP TABLE tracks;

ALTER TABLE tracks_new RENAME TO tracks;

-- NULLs are distinct to the table's UNIQUE, so this is what keeps a file to one whole-file row.
CREATE UNIQUE INDEX idx_tracks_whole_file ON tracks (file_path)
WHERE cue_track IS NULL;

-- Deleting a sheet looks up its tracks.
CREATE INDEX idx_tracks_cue_sheet_id ON tracks (cue_sheet_id)
WHERE cue_sheet_id IS NOT NULL;

-- The rest as they stood.
CREATE INDEX idx_tracks_album_disc_track ON tracks (album_id, disc_number, track_number);

CREATE INDEX idx_tracks_album_id ON tracks (album_id);

CREATE INDEX idx_tracks_album_key ON tracks (album_key, sort_key COLLATE NOCASE);

CREATE INDEX idx_tracks_artist_album ON tracks (artist_id, album_id);

CREATE INDEX idx_tracks_artist_key ON tracks (artist_key, sort_key COLLATE NOCASE);

CREATE INDEX idx_tracks_date_added ON tracks (date_added);

CREATE INDEX idx_tracks_file_hash ON tracks (file_hash) WHERE file_hash IS NOT NULL;

CREATE INDEX idx_tracks_folder_id ON tracks (folder_id);

CREATE INDEX idx_tracks_genre_id ON tracks (genre_id);

CREATE INDEX idx_tracks_genre_key ON tracks (genre_key, sort_key COLLATE NOCASE);

CREATE INDEX idx_tracks_is_favorite ON tracks (is_favorite)
WHERE
    is_favorite = TRUE;

CREATE INDEX idx_tracks_last_played ON tracks (last_played)
WHERE
    last_played IS NOT NULL;

CREATE INDEX idx_tracks_length_order
    ON tracks (MIN(MAX(duration_ms, 0), 2147483647), sort_key COLLATE NOCASE);

CREATE INDEX idx_tracks_play_count ON tracks (play_count)
WHERE
    play_count > 0;

CREATE INDEX idx_tracks_rating ON tracks (rating);

CREATE INDEX idx_tracks_search_text_unfilled ON tracks (id) WHERE search_text = '';

CREATE INDEX idx_tracks_sort_key ON tracks (sort_key COLLATE NOCASE);

CREATE INDEX idx_tracks_year_order
    ON tracks (COALESCE(year, 0), sort_key COLLATE NOCASE);

-- The triggers as they stood. `queries/stats.rs` holds the text of the `*_stats_*` ones and
-- recreates them around bulk scan chunks, so the two copies move together.
CREATE TRIGGER playlist_items_stats_delete AFTER DELETE ON playlist_items BEGIN
UPDATE playlists
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
    id = old.playlist_id;

END;

CREATE TRIGGER playlist_items_stats_insert AFTER INSERT ON playlist_items BEGIN
UPDATE playlists
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
    id = new.playlist_id;

END;

CREATE TRIGGER track_artists_stats_delete AFTER DELETE ON track_artists BEGIN
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

CREATE TRIGGER track_artists_stats_insert AFTER INSERT ON track_artists BEGIN
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

CREATE TRIGGER track_genres_stats_delete AFTER DELETE ON track_genres BEGIN
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

CREATE TRIGGER track_genres_stats_insert AFTER INSERT ON track_genres BEGIN
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

CREATE TRIGGER tracks_credits_cleanup BEFORE DELETE ON tracks BEGIN
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

CREATE TRIGGER tracks_delete_playlist_stats BEFORE DELETE ON tracks BEGIN
UPDATE playlists
SET
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id IN (
        SELECT
            playlist_id
        FROM
            playlist_items
        WHERE
            track_id = old.id
    );

END;

CREATE TRIGGER tracks_duration_update_playlists AFTER
UPDATE OF duration_ms ON tracks BEGIN
UPDATE playlists
SET
    total_duration_ms = total_duration_ms - old.duration_ms + new.duration_ms
WHERE
    id IN (
        SELECT
            playlist_id
        FROM
            playlist_items
        WHERE
            track_id = new.id
    );

END;

CREATE TRIGGER tracks_fts_delete AFTER DELETE ON tracks BEGIN
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

CREATE TRIGGER tracks_fts_insert AFTER INSERT ON tracks BEGIN
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

CREATE TRIGGER tracks_fts_update AFTER
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

CREATE TRIGGER tracks_stats_delete AFTER DELETE ON tracks BEGIN
UPDATE albums
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.album_id;

END;

CREATE TRIGGER tracks_stats_insert AFTER INSERT ON tracks BEGIN
UPDATE albums
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.album_id;

END;

CREATE TRIGGER tracks_stats_update AFTER
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

-- The DROP TABLE took the planner's statistics with it.
ANALYZE tracks;
