-- The photo the artist-image pass fetched for the Unknown Artist placeholder.
--
-- The pass searched the directory by name for every artist without an image, the
-- placeholder included, so an install that ran it shows some stranger's photo under
-- every track with no artist tag. The pass skips the placeholder now
-- (queries::artist::UNKNOWN_ARTIST_ID); this clears what it already wrote, and the
-- artwork sweep retires the file.
UPDATE artists
SET
    image_path = NULL
WHERE
    id = 1;
