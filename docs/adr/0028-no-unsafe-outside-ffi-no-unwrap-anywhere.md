# ADR 28: No `unsafe` outside platform FFI, and no `unwrap` anywhere

**Status:** Accepted, 2026-05-25

A music player is a long-running process that parses files it did not write, on a thread that
cannot afford to stall, and it ships with an updater that pushes new builds to people who did not
ask for them. Those three facts change what a memory-safety bug or a panic costs here compared to
an average crate.

Decision: `unsafe_code` is denied at the workspace level with per-site exceptions only for calls
into an operating system the type system cannot reach, and `unwrap` is denied crate-wide with no
exemption for tests.

Alternatives: `forbid` rather than `deny`; allowing `unsafe` in hot paths with a review
convention; the usual carve-out letting tests unwrap freely.

Trade: three things make a soundness bug expensive here. Release builds abort on panic, so
undefined behaviour does not surface as a clean crash with a backtrace; it surfaces as a corrupted
decode, a wrong colour, or a wrong answer weeks later on someone else's machine. The updater means
a bad release reaches installs that never asked for it, and the rollback path only catches a binary
that fails to start, not one that runs and is quietly wrong. And Miri is out of reach, because the
toolchain is pinned and that pin is the only toolchain installed, so new `unsafe` would ship
verified by review and nothing else.

`deny` rather than `forbid` is what makes the per-site exception legal, and it is deliberate: the
platform calls cannot be deleted, so forbidding would only push each exception into a shape that
evades the lint rather than removing any of them.

Never for performance is the half that gets argued with, so the record should be plain about it:
every hot spot that looked like it wanted `unsafe` here was fixed by asking a smaller question in
safe code, and the per-sample audio path is already index-free. A resampling library was passed
over during the audio migration for exactly this reason, because its adapter traits require
`unsafe` implementations (ADR 7).

Denying `unwrap` in tests is the unusual half and the one with a real cost: it makes assertions
wordier, and a helper that wants to unwrap has to return a result instead. What it buys is that
there is no suppression to copy. A test-only exemption is a pattern sitting in the tree that will
eventually be pasted somewhere that is not a test, and with zero of them nobody has the example.

This ADR was written in September 2026. Both lints were in place before the repository's first
commit; `.claude/rules/unsafe-rust.md` carries the sanctioned-site list and what to reach for
instead.
