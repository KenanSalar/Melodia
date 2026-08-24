-- Internet radio stations. Favorites, hand-typed URLs and play history are one
-- row at different points in its life rather than three tables: a station the
-- user typed in, one they favorited out of the directory and one they merely
-- played differ by station_uuid, is_favorite and last_played, and any of them
-- can become another without moving between tables.
--
-- station_uuid is radio-browser.info's id, NULL for a hand-typed URL. UNIQUE is
-- what the directory upsert conflicts on, and SQLite backs the constraint with
-- an index, so the by-uuid lookup needs no second one. SQLite also treats NULLs
-- as distinct under UNIQUE, which is why a custom station is a plain INSERT and
-- never that upsert: it would conflict with nothing and duplicate on every call.
--
-- No secondary indexes. Favorites and recents are the only list queries and both
-- read a table the user fills by hand, where a scan beats a seek and an index is
-- a cost on the write that follows every play. The tracks table is the other
-- shape, and its history runs both ways: idx_tracks_last_played was dropped
-- there as write-only and had to come back the moment Recently Played existed.
-- Adding one here is additive, so it waits for the surface that wants it.
--
-- sort_key is the to_natural_sort_key column tracks already carries, stored for
-- the ordering semantics rather than for speed. It is what puts "Radio 2" ahead
-- of "Radio 10", which ORDER BY name does not.
--
-- hls is stored rather than re-read from the directory because a favorited HLS
-- station has left the directory behind. Symphonia has no MPEG-TS demuxer, so
-- this column is the only thing left that can say the stream is unplayable.
--
-- country sits beside country_code because they answer different questions: the
-- code is what the search endpoint filters on, the name is what a card shows.
-- Deriving one from the other would mean shipping an ISO table for a string the
-- directory already hands over, and a kept station rendering "DE" where the same
-- card on Browse renders "Germany" reads as a bug.
--
-- The four local_* columns are the user's answers to what the directory left
-- blank, and they are separate columns rather than edits to the originals
-- because the two have different writers: homepage, tags, country and
-- favicon_url are rewritten wholesale by the directory on every re-import,
-- which any station gets on its next play. Folded together it is a choice
-- between the re-import blanking what the user typed and the directory never
-- being able to correct a field that changed; kept apart, both hold, each column
-- has exactly one writer, and a reader just takes the local one first.
--
-- They exist because the directory's entries are community-maintained and
-- frequently partial: roughly one in fifteen carries no homepage, a third no
-- favicon, and a hand-typed station has no country or genre at all — a stream
-- announces neither, and guessing from the host is wrong more often than blank.
-- Nothing can be derived from a stream URL either, that host usually belonging
-- to a streaming provider rather than to the station.
--
-- Keeping the pair also decides who may edit: a field is offered only where the
-- *directory* said nothing, so a value it did supply is never one click from
-- being overwritten by a typo. See `RadioStation::can_override`.
CREATE TABLE
    IF NOT EXISTS radio_stations (
        id INTEGER PRIMARY KEY,
        station_uuid TEXT UNIQUE,
        name TEXT NOT NULL,
        stream_url TEXT NOT NULL,
        homepage TEXT,
        local_homepage TEXT,
        favicon_url TEXT,
        local_favicon_url TEXT,
        local_tags TEXT,
        local_country TEXT,
        artwork_path TEXT,
        tags TEXT NOT NULL DEFAULT '',
        country TEXT NOT NULL DEFAULT '',
        country_code TEXT NOT NULL DEFAULT '',
        language TEXT NOT NULL DEFAULT '',
        codec TEXT NOT NULL DEFAULT '',
        bitrate INTEGER NOT NULL DEFAULT 0,
        hls BOOLEAN NOT NULL DEFAULT FALSE,
        is_favorite BOOLEAN NOT NULL DEFAULT FALSE,
        sort_key TEXT NOT NULL DEFAULT '',
        date_added TEXT NOT NULL,
        last_played TEXT,
        play_count INTEGER NOT NULL DEFAULT 0
    );

-- What each logo URL answered last time it was asked, so a browse doesn't spend
-- the same round trips every launch. Both outcomes, in one row per URL, because
-- a URL has one answer: artwork_path names the stored file for a hit and is NULL
-- for a miss, exactly as the session memo holds an Option. Two tables would mean
-- two queries on the path a page of fifty stations takes, and a URL able to be
-- in both.
--
-- The hit half is what makes the store a cache rather than a spool. The file is
-- named by a hash of its own bytes, so nothing can know a URL's path without
-- downloading the bytes first -- which meant every browsed logo was re-fetched
-- on every launch and rewritten identically, and the files in between were never
-- read by anything.
--
-- Keyed on the URL rather than on the station: a station repointed at a new logo
-- has to be asked again, and the URL is what changed. That also means the table
-- describes our own network outcomes rather than directory rows, which is why it
-- can exist at all next to the rule that browsed stations are never persisted.
--
-- attempts drives the backoff, so a host down for an afternoon is retried and one
-- gone for good is asked at a rate that rounds to never. The answer is stored as
-- the retry time rather than the attempt time so the read is a string comparison
-- against the clock, the idiom the smart-playlist date rules already use: SQLite's
-- date functions would have to parse chrono's nanosecond RFC3339 to do the same
-- arithmetic here, and `library::radio` is where that policy belongs anyway.
--
-- bytes is what the file cost, carried on the row so the retention pass can hold
-- the cache to a size without stat-ing a directory to find out what it is holding.
-- answered_at dates the row for the same pass. Both are zero and clock-stamped on
-- a miss, which costs nothing and keeps one shape for the row.
--
-- No index: the page lookup is by primary key, and the retention pass is a full
-- scan on a table this small either way.
CREATE TABLE
    IF NOT EXISTS radio_logo_answers (
        favicon_url TEXT PRIMARY KEY,
        artwork_path TEXT,
        bytes INTEGER NOT NULL DEFAULT 0,
        attempts INTEGER NOT NULL DEFAULT 0,
        retry_after TEXT,
        answered_at TEXT NOT NULL
    );
