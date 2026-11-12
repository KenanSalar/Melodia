-- A track's loudness as Melodia measured it, for a file carrying no gain tags: apart from the tag
-- columns, which every re-read of the file rewrites. file_hash, cue_start and cue_end are what was
-- measured, and a row applies only while they match the track's (queries::loudness).
CREATE TABLE IF NOT EXISTS track_loudness (
    track_id INTEGER PRIMARY KEY REFERENCES tracks (id) ON DELETE CASCADE,
    file_hash TEXT NOT NULL,
    cue_start INTEGER,
    cue_end INTEGER,
    -- NULL where the audio held nothing to level: silent, too short or unreadable.
    track_gain REAL,
    track_peak REAL,
    -- Set together once the whole album was measured, with the album and its member count then.
    -- No foreign key: an album's delete would scan this table for want of an index.
    album_id INTEGER,
    album_tracks INTEGER,
    album_gain REAL,
    album_peak REAL
);
