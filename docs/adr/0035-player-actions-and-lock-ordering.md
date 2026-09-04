# ADR 35: State mutation returns an action list, and the pair is serialized

**Status:** Accepted, 2026-05-25

A player has to change state and then act on it: hand a file to a deck, bump a play count, put a
toast on screen. Doing both under the state lock is the obvious shape and what a state machine
usually does. It also means holding a mutex across a decode, a device call and a database write, on
the same mutex the UI reads through to paint.

Decision: a mutation runs under the lock and returns a `Vec<PlayerAction>`, the lock drops, and the
actions execute afterwards. The two halves are paired only through one function, which holds a
per-handle execution lock across both so that mutation order equals side-effect order. The lock
order is the execution lock, then the state, then the decks, and it never reverses. Persistence is
deliberately not an action.

Alternatives: doing the work under the state lock; an actor that owns the state and receives
messages; letting each caller pair the two halves itself.

Trade: deferring the effects is what keeps the lock short enough that a view-model publish never
queues behind a device call. An actor would give the same serialization for free and costs a message
round trip on every read, which is the wrong bill for state the UI reads to paint every frame.

Pairing the halves by hand was the original spelling and it is wrong in a way that only appears
under load. Mutation stays atomic on its own, but the effects that follow run on whichever worker
the caller happens to be on, so two batches from different tasks, the playback monitor advancing at
end of stream and a stop arriving from the UI, can interleave their effects and leave the state and
the backend disagreeing. One lock across both halves closes that window, and because it spans only
synchronous work a blocking mutex is the right one.

The cost is a gap the shape cannot remove. A decision made under the state lock executes after the
execution lock is taken, and any control operation can land in between, so a decision can go stale
between being made and being acted on. The crossfade builder re-verifies its own decision for
exactly that reason, and that re-verification is the price of the shape rather than a defect in it.

Persistence stays off the list because an action is something the engine can carry out with what it
already has. Writing a row needs a database pool, and a pool on that side is what would pin the
engine to sqlx, which the crate graph exists to prevent
([ADR 16](0016-workspace-graph-compiler-enforced.md)). So a play count is announced on a
channel and written by a flusher above, and the queue and position saves go through the library
layer directly.

This ADR was written in September 2026. The action list dates from the repository's first commit;
the execution lock that serializes the pair arrived on 2026-07-08, and the function that holds both
halves argues its own mechanism at its definition.
