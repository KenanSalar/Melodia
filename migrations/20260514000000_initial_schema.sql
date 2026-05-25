-- Melodia database schema — squashed pre-release baseline.
-- Audio-only music player, INTEGER primary keys, no video support.
-- PRAGMAs are configured via SqliteConnectOptions, not here.
--
-- This file is the single source of truth for a fresh database. It folds in
-- every pre-release migration that previously lived as a separate file. After
-- the app ships, never squash again — each update gets its own additive
-- migration so existing user databases upgrade in place.
CREATE TABLE
    IF NOT EXISTS folders (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        is_enabled BOOLEAN NOT NULL DEFAULT TRUE,
        last_scanned TEXT,
        added_at TEXT NOT NULL
    );

CREATE TABLE
    IF NOT EXISTS artists (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE COLLATE NOCASE,
        sort_name TEXT,
        musicbrainz_id TEXT,
        image_path TEXT,
        -- Denormalized stats (maintained by triggers on tracks)
        track_count INTEGER NOT NULL DEFAULT 0,
        album_count INTEGER NOT NULL DEFAULT 0,
        total_duration_ms INTEGER NOT NULL DEFAULT 0
    );

-- Sentinel artist for tracks/albums with no known artist.
-- Uses a fixed id so Rust code can reference it as a constant.
INSERT
OR IGNORE INTO artists (id, name, sort_name)
VALUES
    (1, 'Unknown Artist', 'Unknown Artist');

CREATE TABLE
    IF NOT EXISTS albums (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        sort_name TEXT,
        artist_id INTEGER NOT NULL DEFAULT 1 REFERENCES artists (id) ON DELETE SET DEFAULT,
        year INTEGER,
        disc_count INTEGER,
        is_compilation BOOLEAN NOT NULL DEFAULT FALSE,
        musicbrainz_id TEXT,
        artwork_path TEXT,
        -- Denormalized stats (maintained by triggers on tracks)
        track_count INTEGER NOT NULL DEFAULT 0,
        total_duration_ms INTEGER NOT NULL DEFAULT 0
    );

CREATE UNIQUE INDEX IF NOT EXISTS idx_albums_name_artist ON albums (name COLLATE NOCASE, artist_id);

-- Index on artist_id alone — the composite unique index (name, artist_id)
-- cannot serve queries filtering by artist_id only (e.g. stat triggers).
CREATE INDEX IF NOT EXISTS idx_albums_artist_id ON albums (artist_id);

CREATE TABLE
    IF NOT EXISTS genres (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE COLLATE NOCASE,
        -- Denormalized stats (maintained by triggers on tracks)
        track_count INTEGER NOT NULL DEFAULT 0,
        total_duration_ms INTEGER NOT NULL DEFAULT 0
    );

CREATE TABLE
    IF NOT EXISTS tracks (
        id INTEGER PRIMARY KEY,
        file_path TEXT NOT NULL UNIQUE,
        file_name TEXT NOT NULL,
        file_hash TEXT,
        -- Core metadata
        title TEXT NOT NULL,
        artist TEXT,
        album_artist TEXT,
        album TEXT,
        genre TEXT,
        track_number INTEGER,
        disc_number INTEGER,
        year INTEGER,
        composer TEXT,
        comment TEXT,
        -- Extended metadata
        bpm REAL,
        musicbrainz_track_id TEXT,
        musicbrainz_release_id TEXT,
        label TEXT,
        original_year INTEGER,
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
        -- Precomputed natural sort key (numeric segments zero-padded for correct ORDER BY)
        sort_key TEXT
    );

-- Single-column indexes on tracks
CREATE INDEX IF NOT EXISTS idx_tracks_album_id ON tracks (album_id);

CREATE INDEX IF NOT EXISTS idx_tracks_artist_id ON tracks (artist_id);

CREATE INDEX IF NOT EXISTS idx_tracks_genre_id ON tracks (genre_id);

CREATE INDEX IF NOT EXISTS idx_tracks_folder_id ON tracks (folder_id);

CREATE INDEX IF NOT EXISTS idx_tracks_date_added ON tracks (date_added);

CREATE INDEX IF NOT EXISTS idx_tracks_is_favorite ON tracks (is_favorite)
WHERE
    is_favorite = TRUE;

