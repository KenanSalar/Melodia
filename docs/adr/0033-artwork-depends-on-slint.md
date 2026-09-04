# ADR 33: The artwork crate depends on the UI toolkit, and stores its buffers

**Status:** Accepted, 2026-09-03

The workspace graph ([ADR 16](0016-workspace-graph-compiler-enforced.md)) has a pure image
crate near the bottom, below the UI, and that crate names the UI toolkit in its manifest. That reads
like a layering violation, and someone will eventually try to tidy it up.

Decision: the artwork crate depends on the toolkit for one thing, its reference-counted pixel buffer
type, and the thumbnail cache stores those buffers rather than raw bytes or finished images.

Alternatives: storing plain byte vectors so the crate names no toolkit; an extension trait leaving
the buffers below and the image accessors in the UI crate.

Trade: the dependency is on a refcounted pixel buffer, which is the same category of thing as any
shared byte buffer, and never on the event loop or on any widget. The distinction that matters is
what a cache hit costs: handing back a shared buffer is a refcount bump, while copying out of a byte
vector is a per-read memcpy of the whole thumbnail, on a path that runs while a grid is scrolling.
That converts a cheap read into transient allocation per row, which is the thing this tree is least
willing to spend ([ADR 30](0030-memory-is-a-product-requirement.md)).

The extension trait is the tidier-looking loser. The buffers still have to be that type, so the
crate still names the toolkit and no dependency is removed; it buys naming purity and nothing else,
and it is an abstraction whose only purpose is to satisfy the split, which is the stopping rule the
workspace split set for itself ([ADR 18](0018-what-is-not-split-and-zero-features.md)).

Storing buffers rather than finished images is forced rather than chosen. A finished image is
deliberately neither sendable nor shareable across threads, so it can neither live in a cross-thread
cache nor come out of a parallel decode pipeline. The buffer is both.

The cost is that a crate whose job is decoding and resizing cannot be reused anywhere the toolkit is
absent, and that the toolkit's version is now pinned by two layers instead of one.

This ADR was written in September 2026 from the workspace split working doc and the cache's own
module documentation.
