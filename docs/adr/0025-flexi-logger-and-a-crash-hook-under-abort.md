# ADR 25: A rotating log file and a crash hook that survives `panic = "abort"`

**Status:** Accepted, 2026-08-06

A release build has no console on Windows and is launched from a desktop entry on Linux, so
everything the app logged went to a standard error nobody would ever see. A user reporting a bug had
nothing to attach, and a panic left no trace at all.

Decision: log to a rotating file with a size and age bound, through a logging backend that does both
in one, and install a panic hook that writes a crash report next to the logs before the process
goes.

Alternatives: keeping the existing logger and fanning out to a file by hand; a tracing-based file
appender; two other file-logging crates.

Trade: the instinct here is that adding a logging crate means adding weight, and re-resolving the
lock file after the swap said the opposite: thirteen crates left and two arrived. That is the sort
of claim worth measuring rather than predicting, and it inverted the decision. The alternatives lose
on rotation specifically: one rotates by date only, which means an unbounded file for anyone who
leaves the app running, and another is configuration-file oriented for a program that needs one line
of setup.

The panic hook is the part with a real constraint behind it. Release builds abort on panic, and the
obvious worry is that an aborting panic skips the hook. It does not: the hook runs before the abort
path is entered, so coverage is complete rather than partial. What that buys is paid for by order
inside the hook, which is load-bearing and easy to get wrong: the report is written with plain file
operations before anything reaches the logger, because the logger is exactly the machinery that may
be the thing that failed.

Two consequences follow. The release profile strips debug info rather than all symbols specifically
so a user's backtrace still has names in it, which is a size cost taken on purpose. And the log
directory is shared by two independent cleanup mechanisms, one for rotated logs and one for crash
reports, each gated on names it wrote itself, so a third kind of file there needs checking against
both.

The whole of this is only safe because no log call anywhere interpolates a credential. That is a
property of every file rather than of the logger, and it is what makes attaching a log tail to a
public issue a reasonable thing to ask a user to do.

This ADR was written in September 2026 from the crash-reports working doc, deleted when that work
shipped.
