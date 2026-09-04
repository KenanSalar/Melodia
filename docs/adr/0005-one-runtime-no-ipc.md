# ADR 5: One tokio runtime, direct calls, and no IPC

**Status:** Accepted, 2026-05-25

With the WebView gone (ADR 2) there was no longer a process boundary forcing a protocol between
the UI and everything behind it. That is an opportunity and a hazard in the same move: the
serialization boundary was also the thing that made a layering mistake impossible to write by
accident.

Decision: exactly one multi-threaded tokio runtime, created in `main` and shared as an
`Arc<Runtime>`. Slint's event loop owns the main thread and is never blocked. UI callbacks spawn
onto the runtime; results come back over `tokio::sync::watch` and `mpsc` and are written to Slint
properties by UI-thread tasks. Calls between layers are ordinary function calls. The only IPC in
the process is between two Melodias, over the single-instance socket.

Alternatives: a second runtime for audio or for the network, an actor or message-bus layer inside
the process, and keeping a request/response protocol as a discipline even with nothing forcing it.

Trade: one runtime means one place where thread counts, the blocking pool and shutdown are
decided, and it means a UI callback reaching a database row costs a function call rather than a
round trip through a codec. A second runtime buys isolation that is not needed and costs a second
set of workers whose threads are resident whether or not they are busy, in a process where memory
is a product requirement. An in-process message bus would reintroduce the boundary deliberately,
and it is the wrong instrument: it makes every call indirect in order to police a small number of
edges.

What it costs is exactly the hazard above. Nothing at runtime stops the UI reaching the database
or the engine reaching a toast, so those boundaries have to be held somewhere else, and they are
held by the crate graph: a layer that must not be reachable is absent from the dependent's
manifest, and rustc refuses the import by name. Audio is the one thing deliberately outside the
runtime, because cpal's device callback pulls the mixer on its own thread, so the DSP chain and
the mix run there rather than on a worker, and a live stream gets a dedicated thread to keep a
blocking socket read off that callback.

This ADR was written in September 2026 and reconstructed from `README.md`, `CLAUDE.md` and the
code.
