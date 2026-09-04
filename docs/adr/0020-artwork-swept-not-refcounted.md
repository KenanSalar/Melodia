# ADR 20: Artwork is content-addressed and swept, never reference-counted

**Status:** Accepted, 2026-08-19

Cover art is shared. One file backs an album, every track on it, and sometimes a playlist thumbnail,
so deleting a track must not unlink the image eleven other rows still point at. The store had also
already accumulated orphans that no per-delete logic could ever have reached, because there was
nothing left to trigger on.

Decision: images are stored under a content hash and collected by a periodic sweep that reads the
columns naming them and deletes what nothing names. There is no reference count anywhere.

Alternatives: a reference count decremented on every path that drops a cover; deleting eagerly at
the point a row goes away; never collecting at all.

Trade: a count has to be exactly right across scan ingest, orphan cleanup, both watcher delete and
rename paths, a tag edit that replaces a cover, and composite regeneration. Undercount and live
artwork vanishes silently; overcount and it leaks anyway, which is the failure it was added to
prevent. A sweep cannot undercount because it never counts. It also cleans what is already orphaned,
which a count cannot, since there is nothing to decrement for a file whose referrer is long gone, so
a count would have needed a one-time sweep regardless and is then redundant machinery beside it.

The sweep is only ever as correct as its list of columns, and that list is not obvious and does not
stay still. The three anyone would guess are tracks, albums and artists. Playlist thumbnails point
into the same store under the same scheme and are the fourth, and some are reachable through no
other column at all: a three-column sweep deletes them, and the ones that survive do so only by
happening to alias a track's cover, which is not a property to rely on. That is the count's own
failure mode landing on the query that replaced it. Two more columns have joined since, both for
station logos, and those cannot be rebuilt by rescanning because they came off a third-party host.
So the reference set is pinned by a test naming the columns rather than by a reviewer remembering
them, and the standing hazard is a new column that stores a path and does not join it.

The store is capped on ingest for a reason that is not disk. Every display tier decodes the stored
file whole before resizing, and the tiers share no decodes, so the cap is the ceiling on every
transient decode buffer in the app rather than just on what is kept. A full-resolution artwork view
would decode from the user's own file rather than raise it.

This ADR was written in September 2026 from the working doc for the artwork store, which was deleted
when that work shipped.
