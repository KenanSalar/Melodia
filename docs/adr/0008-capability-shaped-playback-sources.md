# ADR 8: Playback sources are capability-shaped, not a source-kind enum

**Status:** Accepted, 2026-08-20

When internet radio landed, "what is playing" became a boolean. `PlayerState` carried an
`Option<RadioNowPlaying>`, and the question "is this a station" was asked through `is_radio()` at
twelve sites inside the state machine plus sixteen more `is_some()` and `is_none()` checks
elsewhere. Podcasts and streaming were already queued behind it, and each of those forty-odd
sites turns into a three or four way question the moment a third kind exists. Every one left as a
boolean silently treats a new source as a local file.

Decision: `PlayerState` carries a `PlaybackSource`, and the transport asks it for a capability
rather than for its variant. `source_allows(PlaybackSource::advances_queue)` and its siblings
replace the branch on kind, so a site says what it actually needs to know.

Alternatives: four variants with four code paths, one per source kind; leaving the boolean and
adding a special case per kind as each arrives.

Trade: four variants is the obvious shape and it is a catalogue taxonomy rather than a pipeline
one. The pipeline asks four questions, and the kinds do not divide along them. A podcast episode
comes off HTTP like a station but is finite, seekable, resumable, and advances a queue, which
makes it far closer to a local file than to radio. Radio is the odd one out on every question:
nothing to seek, nothing to advance to, and a position that means elapsed time rather than a place
in anything. So what the type carries is the answers the boolean was standing in for, and a new
source kind states its capabilities instead of adding a fifth arm to forty branches.

What it costs is that a capability set is weaker than an enum where the code genuinely does need
to know the kind, and it does in five places: the pause that drops a station's socket, the play
that cannot resume from one, the stop that forgets it, the session check, and the track that
evicts it. Those keep `is_radio`, and each is about a station specifically rather than about a
category of source. Below that line nothing changed, because the pipeline was already right: the
stream source and its prebuffer are source-agnostic, and a podcast reuses both with a seekable
store in place of the bounded ring.

It was introduced in the decode migration (ADR 7) rather than after it, because that is the seam
that owns opening a source and deciding what can be done with it, so it was one rewrite rather
than two. Leaving it for podcasts would have meant radio, then a podcast special case, then a
streaming special case, then this migration, then a fourth pass to undo all three.
