# ADR 10: The DSP chain is one wrapper in a fixed order, with lock-free state

**Status:** Accepted, 2026-06-16

The graphic equalizer was the first thing Melodia wanted that no playback library was going to
provide: the ones available ship low-pass and high-pass filters and no peaking filter at all, so a
graphic EQ cannot be built from their primitives. That made the question bigger than the EQ. Once
the chain is ours, its shape decides what every later stage costs, and the audio callback is
pulling it, so how a slider reaches it matters as much as what it does.

Decision: every per-sample stage lives inside one source wrapper, in a fixed order, sitting
directly above the decoder so the playing track and the gapless-preloaded one both get it. Values
that belong to a track are baked into the wrapper when it is built. Controls that belong to the
user live in lock-free shared state that the wrapper polls by generation counter, and it bypasses
entirely when everything is neutral.

Alternatives: a separate source per stage, composed; a lock or a channel to deliver parameter
changes to the audio thread; a shared cell for per-track values as well as user ones.

Trade: one wrapper means one place where order is decided, and the order carries real weight.
ReplayGain applies as a pre-gain before the bands, which means the limiter that exists to catch an
EQ boost catches a ReplayGain boost for free rather than needing a second one. The clamp lands
after both, and the crossfade ramp after that (ADR 9), so the invariant that keeps the mixer safe
is a property of one file rather than of a composition anyone can reorder. A stage per source
would read more cleanly and would put that ordering back in the hands of whoever assembles them.

The per-track and per-user split is not a style choice. A gapless-preloaded track has different
tags from the one playing, so its gain has to ride its own wrapper; on a shared cell the next
track would play at this track's gain, which is audible and arrives exactly at the transition
where nobody is looking. User controls have to go the other way, applying live to both the playing
source and the preloaded one, which is what the generation counter is for.

Lock-free is the only option that survives the constraint rather than the fastest of several. The
audio callback pulls this chain, so anything that can block in it can stall the whole output
block, local tracks included. Atomics plus a generation counter mean a slider drag costs the audio
thread a counter comparison per frame and a coefficient rebuild only when something actually
moved. What it costs is that the state is spread across small atomic cells rather than sitting in
one struct behind a lock, and every reader has to remember to poll.

Two consequences worth naming because they are easy to get wrong later. The filter form is chosen
so that a coefficient swap mid-playback injects no transient, which is what makes a live slider
drag usable rather than a source of clicks. And the visualizer tap wraps the chain from outside
rather than living in it: it reads after the bands, the limiter's clamp and the crossfade ramp but
before the deck's conversion, pause and volume, so the volume slider cannot flatten the bars and
the tap does not have to be threaded through the poll gate and the bypass path that have nothing
to do with it.

This ADR was written in September 2026, reconstructed from the working docs for the equalizer, for
ReplayGain and for the visualizer, all three of which were deleted when their features shipped.
The three arrived in that order and each is the same decision applied again.
