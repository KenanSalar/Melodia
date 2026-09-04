# ADR 14: The network never touches the audio callback thread

**Status:** Accepted, 2026-08-20

The mixer pulls samples inside the audio device's callback. Whatever is on the end of that pull
runs on that thread, and a live stream's end is a socket. A blocking read there stalls the entire
output block for as long as the network is wedged, which is not just the station going quiet: it is
every deck, so a local track crossfading out of a station stalls with it, and a wedged connection
becomes an application-wide audio dropout.

Decision: a ring buffer with its own feed thread sits between the network and the decoder. The
feed thread fills it and can block as much as it likes; the source above it pops without blocking
and yields silence when the ring is dry rather than ending. This is a structural constraint on the
whole player, not a property of the radio feature.

Alternatives: reading the socket inside the source and accepting the stall; an async source polled
from the runtime; a larger read buffer to make the stall rare.

Trade: this is the one decision here with no real competition, which is worth recording precisely
because that makes it look like an implementation detail later. A bigger buffer makes the stall
rarer and not shorter, which is the worst of both: it fails less often and just as badly, and it
costs resident memory in a process where memory is a product requirement (ADR 2). An async source
does not help either, because the callback is not a runtime worker and cannot await anything; it
would mean blocking on a future, which is the original stall wearing a different type.

What it costs is a thread per live stream, a copy through the ring, and the two edge cases that
come with a producer and a consumer at different rates. A dry ring yields `Some(0.0)` rather than
`None`, because ending the source would tear the deck down over a stall the connection is about to
recover from. And a partial frame is never split across the ring boundary and the silence that
follows it, or a stereo frame shears there and that deck's channel parity flips permanently.

Two things fall out of it that are worth naming because they look like separate decisions. Because
starvation is already a state the ring knows about, it is published as the buffering indicator the
UI needed anyway rather than being inferred. And reconnect lives in the feed thread rather than in
the playback monitor: that thread already holds the URL, the client and the ring, so it re-opens
and keeps filling the same ring, the source never ends, the deck never blinks, and the state
machine needs no reconnect path at all. Putting it in the monitor would mean handing the engine an
HTTP client and inverting the dependency direction the crate graph exists to hold.