-- Partial composite indexes for the Favorites view's per-album / per-artist
-- favorite-count aggregation (`get_favorite_albums` / `get_favorite_artists`).
-- They carry the FK column, so the `JOIN ... AND is_favorite = TRUE ... GROUP
-- BY` drives straight off the (small) favorites subset instead of scanning
-- every album/artist. Cost scales with favorites count, not library size.
CREATE INDEX IF NOT EXISTS idx_tracks_fav_album ON tracks (album_id)
WHERE
    is_favorite = TRUE;

CREATE INDEX IF NOT EXISTS idx_tracks_fav_artist ON tracks (artist_id)
WHERE
    is_favorite = TRUE;

CREATE INDEX IF NOT EXISTS idx_tracks_last_played ON tracks (last_played)
WHERE
    last_played IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_tracks_rating ON tracks (rating)
WHERE
    rating > 0;

CREATE INDEX IF NOT EXISTS idx_tracks_play_count ON tracks (play_count)
WHERE
    play_count > 0;

CREATE INDEX IF NOT EXISTS idx_tracks_sort_key ON tracks (sort_key COLLATE NOCASE);

-- Partial index on file_hash for moved-file detection and deduplication
-- lookups. Only indexes non-NULL hashes (tracks may have NULL until the
-- retroactive hashing background task populates them).
CREATE INDEX IF NOT EXISTS idx_tracks_file_hash ON tracks (file_hash) WHERE file_hash IS NOT NULL;

-- Composite indexes on tracks
CREATE INDEX IF NOT EXISTS idx_tracks_artist_album ON tracks (artist_id, album_id);

CREATE INDEX IF NOT EXISTS idx_tracks_album_disc_track ON tracks (album_id, disc_number, track_number);

CREATE TABLE
    IF NOT EXISTS playlists (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        description TEXT,
        thumbnail_path TEXT,
        is_smart BOOLEAN NOT NULL DEFAULT FALSE,
        smart_criteria TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        custom_thumbnail BOOLEAN NOT NULL DEFAULT FALSE,
        -- Denormalized stats (maintained by triggers on playlist_items)
        track_count INTEGER NOT NULL DEFAULT 0,
        total_duration_ms INTEGER NOT NULL DEFAULT 0
    );

CREATE TABLE
    IF NOT EXISTS playlist_items (
        id INTEGER PRIMARY KEY,
        playlist_id INTEGER NOT NULL REFERENCES playlists (id) ON DELETE CASCADE,
        track_id INTEGER NOT NULL REFERENCES tracks (id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        added_at TEXT NOT NULL
    );

CREATE INDEX IF NOT EXISTS idx_playlist_items_playlist ON playlist_items (playlist_id, position);

CREATE UNIQUE INDEX IF NOT EXISTS idx_playlist_items_unique ON playlist_items (playlist_id, track_id);

CREATE INDEX IF NOT EXISTS idx_playlist_items_track_id ON playlist_items (track_id);

-- Full-text search index for tracks (title, artist, album, file_name)
CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5 (
    title,
    artist,
    album,
    file_name,
    content = 'tracks',
    content_rowid = 'id'
);

-- Keep FTS index in sync with tracks table
CREATE TRIGGER IF NOT EXISTS tracks_fts_insert AFTER INSERT ON tracks BEGIN
INSERT INTO
    tracks_fts (rowid, title, artist, album, file_name)
VALUES
    (
        new.id,
        new.title,
        new.artist,
        new.album,
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
        album,
        file_name
    )
VALUES
    (
        'delete',
        old.id,
        old.title,
        old.artist,
        old.album,
        old.file_name
    );

END;

CREATE TRIGGER IF NOT EXISTS tracks_fts_update AFTER
UPDATE OF title,
artist,
album,
file_name ON tracks BEGIN
INSERT INTO
    tracks_fts (
        tracks_fts,
        rowid,
        title,
        artist,
        album,
        file_name
    )
VALUES
    (
        'delete',
        old.id,
        old.title,
        old.artist,
        old.album,
        old.file_name
    );

INSERT INTO
    tracks_fts (rowid, title, artist, album, file_name)
VALUES
    (
        new.id,
        new.title,
        new.artist,
        new.album,
        new.file_name
    );

END;

-- Views for stats (read denormalized columns — O(1) per row instead of JOIN + GROUP BY).
-- Entities with zero tracks are filtered out: when the file watcher deletes
-- tracks, the stats triggers decrement track_count but do not remove the
-- parent row. The WHERE clause hides empty entities everywhere without
-- needing cleanup code.
CREATE VIEW
    IF NOT EXISTS artist_stats AS
SELECT
    a.id,
    a.name,
    a.sort_name,
    a.musicbrainz_id,
    a.image_path,
    a.track_count,
    a.album_count,
    a.total_duration_ms
FROM
    artists a
WHERE
    a.track_count > 0;

CREATE VIEW
    IF NOT EXISTS album_stats AS
SELECT
    al.id,
    al.name,
    al.sort_name,
    al.artist_id,
    a.name AS artist_name,
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

CREATE VIEW
    IF NOT EXISTS genre_stats AS
SELECT
    g.id,
    g.name,
    g.track_count,
    g.total_duration_ms
FROM
    genres g
WHERE
    g.track_count > 0;

CREATE VIEW
    IF NOT EXISTS playlist_stats AS
SELECT
    p.id,
    p.name,
    p.description,
    p.thumbnail_path,
    p.is_smart,
    p.smart_criteria,
    p.created_at,
    p.updated_at,
    p.custom_thumbnail,
    p.track_count,
    p.total_duration_ms
FROM
    playlists p;

-- Triggers to maintain denormalized stats on artists, albums, genres
CREATE TRIGGER IF NOT EXISTS tracks_stats_insert AFTER INSERT ON tracks BEGIN
UPDATE artists
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.artist_id;

UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            albums
        WHERE
            artist_id = new.artist_id
    )
