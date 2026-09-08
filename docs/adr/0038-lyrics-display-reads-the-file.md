# ADR 38: The lyrics display reads the file, one track at a time

**Status:** Accepted, 2026-09-08

[ADR 21](0021-lyrics-live-in-the-file.md) refused a `tracks.lyrics` column, and it named the thing
that would overturn it: the moment a lyrics display or search feature is wanted, the column becomes
correct. A lyrics display has now been built, so on the face of it that decision has just expired.

Decision: no column. The panel reads one file when the track changes, through the same tag read the
tag editor already does, plus a `.lrc` sidecar beside the track and a cache of what the online
directory answered. Nothing goes near the database and there is no migration.

Alternatives: the column ADR 21 costed, filled by the scanner and backfilled; a side table keyed by
track; keeping the parsed sheets of tracks already played, so going back to one costs nothing.

Trade: what ADR 21 objected to was memory, not schema. The scanner gathers every scanned file into
one vector before its caller chunks that up for ingest, so a lyrics column would hold the whole
library's lyrics in memory for as long as a scan runs, and lyrics have no natural size limit. A
display never asks a question about more than one track, so it never reaches that vector. It reads
one file, for the track that is playing, when the user changes it. The half of the exit ramp that
still stands is search, which does need the column and is not what was built here. Two things are
given up for that. Nothing can find a track by its words, which is the limit ADR 21 already
described and this does not lift. And the panel cannot know whether a sheet exists until it has gone
and looked, so its empty state has to mean "we have not looked yet" as well as "there is none", and
those are different things to somebody watching it.

Only the playing track's sheet is held in memory. That is what keeps the memory argument honest, and
it is also where the bugs are: every path that empties the panel needs a matching path that fills it
again, and the first version shipped with one of those missing. It looked like a panel that needed
the app restarted.

The three sources are ordered so that a wrong answer can be corrected. A sidecar someone put there
on purpose beats whatever a tagger wrote, and both beat a stranger's upload. The fix for a wrong
sheet is therefore to drop a `.lrc` next to the file, with no setting to find and nothing to
re-scan. Timings are the one thing that cuts across the order. Somebody who typed plain prose into a
lyrics tag has not thereby asked for the sung line to go unmarked.

The online half ships off. The guard is a single early return in the facade rather than a gate in
the UI, which is [ADR 15](0015-radio-ships-off-guarded-at-the-facade.md)'s shape and is taken for
ADR 15's reason: two shipped product descriptions promise that every online feature is off until you
turn it on, and a lookup that shipped on would make both of them false.
`crates/melodia/tests/lyrics_switch.rs` holds it in two halves, an equality on the single reading of
the flag inside the facade and a walk over every crate for a mention of the directory client outside
it.

The cache is keyed on a hash of the track's path. [ADR 20](0020-artwork-swept-not-refcounted.md)'s
artwork store hashes contents instead, and the difference is deliberate rather than an oversight. A
tag edit rewrites the file and moves its content hash, so a sheet stored that way would be lost the
moment its owner corrected a typo in the title, which is a likely thing to do straight after reading
the words. Both schemes produce the same sixteen hex characters, so the module says which one it is.

This ADR was written in September 2026 from the lyrics working doc, deleted when that feature
shipped.
