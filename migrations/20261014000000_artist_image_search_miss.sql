-- When the artist-image pass last searched the directory for this artist and found nothing, in
-- Unix seconds.
--
-- The pass runs after every scan, a launch's no-change re-scan included, and kept its misses for
-- the session alone, so every launch searched again for every artist the directory doesn't know.
-- A recorded miss is skipped until it is old enough to retry. NULL for an artist never searched,
-- or whose last search failed in transit, which is retried on the next scan.
ALTER TABLE artists ADD COLUMN image_search_missed_at INTEGER;
