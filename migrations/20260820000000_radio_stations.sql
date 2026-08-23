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
CREATE TABLE
    IF NOT EXISTS radio_stations (
        id INTEGER PRIMARY KEY,
        station_uuid TEXT UNIQUE,
        name TEXT NOT NULL,
        stream_url TEXT NOT NULL,
        homepage TEXT,
        favicon_url TEXT,
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

-- Logo URLs that asked and got nothing back, so a browse doesn't spend the same
-- failed round trips every launch. A dead favicon is the normal condition on a
-- directory of community-maintained entries, and a host that hangs costs the
-- full timeout each time a page carrying it is drawn.
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
-- No index: the lookup is by primary key, and the sweep is a full scan on a table
-- this small either way.
CREATE TABLE
    IF NOT EXISTS radio_logo_misses (
        favicon_url TEXT PRIMARY KEY,
        attempts INTEGER NOT NULL DEFAULT 1,
        retry_after TEXT NOT NULL
    );
