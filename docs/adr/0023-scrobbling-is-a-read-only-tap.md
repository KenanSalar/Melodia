# ADR 23: Scrobbling is a read-only tap on the player, decided by a pure function

**Status:** Accepted, 2026-07-22

Scrobbling has to know what is playing and how long it has been playing, which makes it look like
something the playback state machine should tell it. Wiring it in that way puts a network feature
inside the machine that has to stay correct while holding a lock.

Decision: the scrobbler subscribes to the state and position the player already publishes, and
decides what to submit with a pure function over those two values. It writes nothing back and the
player does not know it exists.

Alternatives: hooking the existing play-count update; emitting a scrobble action from the state
machine; calling the provider from the playback path directly.

Trade: the play-count hook is the obvious reuse and it is wrong on timing. It fires near the end of
a track, so a listen that reaches the scrobble threshold and is then skipped never reaches it, and
that is a normal way to listen rather than an edge case. Emitting an action would put a network
call on a list that exists for side effects the machine owns, and the machine deliberately keeps
persistence off that list already.

A pure decision function over published state is testable without a player, a device or a network,
which is most of why the shape holds. What it costs is that the scrobbler polls a published value
rather than being told, so its resolution is the publish interval, and a state change faster than
that interval is not observable to it. For deciding whether a track was listened to, that is far
below the noise.

Credentials live in their own file with restrictive permissions rather than in the settings file
or in an OS keyring. The settings file is read and rewritten constantly and is the file a user is
most likely to paste into a bug report. A keyring on Linux means a D-Bus client, and the one
available conflicts with the accessibility stack the UI toolkit already runs, which is a known
trap in this tree. The trade is honest: the token is on disk in plaintext, protected by file mode
alone, and anything with the user's own privileges can read it.

This ADR was written in September 2026 from the scrobbling working doc and the rule that went with
it, both deleted.
