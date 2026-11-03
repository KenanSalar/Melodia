-- The copies the duplicate finder was told are not duplicates. A group stays out of its list while
-- every copy in it carries one set_id, so a copy that joins later, carrying none, shows it again.
--
-- Keyed by track id rather than path because a row keeps its id through a rename and a move, and a
-- removed copy's entry cascades away with it.
CREATE TABLE IF NOT EXISTS duplicate_dismissals (
    track_id INTEGER PRIMARY KEY REFERENCES tracks (id) ON DELETE CASCADE,
    set_id INTEGER NOT NULL
);
