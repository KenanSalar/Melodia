# ADR 21: Lyrics live in the file, not in a column

**Status:** Accepted, 2026-07-20

Tag editing needs a Lyrics tab, and a lyrics tab looks exactly like a case for a `tracks.lyrics`
column: it is text belonging to a track, and everything else of that description is already a
column.

Decision: no lyrics column. The tag writer reads the lyrics tag when the dialog opens and writes
it back on save, both while it already has the file open, with the database uninvolved.

Alternatives: a `tracks.lyrics` column filled by the scanner; a side table keyed by track.

Trade: the column costs an additive migration, a new extracted-metadata field, and entry into the
lockstep contract between the insert column list, the column binder and the multi-row bind, plus
the metadata update path that shares that binder and is coupled to the same order. That is a lot
of surface for a feature that never reads from the database.

The real objection is memory. The scanner collects every scanned file into one vector before its
caller chunks them for ingest, so a lyrics column would hold the lyrics text for the entire
library resident for the duration of a scan. Lyrics are the one tag with no natural bound: a track
carries a few dozen bytes of title and artist and can carry kilobytes of lyrics. This project
exists because of memory regressions (ADR 2), so an unbounded per-track string in the scan path is
the specific thing it is supposed to refuse.

What it costs is that lyrics cannot be searched or displayed outside the editor, because nothing
can query them. That is the whole of the trade, and it is a real limit rather than a deferred one:
the moment a lyrics display or search feature is wanted, the column becomes correct, and it
arrives as a migration plus a backfill task, which is a pattern the tree already runs elsewhere.
Until then it would be a column nothing reads.

This ADR was written in September 2026 from the tag-editing working doc, deleted when that feature
shipped.
