# ADR 37: One workspace-wide error type, and a failure is never carried as a `String`

**Status:** Accepted, 2026-05-25

An error here starts at the bottom of the tree and ends in something a person reads. A scan that
fails has to arrive at the UI still carrying what failed and why, because a permissions problem and
a full disk are different things to the person looking at the dialog, and by the time either has
been formatted into a message they are the same thing.

Decision: one `AppError` for the whole workspace. The four variants that sit at an I/O boundary are
struct variants carrying a context message and a typed cause as separate fields; the rest use
`Display` and conversions. No `Result` anywhere in the tree carries its error as a `String`.

Alternatives: anyhow or eyre; a per-crate error type with conversions at every boundary; string
errors.

Trade: anyhow is the usual answer and it is built for the case where nothing matches on the error.
This tree matches. The library API hands a `Result<T, AppError>` to the UI, which decides between a
toast, a dialog and a silent skip on the variant, and erasing the type turns that decision into
string inspection. Per-crate types are the textbook shape for a workspace and buy precision at the
cost of a conversion at every edge of an eleven-crate graph, for a binary that renders nearly all of
them the same way at the end of it.

The context-and-cause split is the half that earns its keep, and it is easy to undo without
noticing. `Display` on those variants prints the operation and not the reason, so a log line built
from the error alone reports a permissions failure and a full disk identically. Recovering the
second half means walking the source chain, which only works while the cause is still attached, and
formatting an error into its own message is exactly what detaches it.

A `String` error is that same loss one step earlier. It keeps the message, drops the cause, and does
it at the throw rather than at the read, which is backwards: the throw site is the one with the
least idea of what the eventual reader needs. The rule is violable from any file and free to break,
so a corpus walk holds it rather than review
([ADR 36](0036-architecture-held-by-corpus-walks.md)). Where `AppError` genuinely cannot be
named the answer is a local type implementing the standard error trait rather than a string: the
blocklist parser is included into a build script and may name no crate path, so it defines its own.

The cost is a single enum that every crate depends on, which means it accumulates variants that most
of its dependents will never construct, and a change to it touches the whole graph. That is the
price of the boundary being one type wide.

This ADR was written in September 2026. The type dates from the repository's first commit; the
source-walking helper that recovers a cause for a log line arrived on 2026-08-09.