WHERE
    id = new.artist_id;

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
UPDATE artists
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.artist_id;

UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            albums
        WHERE
            artist_id = old.artist_id
    )
WHERE
    id = old.artist_id;

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

CREATE TRIGGER IF NOT EXISTS tracks_stats_update AFTER
UPDATE OF artist_id,
album_id,
genre_id,
duration_ms ON tracks BEGIN
-- Decrement old parent stats
UPDATE artists
SET
    track_count = MAX(track_count - 1, 0),
    total_duration_ms = MAX(total_duration_ms - old.duration_ms, 0)
WHERE
    id = old.artist_id;

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

-- Increment new parent stats
UPDATE artists
SET
    track_count = track_count + 1,
    total_duration_ms = total_duration_ms + new.duration_ms
WHERE
    id = new.artist_id;

UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            albums
        WHERE
            artist_id = old.artist_id
    )
WHERE
    id = old.artist_id;

UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            albums
        WHERE
            artist_id = new.artist_id
    )
WHERE
    id = new.artist_id;

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

-- Recalculate album_count when albums are reassigned between artists (e.g., artist deletion)
CREATE TRIGGER IF NOT EXISTS albums_artist_update AFTER
UPDATE OF artist_id ON albums BEGIN
UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            albums
        WHERE
            artist_id = old.artist_id
    )
WHERE
    id = old.artist_id;

UPDATE artists
SET
    album_count = (
        SELECT
            COUNT(*)
        FROM
            albums
        WHERE
            artist_id = new.artist_id
    )
WHERE
    id = new.artist_id;

END;

-- Triggers to maintain denormalized stats on playlists
CREATE TRIGGER IF NOT EXISTS playlist_items_stats_insert AFTER INSERT ON playlist_items BEGIN
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

-- AFTER DELETE: handles direct playlist item removal (track still exists in DB).
-- During CASCADE from track deletion, the track row is already gone, so COALESCE
-- returns 0 for duration — but that's fine because tracks_delete_playlist_stats
-- (BEFORE DELETE on tracks) already handled the duration decrement.
CREATE TRIGGER IF NOT EXISTS playlist_items_stats_delete AFTER DELETE ON playlist_items BEGIN
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

-- Handle playlist stats BEFORE track deletion (track data still accessible).
-- When a track is deleted, FK CASCADE will also delete playlist_items rows, which
-- fires playlist_items_stats_delete above — but at that point the track is gone,
-- so only track_count is decremented there (duration COALESCE returns 0).
-- This trigger ensures total_duration_ms is correctly decremented while the track exists.
CREATE TRIGGER IF NOT EXISTS tracks_delete_playlist_stats BEFORE DELETE ON tracks BEGIN
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

-- Update playlist duration when a track's duration changes (e.g., rescan)
CREATE TRIGGER IF NOT EXISTS tracks_duration_update_playlists AFTER
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
