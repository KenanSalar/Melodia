# ADR 36: Architecture is held by tests that read the source

**Status:** Accepted, 2026-08-03

Several properties this tree depends on cannot be stated to the compiler. Exactly one wrapper may
name the file-dialog crate. Every HTTP body must be read through the one capped reader. Nothing
outside the radio facade may name the directory client. A thread name must fit in fifteen bytes or
Linux truncates it silently. Each is violable from any file, free to break, and invisible in review,
because the diff that breaks it looks perfectly correct on its own.

Decision: those properties are pinned by integration tests that walk the source tree as text and
assert on what they find. Seventeen of the twenty-nine test files in the binary crate do this.

Alternatives: writing a custom lint; a review convention; leaving them unenforced.

Trade: a lint is the right instrument for this and it is out of reach. Writing one needs either a
nightly toolchain or a lint-driver crate, and the toolchain here is pinned to a single stable
version precisely so the strict lint gate stays survivable. A convention is what these replaced, and
it did not work: the two boundary rules that were conventions before the workspace split were
checked by whoever remembered to check, which is how both of them had drifted by the time anyone
looked.

What a walk buys over a convention is simply that it runs. What it costs is that it reads text, so
it can be fooled by a spelling nobody anticipated, and worse, it can rot in silence: a walk whose
anchor has moved matches nothing, and a walk that matches nothing passes, taking every pin standing
on it with it. That failure is why the craft around these is not optional, and why
`.claude/rules/testing.md` spends a section on what one owes: a vacuity floor, an equality wherever
a vanishing subtree is the failure, unreadable paths asserted empty rather than skipped, and
exemptions held to an exact count. A walk also lives outside the tree it walks, which retires the
self-exemption it would otherwise owe as its own first hit.

The second cost is where it reports. A violation arrives as a failing test in a suite run rather
than as a compile error at the import, so the person who broke it learns later and further from the
line than they would with a type error.

The rule that keeps this from spreading: where the compiler can hold a property instead, it should.
That is most of what the workspace split bought
([ADR 16](0016-workspace-graph-compiler-enforced.md)), which retired several of these by
turning them into manifest lines. A walk is the instrument for what is left over, not the first
thing to reach for.

This ADR was written in September 2026 from `.claude/rules/testing.md` and the tests themselves.
