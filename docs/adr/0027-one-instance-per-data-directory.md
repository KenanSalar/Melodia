# ADR 27: One instance per data directory, and binding the socket is the claim

**Status:** Accepted, 2026-08-15

Two copies of Melodia running against one data directory is two writers on one database and one set
of JSON state files. It is also the ordinary result of double-clicking an audio file while the app
is already open, which is a thing people do without thinking about it.

Decision: at startup, after resolving the data directory and before anything else, the process tries
to bind a socket whose name is derived from that directory. The bind is the claim: whoever gets it
is the primary, and whoever fails hands its file arguments to the primary and exits.

Alternatives: a lock file with a stored process id; checking whether the socket exists and then
binding it; one instance per user rather than per data directory.

Trade: a lock file has to answer what a stale one means, which needs a liveness check on a recorded
process id, and process ids are reused. Probing before binding is the version that looks identical
and is not: two cold starts race in the window between the probe and the bind, and both conclude
they are first. Binding is atomic, so there is no window.

Keying on the data directory rather than on the user is what lets a development build and an
installed build run side by side, each with its own library, which follows from those already having
separate data roots ([ADR 4](0004-sqlite-through-sqlx.md)). It also means the name is a hash of
the path exactly as spelled, so the path is made absolute and its components collected first: a
relative value spells two different directories depending on where the process was launched from,
and a trailing separator spells one directory two ways. Either is two writers again, arriving
through the door this was built to close.

Two orderings are load-bearing and both are easy to undo. The claim happens before the logger
starts, so a launch that is only going to forward a filename never opens the shared log file. And
the claim happens early while accepting happens late, once there is a window to open a file into,
with the backlog holding whatever arrives in between.

One failure mode has no good answer, and it is resolved by picking the cheaper wrong thing: a claim
that fails for any reason other than a live primary boots anyway. Refusing to start because a socket
could not be created would turn an unusual filesystem into an app that will not open, and a
duplicate window is the smaller harm.

This ADR was written in September 2026 from `CLAUDE.md` and the code.
