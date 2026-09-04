# ADR 24: Discord presence speaks its IPC directly, with no new dependency

**Status:** Accepted, 2026-07-24

Rich presence is a small protocol over a local socket: a length-prefixed frame, a JSON payload, a
handshake. There are crates for it, and reaching for one is the obvious move.

Decision: about two hundred lines speaking the protocol directly, on a detached worker thread,
with no new dependency.

Alternatives: the most-used presence crate; its better-maintained sibling; writing nothing and
dropping the feature.

Trade: the obvious crate pins a UUID library one major behind the one already in this tree, purely
to generate a per-command nonce. Taking it means two majors of that library compiled into the
binary for a counter, which is the same duplicate-major problem the audio stack spent a migration
escaping (ADR 7). The sibling crate resolves that pin correctly and brings seven further crates
and its own threading model, which is a lot of surface for a status line.

What the tree gets instead is a protocol it can read end to end and change in one line. Discord
adds fields to that payload occasionally, and a new one is an addition here rather than waiting on
a release. The cost is that the protocol is now ours to track: if Discord changes the handshake or
the framing, this breaks and nobody upstream fixes it for us, and the socket-level details, the
partial-read loop and the platform differences in how the socket is opened are all things a
dependency would have absorbed.

The reversal condition is worth writing down because it is specific and it may well happen. If the
obvious crate bumps to the current UUID major, the argument above evaporates, and the transport
here deletes cleanly: the model and the decision machine were built to sit above it and would not
change.

This ADR was written in September 2026 from the Discord working doc and the rule that went with
it, both deleted.
